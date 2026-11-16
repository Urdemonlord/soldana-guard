# SolDana Guard: desain arsitektur

## Keputusan arsitektur

Guard adalah **opt-in smart-wallet proxy**, bukan firewall jaringan. Satu kalimat ini yang menentukan semua desain di bawah.

## Dua koreksi kejujuran

### 1. Tidak ada firewall atas transaksi sembarangan

Program Solana cuma bisa ngatur instruksi yang dikirim ke dia. Tidak ada cara buat program mencegat atau memveto transaksi yang tidak melibatkannya. Jadi klaim "program memvalidasi transaksi sebelum dieksekusi" hanya benar buat transaksi yang dirutekan lewat `guarded_execute` atau `guarded_swap`.

Konsekuensi desain:
- User harus pakai Guard client buat setiap transaksi sensitif. Client yang membangun transaksi, membaca Policy PDA, dan merutekan CPI lewat program.
- Drainer tidak akan pernah ngerute lewat Guard. Perlindungan ini buat user yang sadar dan opt-in, bukan buat semua orang di jaringan.
- Demo harus jujur soal ini. Yang didemokan adalah: user yang pakai Guard selamat dari tiga skenario, bukan "jaringan jadi aman".

### 2. Durable nonce ditegakkan client, bukan program

Program tidak punya akses ke blockhash transaksi. Tidak ada sysvar yang ngasih tahu apakah transaksi ini pakai nonce account. Jadi tidak ada revert on-chain `DurableNonceBlocked` buat transaksi yang tidak menyentuh program.

Desain yang benar:
- Policy PDA menyimpan `allow_durable_nonce: bool` sebagai source of truth on-chain. Siapa pun bisa baca policy user dari Explorer.
- Client (`client/guard.js`, `assertNoDurableNonce`) membaca policy sebelum membangun transaksi. Kalau policy melarang dan transaksi mau pakai nonce, client menolak sebelum tanda tangan.
- Bukti demo skenario 1: client menolak + Policy PDA di Explorer menunjukkan `allow_durable_nonce: false`. Bukan signature gagal, karena tidak ada transaksi yang dikirim.

## Komponen

### Policy PDA

Seeds `["policy", owner]`, 309 byte. Isi: owner, max_spend_lamports, max_slippage_bps, allow_durable_nonce, whitelist 8 program + count, bump. Whitelist fixed array biar ukuran akun konstan dan rent bisa diprediksi.

### guarded_execute

Proxy CPI generik, pola yang sama dipakai multisig (Squads). `remaining_accounts[0]` adalah target program (harus executable dan ada di whitelist), sisanya akun yang dibutuhkan CPI. Program mencatat lamport guard account sebelum dan sesudah `invoke`, revert kalau pengeluaran melewati `max_spend_lamports` atau sisa di bawah `min_lamports_after`.

Batasan: cek ini cuma ngukur lamport. Buat token, pakai `guarded_swap` yang ngukur saldo token account. Cek generik buat semua jenis aset dalam satu instruksi tidak praktis.

Catatan biaya: kalau user juga fee payer transaksi luar, fee (~5000 lamport) ikut ngurangin saldo guard account. Client harus memperhitungkannya di `min_lamports_after`.

### guarded_swap

Swap dengan minimum output yang ditegakkan on-chain. Program mengukur dest token account sebelum CPI, invoke DEX, reload akun, dan revert dengan `SlippageExceeded` kalau yang diterima di bawah `min_out`. Client menghitung `min_out` dari quote dan `max_slippage_bps` policy. Program menegakkan angkanya, bukan menghitung quote.

Demo pakai `mock_dex`, program kecil dengan rate yang bisa digeser otoritasnya. Itu simulasi harga bergerak karena front-running. Di produksi targetnya DEX beneran (Jupiter, Raydium). Mock DEX diberi label jelas sebagai fixture demo, bukan bagian dari klaim program unik.

### Vault + circuit breaker

Vault PDA seeds `["vault", authority]`, menyimpan SOL. `withdraw` menghitung penarikan per slot; kalau melebihi `max_withdrawals_per_slot`, vault membeku selama 150 slot (~60 detik) dan call gagal dengan `CircuitBreakerTriggered`. `unfreeze` cuma bisa dipanggil authority setelah masa beku lewat.

Ini vault demo milik program. Mekanismenya nyata, tapi ini bukan perlindungan buat protokol pihak ketiga. Protokol luar cuma kejaga kalau mereka opt-in dan mendelegasikan otoritas ke Guard, dan tidak ada yang begitu sekarang.

## Alur demo (video < 3 menit)

1. Masalah (15 detik): Drift US$270 juta, Loopscale.
2. Setup policy (20 detik): buat Policy PDA di devnet, tunjukin di Explorer.
3. Skenario 1 (30 detik): client tolak transaksi durable nonce, tunjukin policy.
4. Skenario 2 (40 detik): rate mock DEX digeser, guarded_swap revert SlippageExceeded, dana aman.
5. Skenario 3 (30 detik): penarikan cepat memicu CircuitBreakerTriggered, vault beku.
6. Penutup (20 detik): arsitektur + Program ID di Explorer.

## Model bisnis (opsi, belum diputuskan)

- Fee kecil per guarded swap (misal 0,1% dengan cap), ditegakkan program. Paling jujur buat cerita "bisa cuan".
- Freemium client: policy dasar gratis, monitoring dan alert berbayar.
- Yang TIDAK dijual: program tidak pernah menahan dana user dan tidak mengambil cut dari transfer biasa.

## Yang belum dikerjakan (buat jendela hackathon 27-29 Nov)

- Deploy `soldana_guard` + `mock_dex` ke devnet (wallet deployer sudah punya ~2,59 SOL devnet, cukup buat program ~300KB).
- E2E script devnet (replay 3 skenario, kumpulkan signature).
- Frontend demo (hubungkan wallet, atur policy, jalankan 3 skenario dengan sekali klik).
- Video demo + submission.
