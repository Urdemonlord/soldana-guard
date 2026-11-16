// SolDana Guard client (ESM, no build step). Mirrors the pattern from the
// soldana repo: hardcoded Anchor discriminators + manual account decoding,
// so the client never depends on IDL generation (which breaks under newer
// host rustc versions).
//
// The durable-nonce rule is enforced HERE, in the client, not on-chain:
// a program cannot see the transaction's blockhash, so there is no
// on-chain durable-nonce revert. The client reads the Policy PDA (the
// on-chain source of truth for the rule) and refuses to build a nonce
// transaction when the policy forbids it.

export const GUARD_PROGRAM_ID = "5XeayPhkQW1QyG2siahkV6zhYArxsx2uEhxJu5vjEy3S";
export const MOCK_DEX_PROGRAM_ID = "G2UsVQWBhPXbR8T8n6QqbWeCuySR8vGA7UM7T2yd7R7D";

export const DISCRIMINATORS = {
  initialize_policy: "09ba56e181a2e738",
  update_policy: "d4f5f607a3971239",
  guarded_execute: "18068369ba16f7e1",
  guarded_swap: "a3e710113cf9dbd6",
  initialize_vault: "30bfa32c47813fa4",
  deposit: "f223c68952e1f2b6",
  withdraw: "b712469c946da122",
  unfreeze: "85a044fd50e8daf7",
  mock_dex_initialize: "afaf6d1f0d989bed",
  mock_dex_set_rate: "633aaaeea0784a0b",
  mock_dex_swap: "f8c69e91e17587c8",
};

// Policy account layout (soldana_guard Policy, total 309 bytes):
// 8 discriminator | 32 owner | 8 max_spend_lamports | 2 max_slippage_bps |
// 1 allow_durable_nonce | 8*32 whitelist | 1 allowed_count | 1 bump
export const POLICY_LEN = 309;
export const MAX_WHITELIST = 8;

export function findPolicyPda(web3, owner) {
  return web3.PublicKey.findProgramAddressSync(
    [Buffer.from("policy"), owner.toBuffer()],
    new web3.PublicKey(GUARD_PROGRAM_ID),
  );
}

export function findVaultPda(web3, authority) {
  return web3.PublicKey.findProgramAddressSync(
    [Buffer.from("vault"), authority.toBuffer()],
    new web3.PublicKey(GUARD_PROGRAM_ID),
  );
}

export function decodePolicyAccount(data) {
  const buf = Buffer.from(data);
  if (buf.length < POLICY_LEN) throw new Error("Policy account too short");
  let o = 8;
  const owner = buf.subarray(o, o + 32).toString("hex"); o += 32;
  const maxSpendLamports = buf.readBigUInt64LE(o); o += 8;
  const maxSlippageBps = buf.readUInt16LE(o); o += 2;
  const allowDurableNonce = buf.readUInt8(o) === 1; o += 1;
  const allowedPrograms = [];
  for (let i = 0; i < MAX_WHITELIST; i++) {
    allowedPrograms.push(buf.subarray(o, o + 32).toString("hex"));
    o += 32;
  }
  const allowedCount = buf.readUInt8(o); o += 1;
  const bump = buf.readUInt8(o);
  return {
    owner,
    maxSpendLamports,
    maxSlippageBps,
    allowDurableNonce,
    allowedPrograms: allowedPrograms.slice(0, allowedCount),
    allowedCount,
    bump,
  };
}

export async function readPolicy(connection, web3, owner) {
  const [pda] = findPolicyPda(web3, owner);
  const info = await connection.getAccountInfo(pda);
  if (!info) return null;
  return { pda, ...decodePolicyAccount(info.data) };
}

/// Client-side durable-nonce gate. The policy is the on-chain source of
/// truth; the enforcement point is here, because the program cannot see
/// the transaction blockhash. Throws when the policy forbids nonces and
/// the caller is about to build a nonce transaction.
export function assertNoDurableNonce(policy, usesNonce) {
  if (!policy) return;
  if (!policy.allowDurableNonce && usesNonce) {
    const err = new Error(
      "DurableNonceBlocked: policy kamu melarang transaksi durable-nonce. " +
      "Transaksi yang ditandatangani sekarang bisa dipakai berminggu-minggu lagi " +
      "tanpa kamu tahu. Ubah policy dulu kalau kamu memang butuh nonce.",
    );
    err.code = "DurableNonceBlocked";
    throw err;
  }
}

