# Phase 4.10 — CoffeePOS health endpoint (Windows-first)

## Trạng thái

**Hoàn thành Windows-first ngày 2026-09-18.**

Phase 4.10 đưa application-level health về đúng owner: CoffeePOS plugin quyết định WordPress/WooCommerce/CoffeePOS/database readiness; Desktop chỉ authenticate, gọi endpoint, validate contract và hiển thị/phân loại kết quả.

## CoffeePOS 1.0.1 artifact

Machine-health cần sửa plugin nên Phase 4.10 không mutate artifact 1.0.0 của Phase 4.7. Release mới được build từ clean isolated plugin checkout:

| Thuộc tính | Giá trị |
| --- | --- |
| CoffeePOS | `1.0.1` |
| Source Git commit | `6de371a104aca6ae426ebea78a97460820dc4d65` |
| Checked-in archive | `scripts/coffeepos-development/coffeepos-1.0.1.zip` |
| SHA256 | `67e3f268ffd29946cfb4fdce13d6e7ad12caaf3ca7af007cff2177940d8e4a64` |
| Machine-health schema | `1` |
| CoffeePOS DB schema | `0.0.1` |

Endpoint không thêm bảng/column nên không bump `Migrator::SCHEMA_VERSION`. Native chỉ auto-upgrade exact managed Phase 4.9 artifact 1.0.0/hash đã pin sang exact 1.0.1/hash trên bằng owned staging + backup + atomic rename. Unmanaged, corrupt hoặc version/hash khác được preserve và từ chối.

## Endpoint contract

`GET /wp-json/coffeepos/v1/system/status` dùng `X-CoffeePOS-Machine-Token` và trả schema version 1 gồm `status`, four component booleans, runtime versions, `store.name` và Router-derived `pos_path`.

- thiếu/sai credential: HTTP 401;
- mọi required component ready: HTTP 200 + `status=healthy`;
- endpoint vẫn chạy nhưng một dependency/plugin invariant không ready: HTTP 503 + `status=degraded`;
- plugin inactive/missing route, malformed HTTP hoặc endpoint không reachable: Desktop `transport_bootstrap` failure;
- schema/JSON/status-component inconsistency/unsafe POS path: Desktop `contract` failure;
- protected token missing/unreadable hoặc server reject token: Desktop `authentication` failure.

CoffeePOS registers `/system/status` trước WooCommerce availability gate để WooCommerce missing vẫn là một verified degraded payload thay vì biến route mất theo dependency. Response không chứa token/hash, filesystem path, WordPress user/session hoặc customer data.

CoffeePOS readiness không phụ thuộc `coffeepos_installed_version === COFFEEPOS_VERSION`. Option đó được activation lifecycle ghi và có thể stale sau một normal file upgrade; live plugin code/version cùng plugin-owned DB/schema checks mới là bằng chứng health phù hợp. Phase 4.9 fresh activation verifier vẫn kiểm installed-version baseline riêng trong provisioning.

## Machine credential

Desktop tạo 32 random bytes, encode lowercase hex và lưu plaintext token bằng Windows DPAPI trong `config/machine-token.secret`. Token không đi qua WebView, query string, process argument hoặc log. Plugin chỉ lưu SHA-256 trong option `coffeepos_desktop_machine_token_sha256` và dùng `hash_equals` khi verify request.

Fresh bootstrap chạy sau CoffeePOS activation bằng bounded pinned-PHP CLI. Native đưa token qua stdin; script load exact managed WordPress/plugin, chỉ persist hash và giữ retry idempotent. Journal chỉ tiến tới `machine_health_bootstrapped` sau authenticated endpoint probe accepted (`healthy` hoặc verified `degraded`).

Restart/runtime retry không rotate hoặc tạo lại token. Nếu journal đã nói machine-health bootstrapped nhưng protected active token bị mất/unreadable, inspect chuyển `needs_repair` và normal provisioning không tự phát sinh credential mới.

Rotation dùng protected active + pending token. Server hash được đổi qua bounded pinned-PHP stdin; pending token phải probe thành công trước khi promote. Failure recovery xác định credential nào server đang chấp nhận, restore active hash khi cần và chỉ giữ cả hai file khi authority không thể xác định an toàn.

## Native health lifecycle

`RuntimeInfo` có `coffeepos_health` với state `unavailable/checking/healthy/degraded/failed`, optional payload, structured error và failure kind. Runtime chỉ probe CoffeePOS sau khi WordPress healthy, reprobe bounded theo interval khi running và clear stale health khi stop, child exit, failed start hoặc WordPress unhealthy.

Parser chỉ nhận HTTP/1.0 hoặc HTTP/1.1, status 200/503 cho payload, exact schema 1 và JSON shape strict kể cả nested `versions`/`store`. `pos_path` phải bắt đầu bằng một `/`, không có scheme/host, UNC/backslash hoặc control characters. Desktop không hard-code `/pos/` và không tự tính lại domain dependency health.

UI hiển thị state/failure kind, runtime versions, component readiness, store identity, schema và POS path từ verified payload; machine token không được expose cho TypeScript/WebView.

## Acceptance

Plugin behavioral checks cover route registration before Woo gate, missing/wrong/correct machine token, SHA-256 auth, 200 healthy, 503 degraded, runtime versions, store identity, custom Router path, stale activation-version tolerance và secret non-disclosure. Phase 13 release scenarios vẫn pass.

Native/unit acceptance cover healthy/degraded parsing, 401 auth, missing protected token, missing route/invalid protocol transport failure, malformed/incompatible/unknown-field contract payload và bounded timeout classification. Real staged Windows E2E covers fresh 1.0.1 provisioning, token bootstrap, missing/wrong/correct HTTP auth, credential rotation, plugin/dependency failure, runtime restart/process death/start retry/port move và second provisioning without token/data reset.

Validation cuối:

```text
Plugin system-status contract: PASS
Plugin system-status behavior: PASS
Plugin Phase 13 release scenarios: PASS
Rust tests: 41 passed, 0 failed, 2 ignored
cargo fmt --check: PASS
cargo clippy --locked --all-targets -- -D warnings: PASS
npm run lint:ui: PASS
npm run build:ui: PASS
Real staged Windows E2E: 1 passed, 0 failed, 239.20s
```

Final plugin-source commands (chạy trong isolated CoffeePOS `desktop-phase-4.10-artifact` checkout):

```powershell
php tests/Desktop/system_status_contract.php
php tests/Desktop/system_status_behavior.php
php tests/Phase13/release_scenarios.php
```

Final Desktop commands (chạy trong Desktop App root):

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\stage-coffeepos-development.ps1
. .\scripts\use-local-rust.ps1
cargo test --manifest-path .\src-tauri\Cargo.toml --locked
cargo fmt --manifest-path .\src-tauri\Cargo.toml --check
cargo clippy --manifest-path .\src-tauri\Cargo.toml --locked --all-targets -- -D warnings
npm run lint:ui
npm run build:ui
cargo test --manifest-path .\src-tauri\Cargo.toml --locked provisioning::tests::staged_runtime_provisions_twice_stops_and_cleans_temp_store -- --ignored
```

## Gate sang Phase 4.11

Phase 4.11 có thể dùng `provisioning.ready` cho installation baseline và `runtime.coffeepos_health` cho live application state. Idempotency acceptance phải giữ nguyên active machine credential, WordPress/database/uploads/plugin state và business sentinels qua full install retry; không dùng retry để rotate credential hoặc che health failure.
