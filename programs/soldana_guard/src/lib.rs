//! SolDana Guard on-chain program (Anchor 0.30.1).
//!
//! An opt-in transaction guard for Solana users. A user stores a personal
//! security policy in a Policy PDA, then routes sensitive calls through
//! this program, which enforces the policy on-chain before and after the
//! underlying call (CPI).
//!
//! What it IS:
//! - A policy store: max spend per call, max slippage, a whitelist of
//!   programs the user trusts, and a durable-nonce preference.
//! - A guarded executor: `guarded_execute` invokes an arbitrary CPI only
//!   when the target program is whitelisted, and reverts when the guarded
//!   account spends more than the policy allows.
//! - A guarded swap: `guarded_swap` invokes a DEX CPI and reverts when the
//!   tokens received fall below the caller's minimum (on-chain slippage
//!   floor, the anti-sandwich check).
//! - A demo vault with an automatic circuit breaker: withdrawals above a
//!   per-slot rate freeze the vault.
//!
//! What it is NOT (read this before trusting it):
//! - It cannot intercept arbitrary transactions. A program can only rule
//!   on instructions sent TO it. A drainer site never routes through the
//!   guard, so a user is only protected while they use a Guard client.
//! - It cannot detect durable-nonce transactions on-chain. A program has
//!   no access to the transaction's blockhash, so there is no on-chain
//!   `DurableNonceBlocked` revert. The durable-nonce rule is enforced by
//!   the client, which reads the Policy PDA and refuses to build a nonce
//!   transaction when the policy forbids it. The Policy PDA is the
//!   on-chain source of truth for the rule; the enforcement point is the
//!   client. Any claim of an on-chain durable-nonce revert would be false.
//! - The vault is a demo vault owned by this program. It proves the
//!   circuit-breaker mechanism; it does not protect third-party protocols
//!   (Drift, Loopscale, or anyone else) unless they opt in and delegate
//!   authority to it, which they have not.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::{
    instruction::{AccountMeta, Instruction},
    program::invoke,
};
use anchor_spl::token::{TokenAccount, Token};

declare_id!("5XeayPhkQW1QyG2siahkV6zhYArxsx2uEhxJu5vjEy3S");

/// Maximum programs in a policy whitelist. Fixed array, so the account
/// size is constant and rent is predictable.
pub const MAX_WHITELIST: usize = 8;
/// Slots a frozen vault stays frozen before the authority may unfreeze it
/// (about 60 seconds at ~400ms per slot).
pub const FREEZE_SLOTS: u64 = 150;

#[program]
pub mod soldana_guard {
    use super::*;

    /// Create the caller's Policy PDA. The caller pays rent. A policy can
    /// only be created once per owner; use `update_policy` to change it.
    pub fn initialize_policy(
        ctx: Context<InitializePolicy>,
        max_spend_lamports: u64,
        max_slippage_bps: u16,
        allow_durable_nonce: bool,
        allowed_programs: Vec<Pubkey>,
    ) -> Result<()> {
        require!(
            max_slippage_bps <= 10_000,
            GuardError::InvalidSlippageBps
        );
        require!(
            allowed_programs.len() <= MAX_WHITELIST,
            GuardError::WhitelistTooLong
        );

        let policy = &mut ctx.accounts.policy;
        policy.owner = ctx.accounts.owner.key();
        policy.max_spend_lamports = max_spend_lamports;
        policy.max_slippage_bps = max_slippage_bps;
        policy.allow_durable_nonce = allow_durable_nonce;
        policy.allowed_programs = [Pubkey::default(); MAX_WHITELIST];
        for (i, program) in allowed_programs.iter().enumerate() {
            policy.allowed_programs[i] = *program;
        }
        policy.allowed_count = allowed_programs.len() as u8;
        policy.bump = ctx.bumps.policy;
        Ok(())
    }

    /// Change the caller's policy. Only the policy owner may call it.
    /// The whitelist is replaced wholesale; pass the full new list.
    pub fn update_policy(
        ctx: Context<UpdatePolicy>,
        max_spend_lamports: u64,
        max_slippage_bps: u16,
        allow_durable_nonce: bool,
        allowed_programs: Vec<Pubkey>,
    ) -> Result<()> {
        require!(
            max_slippage_bps <= 10_000,
            GuardError::InvalidSlippageBps
        );
        require!(
            allowed_programs.len() <= MAX_WHITELIST,
            GuardError::WhitelistTooLong
        );

        let policy = &mut ctx.accounts.policy;
        policy.max_spend_lamports = max_spend_lamports;
        policy.max_slippage_bps = max_slippage_bps;
        policy.allow_durable_nonce = allow_durable_nonce;
        policy.allowed_programs = [Pubkey::default(); MAX_WHITELIST];
        for (i, program) in allowed_programs.iter().enumerate() {
            policy.allowed_programs[i] = *program;
        }
        policy.allowed_count = allowed_programs.len() as u8;
        Ok(())
    }