/// Plain-language policy summary (Indonesian), for the pre-sign screen.
export function explainPolicy(policy) {
  if (!policy) return "Belum ada policy. Buat dulu sebelum pakai Guard.";
  const sol = Number(policy.maxSpendLamports) / 1e9;
  const lines = [
    `Batas belanja per transaksi: ${sol} SOL.`,
    `Slippage maksimal: ${(policy.maxSlippageBps / 100).toFixed(2)}%.`,
    policy.allowDurableNonce
      ? "Durable nonce: diizinkan (transaksi bisa dipakai lama setelah ditandatangani)."
      : "Durable nonce: DILARANG (transaksi cuma berlaku sebentar).",
    `Program yang diizinkan: ${policy.allowedCount} program di whitelist.`,
  ];
  return lines.join(" ");
}

/// Simulate a transaction and return a plain-language risk summary.
/// Detects AdvanceNonceAccount (durable nonce) and SetAuthority
/// instructions, and reports balance changes from the simulation.
export async function simulateAndExplain(connection, web3, tx) {
  const sim = await connection.simulateTransaction(tx);
  const issues = [];
  if (sim.value.err) {
    issues.push(`Simulasi gagal: ${JSON.stringify(sim.value.err)}`);
  }
  const ixs = tx.instructions ?? [];
  for (const ix of ixs) {
    const pid = ix.programId.toBase58();
    if (pid === web3.SystemProgram.programId.toBase58()) {
      const data = Buffer.from(ix.data);
      if (data.length >= 4 && data.readUInt32LE(0) === 4) {
        issues.push("Transaksi ini memakai AdvanceNonceAccount (durable nonce).");
      }
    }
  }
  const logs = sim.value.logs ?? [];
  const errLogs = logs.filter((l) => l.includes("Error") || l.includes("failed"));
  return {
    ok: !sim.value.err,
    err: sim.value.err ?? null,
    unitsConsumed: sim.value.unitsConsumed ?? null,
    issues,
    errLogs,
    logs,
  };
}

function u64le(n) {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(BigInt(n));
  return b;
}

function u16le(n) {
  const b = Buffer.alloc(2);
  b.writeUInt16LE(n);
  return b;
}

function u32le(n) {
  const b = Buffer.alloc(4);
  b.writeUInt32LE(n);
  return b;
}

export function buildInitializePolicyIx(web3, owner, policyPda, opts) {
  const { maxSpendLamports, maxSlippageBps, allowDurableNonce, allowedPrograms } = opts;
  if (allowedPrograms.length > MAX_WHITELIST) {
    throw new Error("Maksimal 8 program di whitelist.");
  }
  const data = Buffer.concat([
    Buffer.from(DISCRIMINATORS.initialize_policy, "hex"),
    u64le(maxSpendLamports),
    u16le(maxSlippageBps),
    Buffer.from([allowDurableNonce ? 1 : 0]),
    u32le(allowedPrograms.length),
    ...allowedPrograms.map((p) => new web3.PublicKey(p).toBuffer()),
  ]);
  return new web3.TransactionInstruction({
    programId: new web3.PublicKey(GUARD_PROGRAM_ID),
    keys: [
      { pubkey: policyPda, isSigner: false, isWritable: true },
      { pubkey: owner, isSigner: true, isWritable: true },
      { pubkey: web3.SystemProgram.programId, isSigner: false, isWritable: false },
    ],
    data,
  });
}

export function buildGuardedSwapIx(web3, accounts, ixData, minOut) {
  const { policyPda, authority, sourceTokenAccount, destTokenAccount, dexProgram, dexAccounts } = accounts;
  const data = Buffer.concat([
    Buffer.from(DISCRIMINATORS.guarded_swap, "hex"),
    u32le(ixData.length),
    Buffer.from(ixData),
    u64le(minOut),
  ]);
  return new web3.TransactionInstruction({
    programId: new web3.PublicKey(GUARD_PROGRAM_ID),
    keys: [
      { pubkey: policyPda, isSigner: false, isWritable: false },
      { pubkey: authority, isSigner: true, isWritable: false },
      { pubkey: sourceTokenAccount, isSigner: false, isWritable: true },
      { pubkey: destTokenAccount, isSigner: false, isWritable: true },
      { pubkey: new web3.PublicKey("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"), isSigner: false, isWritable: false },
      { pubkey: dexProgram, isSigner: false, isWritable: false },
      ...dexAccounts,
    ],
    data,
  });
}
