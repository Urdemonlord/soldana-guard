# SolDana Guard

Opt-in on-chain transaction guard buat pengguna Solana. Dibangun buat Dev3pack Global Hackathon 2026.

**Program ID (devnet): `5XeayPhkQW1QyG2siahkV6zhYArxsx2uEhxJu5vjEy3S`**

Explorer (setelah deploy): https://explorer.solana.com/address/5XeayPhkQW1QyG2siahkV6zhYArxsx2uEhxJu5vjEy3S?cluster=devnet

## Masalahnya

Dua insiden terbesar Solana 2026 punya akar yang sama: transaksi yang ditandatangani user dipakai di luar yang dia kira.

- **Drift, 1 April 2026, minimal US$270 juta.** Penyerang pakai durable nonce: approval yang ditandatangani berminggu-minggu sebelumnya tetap valid, lalu dipakai buat nguras protokol. Solana sekarang ngajuin SIMD-0723 buat menghapus durable nonce.
- **Loopscale, 5 November 2026.** Vault SOL dan USDC diserang berulang selama seminggu, satu pinjaman 4.597 SOL lolos karena salah harga collateral.

Guard nggak bisa memperbaiki Drift atau Loopscale. Yang dia lakukan: ngasih user alat buat ngejaga transaksinya sendiri, dengan aturan yang ditegakin on-chain.

## Cara kerja

User bikin Policy PDA berisi aturannya sendiri: batas belanja per transaksi, slippage maksimal, whitelist program yang dipercaya, dan apakah durable nonce diizinkan. Terus semua transaksi sensitif dirutekan lewat program Guard:

1. `initialize_policy` / `update_policy`: bikin dan ubah Policy PDA.
2. `guarded_execute`: eksekusi CPI generik. Target program harus ada di whitelist, dan program ngecek saldo guard account sebelum dan sesudah CPI. Kalau pengeluaran melewati batas policy, transaksi di-revert.
3. `guarded_swap`: swap token lewat DEX dengan minimum output yang ditegakin on-chain. Kalau token yang diterima di bawah minimum (harga bergerak gara-gara sandwich), transaksi di-revert dengan `SlippageExceeded`.
4. Vault demo + circuit breaker: `initialize_vault`, `deposit`, `withdraw`, `unfreeze`. Penarikan di atas batas per slot otomatis membekukan vault (`CircuitBreakerTriggered`).

Durable nonce ditegakkan di client (`client/guard.js`), bukan di program. Program tidak bisa melihat blockhash transaksi, jadi tidak ada revert on-chain buat nonce. Client baca Policy PDA dan menolak membangun transaksi nonce kalau policy melarang. Lihat DESIGN.md buat penjelasan lengkapnya.

## Yang program ini TIDAK bisa

- Mencegat transaksi sembarangan. Program cuma bisa ngatur instruksi yang dikirim ke dia. User cuma kejaga selama dia pakai Guard client. Situs drainer tidak akan ngerute lewat Guard.
- Melindungi protokol pihak ketiga. Vault di sini adalah vault demo milik program ini, buat nunjukin mekanisme circuit breaker. Drift dan Loopscale tidak pakai Guard.

## Demo (3 skenario, devnet)

1. **Serangan durable nonce**: client coba bikin transaksi nonce saat policy melarang. Client menolak sebelum tanda tangan, dengan Policy PDA sebagai bukti.
2. **Sandwich swap**: rate mock DEX digeser sebelum swap user. `guarded_swap` revert dengan `SlippageExceeded`, dana tidak keluar.
3. **Vault drain cepat**: penarikan berulang dalam satu slot memicu `CircuitBreakerTriggered`, vault membeku.

## Yang dibangun sebelum hackathon vs selama hackathon

Aturan hackathon mengizinkan pre-build asal transparan. Pembagiannya:

**Dibangun sebelum hackathon (16-17 November 2026):**
- Kerangka proyek Anchor, `lib.rs` program `soldana_guard` + `mock_dex` (fixture demo), test Anchor, client `guard.js`, dokumen DESIGN.md dan README ini.

**Dibangun selama hackathon (27-29 November 2026):**
- Deploy program ke devnet, E2E script di devnet, frontend demo, video demo, submission.

## Menjalankan test

`anchor test` butuh local validator (UDP). Tidak bisa jalan di sandbox Muse. Jalankan di VPS atau mesin lokal:

```
anchor test
```

## Struktur

```
programs/soldana_guard/src/lib.rs  program utama (Policy, guarded_execute, guarded_swap, vault)
programs/mock_dex/src/lib.rs       fixture demo saja, bukan bagian submission
tests/soldana_guard.ts             test Anchor (policy, execute, swap, vault breaker)
client/guard.js                    client ESM: PDA, decode policy, durable-nonce gate, simulasi
DESIGN.md                          arsitektur + koreksi kejujuran
```

## Lisensi

MIT. Copyright (c) 2026 Hasrinata Arya Afendi.