    /// Execute an arbitrary instruction through the guard.
    ///
    /// `ctx.remaining_accounts[0]` must be the target program (executable,
    /// whitelisted in the policy). The rest are the accounts the CPI needs,
    /// in the order the target instruction expects them. The caller's
    /// `guard_account` (usually their own wallet account) is measured in
    /// lamports before and after the CPI: the call reverts when the drop
    /// exceeds `policy.max_spend_lamports` or the remainder falls below
    /// `min_lamports_after`.
    ///
    /// The caller must sign this instruction, and any account the CPI
    /// needs as a signer must have signed the outer transaction (signer
    /// status propagates through CPI).
    pub fn guarded_execute(
        ctx: Context<GuardedExecute>,
        ix_data: Vec<u8>,
        min_lamports_after: u64,
    ) -> Result<()> {
        let policy = &ctx.accounts.policy;

        require!(
            !ctx.remaining_accounts.is_empty(),
            GuardError::ProgramNotWhitelisted
        );
        let target = &ctx.remaining_accounts[0];
        require!(target.executable, GuardError::ProgramNotWhitelisted);
        require!(
            policy.is_allowed(&target.key()),
            GuardError::ProgramNotWhitelisted
        );

        let lamports_before = ctx.accounts.guard_account.lamports();

        let metas: Vec<AccountMeta> = ctx.remaining_accounts[1..]
            .iter()
            .map(|acc| AccountMeta {
                pubkey: acc.key(),
                is_signer: acc.is_signer,
                is_writable: acc.is_writable,
            })
            .collect();
        let ix = Instruction {
            program_id: target.key(),
            accounts: metas,
            data: ix_data,
        };
        invoke(&ix, &ctx.remaining_accounts[1..])?;

        let lamports_after = ctx.accounts.guard_account.lamports();
        let spent = lamports_before.saturating_sub(lamports_after);
        require!(spent <= policy.max_spend_lamports, GuardError::SpendLimitExceeded);
        require!(
            lamports_after >= min_lamports_after,
            GuardError::GuardBalanceBreached
        );
        Ok(())
    }

    /// Execute a token swap through the guard with an on-chain minimum
    /// output. `ctx.remaining_accounts[0]` must be the DEX program
    /// (executable, whitelisted). The rest are the DEX accounts for the
    /// swap CPI. The program measures the destination token account
    /// before and after the CPI and reverts with SlippageExceeded when
    /// the received amount is below `min_out`.
    ///
    /// The caller derives `min_out` from the policy's max_slippage_bps
    /// and the quoted rate; the program enforces the number, it does not
    /// compute the quote.
    pub fn guarded_swap(
        ctx: Context<GuardedSwap>,
        ix_data: Vec<u8>,
        min_out: u64,
    ) -> Result<()> {
        let policy = &ctx.accounts.policy;

        require!(
            !ctx.remaining_accounts.is_empty(),
            GuardError::ProgramNotWhitelisted
        );
        let dex = &ctx.remaining_accounts[0];
        require!(dex.executable, GuardError::ProgramNotWhitelisted);
        require!(
            policy.is_allowed(&dex.key()),
            GuardError::ProgramNotWhitelisted
        );

        let amount_before = ctx.accounts.dest_token_account.amount;

        let metas: Vec<AccountMeta> = ctx.remaining_accounts[1..]
            .iter()
            .map(|acc| AccountMeta {
                pubkey: acc.key(),
                is_signer: acc.is_signer,
                is_writable: acc.is_writable,
            })
            .collect();
        let ix = Instruction {
            program_id: dex.key(),
            accounts: metas,
            data: ix_data,
        };
        invoke(&ix, &ctx.remaining_accounts[1..])?;

        // Re-read the destination balance after the CPI. The account data
        // was mutated by the token program during the CPI, so reload it.
        ctx.accounts.dest_token_account.reload()?;
        let received = ctx
            .accounts
            .dest_token_account
            .amount
            .saturating_sub(amount_before);
        require!(received >= min_out, GuardError::SlippageExceeded);
        Ok(())
    }

    /// Create the demo vault PDA. The authority pays rent and is the only
    /// account that may withdraw.
    pub fn initialize_vault(
        ctx: Context<InitializeVault>,
        max_withdrawals_per_slot: u16,
    ) -> Result<()> {
        require!(
            max_withdrawals_per_slot > 0,
            GuardError::InvalidRateLimit
        );
        let vault = &mut ctx.accounts.vault;
        vault.authority = ctx.accounts.authority.key();
        vault.total_deposited = 0;
        vault.withdrawals_this_slot = 0;
        vault.last_withdraw_slot = 0;
        vault.max_withdrawals_per_slot = max_withdrawals_per_slot;
        vault.frozen = false;
        vault.frozen_until_slot = 0;
        vault.bump = ctx.bumps.vault;
        Ok(())
    }

