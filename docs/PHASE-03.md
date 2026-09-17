# Phase 3 — WordPress Provisioning (Windows-first)

## Trạng thái

**Hoàn thành Windows-first ngày 2026-09-17.** Phase 3 kết thúc ở WordPress provisioning core. Provisioning UI, WooCommerce và CoffeePOS bắt đầu từ Phase 4.x; xem [ROADMAP.md](ROADMAP.md).

## Phạm vi đã hoàn thành

- Pin WordPress 7.1 từ archive chính thức và stage deterministic bằng `scripts/stage-wordpress-development.ps1`.
- Verify archive SHA256 `d1ae02b5ae18428031ffc3943659fa87ab361d827f4aa804adf9276e4dc75df6` và official SHA1 `b2b81d9242a122a8c7104a92387794eb64fcde97`.
- Native provisioning chỉ resolve manifest/core bên trong `runtime/development` và chặn path escape/schema sai.
- Initialize MariaDB datadir mới qua staged runtime, tạo database/account cần thiết và bảo vệ credential bằng Windows DPAPI.
- Copy WordPress core qua owned staging directory; sinh managed `wp-config.php`, router và uploads bridge.
- Start MariaDB/PHP bằng Runtime Manager, chạy WordPress bootstrap thật, tạo initial administrator và xác minh `/wp-login.php`.
- Journal provisioning, preserve existing managed store, từ chối ghi đè site/config/database không rõ ownership.
- Retry/idempotency: chạy provisioning lần hai giữ dữ liệu hiện có và không reset credential/store.

## Bằng chứng validation

| Kiểm tra | Kết quả |
| --- | --- |
| WordPress staging + SHA256/SHA1 | PASS |
| Rust unit tests | PASS: 19 passed, 0 failed; real-runtime tests chạy riêng |
| Clippy `-D warnings` + rustfmt + TypeScript lint | PASS |
| Tauri release no-bundle build | PASS |
| Real runtime E2E PHP 8.4.25 + MariaDB 11.4.13 + WordPress 7.1 | PASS |
| Fresh provision → start → WordPress install → stop | PASS |
| Provision lần hai giữ sentinel data | PASS |
| PID/port/staging/probe cleanup sau stop | PASS |
| CoffeePOS staged PHP/MariaDB process còn sót sau validation | PASS: không còn process |

Real-runtime E2E có thể chạy thủ công:

```powershell
. .\scripts\use-local-rust.ps1
cargo test --manifest-path .\src-tauri\Cargo.toml --locked provisioning::tests::staged_runtime_provisions_twice_stops_and_cleans_temp_store -- --ignored
```

## Ngoài phạm vi Phase 3

- UI để người dùng bấm Install: Phase 4.1.
- WordPress runtime UX và action mở site: Phase 4.2–4.3.
- WooCommerce: Phase 4.4–4.6.
- CoffeePOS: Phase 4.7–4.10.
- Full plugin-stack idempotency/recovery: Phase 4.11–4.12.
