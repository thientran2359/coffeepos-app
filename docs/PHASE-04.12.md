# Phase 4.12 — First-run recovery

Phase 4.12 hoàn thiện recovery cho first-run provisioning trên Windows x64. Flow vẫn là một thao tác từ Desktop UI: fresh store đi qua database → WordPress → WooCommerce → CoffeePOS → machine health. Nếu app/process bị ngắt giữa setup, lần mở lại đọc `config/provisioning.json` và chỉ tiếp tục từ trạng thái mà native layer có thể chứng minh an toàn.

## Recovery contract

Mỗi provisioning side effect hoàn tất trước khi journal advance. Phase 4.12 diễn tập interruption ngay sau side effect nhưng trước journal commit tại các boundary:

- database accounts ready;
- managed WordPress site ready;
- WordPress installed + HTTP verified;
- WooCommerce provisioned;
- WooCommerce activated + baseline verified;
- CoffeePOS provisioned;
- CoffeePOS activated + fresh-process verifier pass;
- machine-health token bootstrapped + endpoint verified.

Retry luôn chạy lại ensure semantics trên cùng `data_root`. Database credentials, WordPress administrator credential và machine token không được regenerate khi protected value hiện hữu. Managed plugin copy/upgrade tiếp tục dùng owned staging + atomic rename; activation/lifecycle verifier được phép replay khi journal còn ở stage trước commit.

Test-only interruption seam nằm trong `Provisioner` và chỉ inject lỗi sau side effect, trước `persist_stage()`. Production build không expose control này.

## Partial WordPress install

`wp_install()` là boundary đặc biệt vì process có thể chết sau khi đã tạo một phần WordPress tables. Bootstrap đã từ chối destructive reinstall khi thấy `wp_*` tables nhưng chưa có installation hoàn chỉnh. Phase 4.12 bổ sung persistent `recovery_blocker` trong provisioning journal cho trường hợp này.

Khi bootstrap trả exit code partial-install (`6` hoặc `7`):

- native giữ nguyên database/site;
- journal không advance khỏi `site_ready`;
- `inspect()` trả `needs_repair`, `can_retry=false` và recovery text rõ ràng;
- normal `prepare()` từ chối bypass blocker;
- UI chỉ cho kiểm tra lại trạng thái, không hiện một nút Repair giả khi Phase 6.2 chưa tồn tại.

Field blocker là optional + serde default nên journal Phase 4.6–4.11 vẫn đọc được mà không cần tăng schema version.

## Acceptance

Ignored staged Windows E2E `staged_runtime_recovers_across_first_run_journal_boundaries` dùng runtime development thật. Test inject interruption qua toàn bộ journal boundaries, stop/drop `RuntimeManager` + `Provisioner`, recreate chúng trên cùng store rồi Retry. Final run phải đạt `MachineHealthBootstrapped`/`ready`, trong khi runtime DB credential, WordPress DB credential, admin secret và machine token giữ nguyên qua các lần relaunch.

Ignored staged E2E `staged_runtime_blocks_retry_for_partial_wordpress_tables` tạo một partial `wp_*` table sau `site_ready`, chạy bootstrap thật, xác nhận table vẫn còn, recovery blocker được persist, relaunch vẫn `can_retry=false`, và normal provisioning không xóa hay reinstall store.

## Validation

Các gate áp dụng cho Phase 4.12:

```powershell
. .\scripts\use-local-rust.ps1
cargo test --manifest-path .\src-tauri\Cargo.toml --locked
cargo fmt --manifest-path .\src-tauri\Cargo.toml --check
cargo clippy --manifest-path .\src-tauri\Cargo.toml --locked --all-targets -- -D warnings
npm run lint:ui
npm run build:ui
cargo test --manifest-path .\src-tauri\Cargo.toml --locked provisioning::tests::staged_runtime_recovers_across_first_run_journal_boundaries -- --ignored
cargo test --manifest-path .\src-tauri\Cargo.toml --locked provisioning::tests::staged_runtime_blocks_retry_for_partial_wordpress_tables -- --ignored
```

Final acceptance ngày 2026-09-18:

```text
Rust tests: 42 passed, 0 failed, 4 ignored
cargo fmt --check: PASS
cargo clippy --locked --all-targets: PASS with warnings denied
npm run lint:ui: PASS
npm run build:ui: PASS
First-run journal-boundary recovery E2E: 1 passed, 0 failed, 249.36s
Partial WordPress repair-blocker E2E: 1 passed, 0 failed, 98.97s
```

## Boundary sang Phase 5.1

Phase 4.12 kết thúc setup/recovery milestone. Phase 5.1 sở hữu khung giao diện + điều hướng và phải giữ recovery flow này hoạt động khi tách setup khỏi khu vực cửa hàng đã cài. POS open/login thuộc Phase 5.4; Phase 4.12 không mở rộng auto-start hay Repair engine của Phase 6.