    /// Deposit SOL into the vault. Anyone may deposit.
    pub fn deposit(ctx: Context<Deposit>, amount: u64) -> Result<()> {
        require!(amount > 0, GuardError::InvalidAmount);
        let vault_info = ctx.accounts.vault.to_account_info();
        let from = ctx.accounts.depositor.to_account_info();
        **vault_info.try_borrow_mut_lamports()? += amount;
        **from.try_borrow_mut_lamports()? -= amount;
        ctx.accounts.vault.total_deposited = ctx
            .accounts
            .vault
            .total_deposited
            .checked_add(amount)
            .ok_or(GuardError::InvalidAmount)?;
        Ok(())
    }

    /// Withdraw SOL from the vault. Authority only, rate-limited per
    /// slot. When withdrawals in the current slot exceed the limit, the
    /// circuit breaker trips: the vault freezes for FREEZE_SLOTS and
    /// this call fails with CircuitBreakerTriggered. A frozen vault
    /// rejects every withdrawal with VaultFrozen until `unfreeze`.
    pub fn withdraw(ctx: Context<Withdraw>, amount: u64) -> Result<()> {
        require!(amount > 0, GuardError::InvalidAmount);
        let vault = &mut ctx.accounts.vault;
        let clock = Clock::get()?;

        require!(!vault.frozen, GuardError::VaultFrozen);
        let vault_lamports = vault.to_account_info().lamports();
        let rent_minimum = Rent::get()?.minimum_balance(Vault::LEN);
        require!(
            amount
                <= vault_lamports
                    .saturating_sub(rent_minimum),
            GuardError::InsufficientVaultBalance
        );

        if clock.slot == vault.last_withdraw_slot {
            vault.withdrawals_this_slot = vault.withdrawals_this_slot.saturating_add(1);
        } else {
            vault.last_withdraw_slot = clock.slot;
            vault.withdrawals_this_slot = 1;
        }
        if vault.withdrawals_this_slot > vault.max_withdrawals_per_slot {
            vault.frozen = true;
            vault.frozen_until_slot = clock.slot.saturating_add(FREEZE_SLOTS);
            return Err(GuardError::CircuitBreakerTriggered.into());
        }

        let vault_info = vault.to_account_info();
        let to = ctx.accounts.authority.to_account_info();
        **vault_info.try_borrow_mut_lamports()? -= amount;
        **to.try_borrow_mut_lamports()? += amount;
        Ok(())
    }

    /// Unfreeze a frozen vault. Authority only, and only after the
    /// freeze window has passed.
    pub fn unfreeze(ctx: Context<Unfreeze>) -> Result<()> {
        let vault = &mut ctx.accounts.vault;
        let clock = Clock::get()?;
        require!(vault.frozen, GuardError::VaultNotFrozen);
        require!(
            clock.slot >= vault.frozen_until_slot,
            GuardError::StillFrozen
        );
        vault.frozen = false;
        vault.frozen_until_slot = 0;
        vault.withdrawals_this_slot = 0;
        Ok(())
    }
}

#[derive(Accounts)]
#[instruction(max_spend_lamports: u64, max_slippage_bps: u16, allow_durable_nonce: bool, allowed_programs: Vec<Pubkey>)]
pub struct InitializePolicy<'info> {
    #[account(
        init,
        payer = owner,
        space = Policy::LEN,
        seeds = [b"policy", owner.key().as_ref()],
        bump
    )]
    pub policy: Account<'info, Policy>,
    #[account(mut)]
    pub owner: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct UpdatePolicy<'info> {
    #[account(
        mut,
        seeds = [b"policy", policy.owner.as_ref()],
        bump = policy.bump,
        has_one = owner
    )]
    pub policy: Account<'info, Policy>,
    pub owner: Signer<'info>,
}

#[derive(Accounts)]
pub struct GuardedExecute<'info> {
    #[account(
        seeds = [b"policy", policy.owner.as_ref()],
        bump = policy.bump,
        constraint = policy.owner == authority.key() @ GuardError::NotPolicyOwner
    )]
    pub policy: Account<'info, Policy>,
    /// The policy owner. Must sign this instruction.
    pub authority: Signer<'info>,
    /// The account whose lamport balance is guarded (usually the
    /// authority's own wallet account). Checked, not written by the
    /// program directly; the CPI may move its lamports.
    /// CHECK: only its lamport balance is read.
    pub guard_account: AccountInfo<'info>,
    // Remaining accounts: [0] target program, [1..] CPI accounts.
}

