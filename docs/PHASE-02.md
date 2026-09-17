# Phase 2 — Runtime Manager (Windows-first)

## Phạm vi đã chốt

Phase 2 chỉ quản lý runtime PHP + MariaDB trên Windows x64. WordPress provisioning và database/account ứng dụng đã được hoàn thành ở Phase 3; WooCommerce và CoffeePOS thuộc Phase 4.x.

Runtime production không lấy executable từ `PATH`, LocalWP hoặc cài đặt hệ thống. Development runtime được stage vào `runtime/development/x86_64-pc-windows-msvc/` và bị Git ignore; manifest/template/script staging nằm ngoài thư mục ignored.

## Runtime đã pin

- PHP 8.4.25 NTS VS17 x64, SHA256 `43a8f67ed2e5223fafb21293c85976361808855405278cef2cf3037c3ae2529c`.
- MariaDB 11.4.13 x64 ZIP, SHA256 `d62986d433eeebfde218560b276103831604a61e929e87f1a17f5aebd80257e2`.
- `scripts/stage-runtime-development.ps1` tải artifact exact-version từ nguồn chính thức, verify SHA256 trước extract và stage deterministic.
- PHP bundle đã kiểm `mysqli`, `pdo_mysql`, `curl`, `openssl`, `mbstring`, `intl`, `zip`, `gd`, `fileinfo`, DOM/XML.

## Runtime manager

`src-tauri/src/runtime.rs` quản lý state `not_installed / stopped / starting / running / stopping`, chọn port loopback khả dụng, spawn process bằng absolute executable path, capture log và kiểm readiness trước khi báo `running`.

MariaDB dùng explicit `--no-defaults`, `basedir`, `datadir`, port và `127.0.0.1`; readiness dùng `mariadb.exe` authenticated `SELECT 1`. PHP dùng bundled `php.ini`, built-in server loopback và probe nonce riêng cho từng start attempt. Start thất bại dọn child đã tạo; Stop dừng PHP trước rồi gửi `SHUTDOWN` cho MariaDB và fallback terminate khi cần.

Trên Windows, child process được gắn vào Job Object với `KILL_ON_JOB_CLOSE` và chạy `CREATE_NO_WINDOW`, nên khi owner process mất thì runtime child không được giữ lại ngoài ý muốn.

Phase 2 không tự initialize datadir hoặc invent database credentials. Nếu `database/mysql` hoặc `runtime-client.cnf` chưa có, state là `not_installed` và runtime yêu cầu Phase 3 provisioning.

## Bằng chứng Windows 2026-09-17

| Kiểm tra | Kết quả |
| --- | --- |
| Stage exact PHP/MariaDB + verify SHA256 | PASS |
| PHP/MariaDB version + required PHP modules | PASS |
| PHP fixture HTTP readiness | PASS |
| Rust tests | PASS: 14 tests thường; real-runtime smoke chạy riêng |
| Real staged runtime smoke | PASS: start → stop → restart → restart while running → stop |
| Authenticated MariaDB SQL readiness | PASS trong smoke test |
| Instance-specific PHP HTTP readiness | PASS trong smoke test |
| `npm run lint` | PASS: TypeScript, rustfmt, Clippy `-D warnings` |
| `npm run build` | PASS; tạo native release executable |
| Orphan staged PHP/MariaDB sau smoke/cleanup | PASS: không còn process theo staged executable path |

Trong smoke test, datadir MariaDB disposable nằm dưới `.tools/` và chỉ phục vụ validation. Dữ liệu app/local store thật không bị provision hoặc sửa để vượt Phase 3.

Hai lỗi Windows được phát hiện bằng runtime thật và đã sửa: MariaDB client probe cần timeout có giới hạn riêng (`--connect-timeout=1`, bounded command timeout 3s), và Windows PHP không xử lý ổn đường dẫn verbatim `\\?\...`; resolver vẫn canonicalize để kiểm path containment nhưng chuyển executable/config path về dạng Windows command-compatible khi spawn.

## Gate sau Phase 2

Phase 2 Windows-first được coi là hoàn tất cho development runtime. Phase 3 sau đó đã hoàn thành provisioning idempotent cho datadir/database/site và giữ nguyên dữ liệu store hiện có khi retry. Release installer vẫn chưa nhúng runtime resources; runtime bundling/installer thuộc Phase 9.x.
