//! Mock DEX, demo fixture only. NOT part of the SolDana Guard submission.
//!
//! A tiny fixed-rate swap used to demonstrate `guarded_swap` on devnet:
//! the demo authority moves the rate (simulating price impact from a
//! front-running trade) and the guard program reverts the swap when the
//! output falls below the caller's minimum. In production the swap target
//! would be a real DEX (Jupiter, Raydium); the guard's min-out check
//! around an arbitrary CPI is the real mechanism, this program is only
//! the counterparty in the demo.

use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

declare_id!("G2UsVQWBhPXbR8T8n6QqbWeCuySR8vGA7UM7T2yd7R7D");

#[program]
pub mod mock_dex {
    use super::*;

    /// Create the DEX state PDA. `rate_bps` is the output per 10_000
    /// input units (10_000 = 1:1). The authority may change the rate.
    pub fn initialize(ctx: Context<Initialize>, rate_bps: u64) -> Result<()> {
        require!(rate_bps > 0, DexError::InvalidRate);
        let dex = &mut ctx.accounts.dex;
        dex.authority = ctx.accounts.authority.key();
        dex.rate_bps = rate_bps;
        dex.bump = ctx.bumps.dex;
        Ok(())
    }

    /// Move the rate. Demo only: this is how the demo simulates a price
    /// moving against the user (in production the rate moves because of
    /// real trades against the pool).
    pub fn set_rate(ctx: Context<SetRate>, rate_bps: u64) -> Result<()> {
        require!(rate_bps > 0, DexError::InvalidRate);
        ctx.accounts.dex.rate_bps = rate_bps;
        Ok(())
    }

    /// Swap `amount_in` of token A for token B at the current rate.
    /// Moves amount_in from the user's source account into the pool, and
    /// amount_out from the pool into the user's destination account.
    pub fn swap(ctx: Context<Swap>, amount_in: u64) -> Result<()> {
        require!(amount_in > 0, DexError::InvalidAmount);
        let dex = &ctx.accounts.dex;
        let amount_out = amount_in
            .checked_mul(dex.rate_bps)
            .ok_or(DexError::MathOverflow)?
            .checked_div(10_000)
            .ok_or(DexError::MathOverflow)?;
        require!(amount_out > 0, DexError::InvalidAmount);

        let cpi_accounts = Transfer {
            from: ctx.accounts.user_source.to_account_info(),
            to: ctx.accounts.pool_source.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
        };
        token::transfer(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                cpi_accounts,
            ),
            amount_in,
        )?;

        let seeds: &[&[u8]] = &[b"dex", dex.authority.as_ref(), &[dex.bump]];
        let cpi_accounts = Transfer {
            from: ctx.accounts.pool_dest.to_account_info(),
            to: ctx.accounts.user_dest.to_account_info(),
            authority: ctx.accounts.dex.to_account_info(),
        };
        token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                cpi_accounts,
                &[seeds],
            ),
            amount_out,
        )?;
        Ok(())
    }
}

#[derive(Accounts)]
#[instruction(rate_bps: u64)]
pub struct Initialize<'info> {
    #[account(
        init,
        payer = authority,
        space = DexState::LEN,
        seeds = [b"dex", authority.key().as_ref()],
        bump
    )]
    pub dex: Account<'info, DexState>,
    #[account(mut)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct SetRate<'info> {
    #[account(
        mut,
        seeds = [b"dex", dex.authority.as_ref()],
        bump = dex.bump,
        has_one = authority
    )]
    pub dex: Account<'info, DexState>,
    pub authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct Swap<'info> {
    #[account(
        seeds = [b"dex", dex.authority.as_ref()],
        bump = dex.bump
    )]
    pub dex: Account<'info, DexState>,
    /// Pool account holding token A. Receives the user's input.
    #[account(mut)]
    pub pool_source: Account<'info, TokenAccount>,
    /// Pool account holding token B. Pays the user's output.
    #[account(mut)]
    pub pool_dest: Account<'info, TokenAccount>,
    /// User's token A account.
    #[account(mut)]
    pub user_source: Account<'info, TokenAccount>,
    /// User's token B account.
    #[account(mut)]
    pub user_dest: Account<'info, TokenAccount>,
    pub user: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[account]
pub struct DexState {
    pub authority: Pubkey,
    pub rate_bps: u64,
    pub bump: u8,
}

impl DexState {
    pub const LEN: usize = 8 + 32 + 8 + 1;
}

#[error_code]
pub enum DexError {
    #[msg("Rate tidak valid.")]
    InvalidRate,
    #[msg("Jumlah tidak valid.")]
    InvalidAmount,
    #[msg("Overflow matematika.")]
    MathOverflow,
}