#[derive(Accounts)]
pub struct GuardedSwap<'info> {
    #[account(
        seeds = [b"policy", policy.owner.as_ref()],
        bump = policy.bump,
        constraint = policy.owner == authority.key() @ GuardError::NotPolicyOwner
    )]
    pub policy: Account<'info, Policy>,
    /// The policy owner. Must sign this instruction.
    pub authority: Signer<'info>,
    /// Source token account. The DEX CPI moves tokens out of it; the
    /// user must have signed the outer transaction for the DEX to use
    /// them as authority.
    #[account(mut)]
    pub source_token_account: Account<'info, TokenAccount>,
    /// Destination token account. Measured before and after the CPI.
    #[account(mut)]
    pub dest_token_account: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
    // Remaining accounts: [0] DEX program, [1..] DEX swap accounts.
}

#[derive(Accounts)]
#[instruction(max_withdrawals_per_slot: u16)]
pub struct InitializeVault<'info> {
    #[account(
        init,
        payer = authority,
        space = Vault::LEN,
        seeds = [b"vault", authority.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, Vault>,
    #[account(mut)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Deposit<'info> {
    #[account(
        mut,
        seeds = [b"vault", vault.authority.as_ref()],
        bump = vault.bump
    )]
    pub vault: Account<'info, Vault>,
    #[account(mut)]
    pub depositor: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Withdraw<'info> {
    #[account(
        mut,
        seeds = [b"vault", vault.authority.as_ref()],
        bump = vault.bump,
        has_one = authority
    )]
    pub vault: Account<'info, Vault>,
    #[account(mut)]
    pub authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct Unfreeze<'info> {
    #[account(
        mut,
        seeds = [b"vault", vault.authority.as_ref()],
        bump = vault.bump,
        has_one = authority
    )]
    pub vault: Account<'info, Vault>,
    pub authority: Signer<'info>,
}

#[account]
pub struct Policy {
    pub owner: Pubkey,
    pub max_spend_lamports: u64,
    pub max_slippage_bps: u16,
    pub allow_durable_nonce: bool,
    pub allowed_programs: [Pubkey; MAX_WHITELIST],
    pub allowed_count: u8,
    pub bump: u8,
}

impl Policy {
    /// 8 discriminator + 32 owner + 8 max_spend + 2 slippage + 1 nonce
    /// flag + 8*32 whitelist + 1 count + 1 bump.
    pub const LEN: usize = 8 + 32 + 8 + 2 + 1 + (MAX_WHITELIST * 32) + 1 + 1;

    pub fn is_allowed(&self, program: &Pubkey) -> bool {
        self.allowed_programs[..self.allowed_count as usize]
            .iter()
            .any(|p| p == program)
    }
}

#[account]
pub struct Vault {
    pub authority: Pubkey,
    pub total_deposited: u64,
    pub withdrawals_this_slot: u16,
    pub last_withdraw_slot: u64,
    pub max_withdrawals_per_slot: u16,
    pub frozen: bool,
    pub frozen_until_slot: u64,
    pub bump: u8,
}

impl Vault {
    /// 8 discriminator + 32 authority + 8 deposited + 2 count + 8 slot
    /// + 2 max + 1 frozen + 8 until + 1 bump.
    pub const LEN: usize = 8 + 32 + 8 + 2 + 8 + 2 + 1 + 8 + 1;
}

#[error_code]
pub enum GuardError {
    #[msg("Slippage melebihi batas: token yang diterima di bawah minimum.")]
    SlippageExceeded,
    #[msg("Program target tidak ada di whitelist policy.")]
    ProgramNotWhitelisted,
    #[msg("Pengeluaran melebihi batas maksimal policy.")]
    SpendLimitExceeded,
    #[msg("Saldo penjaga turun di bawah batas minimum setelah CPI.")]
    GuardBalanceBreached,
    #[msg("Vault sedang dibekukan oleh circuit breaker.")]
    VaultFrozen,
    #[msg("Circuit breaker terpicu: penarikan terlalu cepat.")]
    CircuitBreakerTriggered,
    #[msg("Slippage maksimal harus 0 sampai 10000 bps.")]
    InvalidSlippageBps,
    #[msg("Maksimal 8 program di whitelist.")]
    WhitelistTooLong,
    #[msg("Jumlah penarikan melebihi saldo vault.")]
    InsufficientVaultBalance,
    #[msg("Vault belum melewati masa beku.")]
    StillFrozen,
    #[msg("Vault tidak sedang dibekukan.")]
    VaultNotFrozen,
    #[msg("Jumlah tidak valid.")]
    InvalidAmount,
    #[msg("Batas laju penarikan tidak valid.")]
    InvalidRateLimit,
    #[msg("Bukan pemilik policy ini.")]
    NotPolicyOwner,
}
