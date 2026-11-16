import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import {
  createMint,
  createAccount,
  mintTo,
  getAccount,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { PublicKey, SystemProgram, LAMPORTS_PER_SOL } from "@solana/web3.js";
import { assert } from "chai";
import { SoldanaGuard } from "../target/types/soldana_guard";
import { MockDex } from "../target/types/mock_dex";

// NOTE: `anchor test` needs a local validator (UDP). It cannot run in the
// Muse sandbox (UDP blocked). Run it on the VPS or a local machine:
//   anchor test
// The devnet E2E script (scripts/e2e-devnet.mjs, to be written during the
// hackathon window) replays the same scenarios against the deployed program.

describe("soldana_guard", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);
  const program = anchor.workspace.SoldanaGuard as Program<SoldanaGuard>;
  const dexProgram = anchor.workspace.MockDex as Program<MockDex>;
  const connection = provider.connection;
  const wallet = provider.wallet as anchor.Wallet;

  const [policyPda] = PublicKey.findProgramAddressSync(
    [Buffer.from("policy"), wallet.publicKey.toBuffer()],
    program.programId,
  );
  const [vaultPda] = PublicKey.findProgramAddressSync(
    [Buffer.from("vault"), wallet.publicKey.toBuffer()],
    program.programId,
  );
  const [dexPda] = PublicKey.findProgramAddressSync(
    [Buffer.from("dex"), wallet.publicKey.toBuffer()],
    dexProgram.programId,
  );

  const MAX_SPEND = 0.5 * LAMPORTS_PER_SOL;

  it("initialize_policy stores the policy", async () => {
    await program.methods
      .initializePolicy(
        new anchor.BN(MAX_SPEND),
        500,
        false,
        [SystemProgram.programId, dexProgram.programId, TOKEN_PROGRAM_ID],
      )
      .accounts({ policy: policyPda, owner: wallet.publicKey })
      .rpc();
    const policy = await program.account.policy.fetch(policyPda);
    assert.equal(policy.owner.toBase58(), wallet.publicKey.toBase58());
    assert.equal(policy.maxSpendLamports.toNumber(), MAX_SPEND);
    assert.equal(policy.maxSlippageBps, 500);
    assert.equal(policy.allowDurableNonce, false);
    assert.equal(policy.allowedCount, 3);
  });

  it("update_policy changes max spend", async () => {
    await program.methods
      .updatePolicy(
        new anchor.BN(1 * LAMPORTS_PER_SOL),
        500,
        false,
        [SystemProgram.programId, dexProgram.programId, TOKEN_PROGRAM_ID],
      )
      .accounts({ policy: policyPda, owner: wallet.publicKey })
      .rpc();
    const policy = await program.account.policy.fetch(policyPda);
    assert.equal(policy.maxSpendLamports.toNumber(), 1 * LAMPORTS_PER_SOL);
  });

  it("guarded_execute allows a whitelisted transfer within the limit", async () => {
    const recipient = anchor.web3.Keypair.generate();
    const before = await connection.getBalance(recipient.publicKey);
    const ix = SystemProgram.transfer({
      fromPubkey: wallet.publicKey,
      toPubkey: recipient.publicKey,
      lamports: 0.01 * LAMPORTS_PER_SOL,
    });
    await program.methods
      .guardedExecute(
        Buffer.from(ix.data),
        new anchor.BN(0),
      )
      .accounts({
        policy: policyPda,
        authority: wallet.publicKey,
        guardAccount: wallet.publicKey,
      })
      .remainingAccounts([
        { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
        { pubkey: wallet.publicKey, isSigner: true, isWritable: true },
        { pubkey: recipient.publicKey, isSigner: false, isWritable: true },
      ])
      .rpc();
    const after = await connection.getBalance(recipient.publicKey);
    assert.equal(after - before, 0.01 * LAMPORTS_PER_SOL);
  });

  it("guarded_execute rejects a program outside the whitelist", async () => {
    const recipient = anchor.web3.Keypair.generate();
    const ix = SystemProgram.transfer({
      fromPubkey: wallet.publicKey,
      toPubkey: recipient.publicKey,
      lamports: 1000,
    });
    // Memo program is NOT whitelisted, so the call must fail before CPI.
    const MEMO_PROGRAM_ID = new PublicKey(
      "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr",
    );
    try {
      await program.methods
        .guardedExecute(Buffer.from(ix.data), new anchor.BN(0))
        .accounts({
          policy: policyPda,
          authority: wallet.publicKey,
          guardAccount: wallet.publicKey,
        })
        .remainingAccounts([
          { pubkey: MEMO_PROGRAM_ID, isSigner: false, isWritable: false },
        ])
        .rpc();
      assert.fail("expected ProgramNotWhitelisted");
    } catch (e: any) {
      assert.include(e.toString(), "ProgramNotWhitelisted");
    }
  });

  it("guarded_execute rejects spending above the policy limit", async () => {
    const recipient = anchor.web3.Keypair.generate();
    const ix = SystemProgram.transfer({
      fromPubkey: wallet.publicKey,
      toPubkey: recipient.publicKey,
      lamports: 2 * LAMPORTS_PER_SOL,
    });
    try {
      await program.methods
        .guardedExecute(Buffer.from(ix.data), new anchor.BN(0))
        .accounts({
          policy: policyPda,
          authority: wallet.publicKey,
          guardAccount: wallet.publicKey,
        })
        .remainingAccounts([
          { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
          { pubkey: wallet.publicKey, isSigner: true, isWritable: true },
          { pubkey: recipient.publicKey, isSigner: false, isWritable: true },
        ])
        .rpc();
      assert.fail("expected SpendLimitExceeded");
    } catch (e: any) {
      assert.include(e.toString(), "SpendLimitExceeded");
    }
  });

  describe("guarded_swap with mock_dex", () => {
    let mintA: PublicKey;
    let mintB: PublicKey;
    let poolA: PublicKey;
    let poolB: PublicKey;
    let userA: PublicKey;
    let userB: PublicKey;

    before(async () => {
      mintA = await createMint(
        connection,
        wallet.payer,
        wallet.publicKey,
        null,
        6,
      );
      mintB = await createMint(
        connection,
        wallet.payer,
        wallet.publicKey,
        null,
        6,
      );
      poolA = await createAccount(connection, wallet.payer, mintA, dexPda, undefined);
      poolB = await createAccount(connection, wallet.payer, mintB, dexPda, undefined);
      userA = await createAccount(connection, wallet.payer, mintA, wallet.publicKey);
      userB = await createAccount(connection, wallet.payer, mintB, wallet.publicKey);
      await mintTo(connection, wallet.payer, mintA, userA, wallet.publicKey, 1_000_000);
      await mintTo(connection, wallet.payer, mintB, poolB, wallet.publicKey, 10_000_000);
      await dexProgram.methods
        .initialize(new anchor.BN(10_000))
        .accounts({ dex: dexPda, authority: wallet.publicKey })
        .rpc();
    });

    it("swap at the fair rate succeeds", async () => {
      const before = (await getAccount(connection, userB)).amount;
      const ix = await dexProgram.methods
        .swap(new anchor.BN(100_000))
        .accounts({
          dex: dexPda,
          poolSource: poolA,
          poolDest: poolB,
          userSource: userA,
          userDest: userB,
          user: wallet.publicKey,
        })
        .instruction();
      await program.methods
        .guardedSwap(Buffer.from(ix.data), new anchor.BN(100_000))
        .accounts({
          policy: policyPda,
          authority: wallet.publicKey,
          sourceTokenAccount: userA,
          destTokenAccount: userB,
        })
        .remainingAccounts([
          { pubkey: dexProgram.programId, isSigner: false, isWritable: false },
          { pubkey: dexPda, isSigner: false, isWritable: false },
          { pubkey: poolA, isSigner: false, isWritable: true },
          { pubkey: poolB, isSigner: false, isWritable: true },
          { pubkey: userA, isSigner: false, isWritable: true },
          { pubkey: userB, isSigner: false, isWritable: true },
          { pubkey: wallet.publicKey, isSigner: true, isWritable: false },
        ])
        .rpc();
      const after = (await getAccount(connection, userB)).amount;
      assert.equal(Number(after - before), 100_000);
    });

    it("swap after a rate move reverts with SlippageExceeded", async () => {
      // Simulate price impact: the rate halves before the user's swap.
      await dexProgram.methods
        .setRate(new anchor.BN(5_000))
        .accounts({ dex: dexPda, authority: wallet.publicKey })
        .rpc();
      const ix = await dexProgram.methods
        .swap(new anchor.BN(100_000))
        .accounts({
          dex: dexPda,
          poolSource: poolA,
          poolDest: poolB,
          userSource: userA,
          userDest: userB,
          user: wallet.publicKey,
        })
        .instruction();
      try {
        await program.methods
          .guardedSwap(Buffer.from(ix.data), new anchor.BN(100_000))
          .accounts({
            policy: policyPda,
            authority: wallet.publicKey,
            sourceTokenAccount: userA,
            destTokenAccount: userB,
          })
          .remainingAccounts([
            { pubkey: dexProgram.programId, isSigner: false, isWritable: false },
            { pubkey: dexPda, isSigner: false, isWritable: false },
            { pubkey: poolA, isSigner: false, isWritable: true },
            { pubkey: poolB, isSigner: false, isWritable: true },
            { pubkey: userA, isSigner: false, isWritable: true },
            { pubkey: userB, isSigner: false, isWritable: true },
            { pubkey: wallet.publicKey, isSigner: true, isWritable: false },
          ])
          .rpc();
        assert.fail("expected SlippageExceeded");
      } catch (e: any) {
        assert.include(e.toString(), "SlippageExceeded");
      }
    });
  });

  describe("vault circuit breaker", () => {
    it("deposit and slow withdrawals work", async () => {
      await program.methods
        .initializeVault(3)
        .accounts({ vault: vaultPda, authority: wallet.publicKey })
        .rpc();
      await program.methods
        .deposit(new anchor.BN(1 * LAMPORTS_PER_SOL))
        .accounts({ vault: vaultPda, depositor: wallet.publicKey })
        .rpc();
      await program.methods
        .withdraw(new anchor.BN(0.01 * LAMPORTS_PER_SOL))
        .accounts({ vault: vaultPda, authority: wallet.publicKey })
        .rpc();
      const vault = await program.account.vault.fetch(vaultPda);
      assert.equal(vault.frozen, false);
    });

    it("rapid withdrawals trip the circuit breaker", async () => {
      // 3 withdrawals already used in this slot budget; hammer the vault
      // with more in the same slot window until the breaker trips.
      let tripped = false;
      for (let i = 0; i < 6 && !tripped; i++) {
        try {
          await program.methods
            .withdraw(new anchor.BN(0.01 * LAMPORTS_PER_SOL))
            .accounts({ vault: vaultPda, authority: wallet.publicKey })
            .rpc();
        } catch (e: any) {
          if (e.toString().includes("CircuitBreakerTriggered")) tripped = true;
          else throw e;
        }
      }
      assert.equal(tripped, true, "breaker should trip on rapid withdrawals");
      const vault = await program.account.vault.fetch(vaultPda);
      assert.equal(vault.frozen, true);
      try {
        await program.methods
          .withdraw(new anchor.BN(0.01 * LAMPORTS_PER_SOL))
          .accounts({ vault: vaultPda, authority: wallet.publicKey })
          .rpc();
        assert.fail("expected VaultFrozen");
      } catch (e: any) {
        assert.include(e.toString(), "VaultFrozen");
      }
    });
  });
});
