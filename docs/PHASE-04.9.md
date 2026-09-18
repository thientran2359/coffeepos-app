# Phase 4.9 — CoffeePOS activation (Windows-first)

## Trạng thái

**Hoàn thành Windows-first ngày 2026-09-18.**

Phase 4.9 activate exact managed CoffeePOS 1.0.0 artifact đã provision ở Phase 4.8. Ready chỉ được ghi sau khi một WordPress process mới xác nhận plugin load bình thường và plugin-owned activation baseline thực sự tồn tại.

## Activation contract

Phase 4.9 tiếp tục dùng command `provision_wordpress` và lifecycle lock hiện có; không thêm activation button hoặc orchestration path riêng. Sau journal `coffee_pos_provisioned`:

1. native xác minh managed CoffeePOS tree vẫn đúng ownership/version và exact WooCommerce 11.1.0 đã được Phase 4.6 provision + activate;
2. bounded pinned-PHP activation process load managed WordPress, xác minh WooCommerce entry/header/runtime đúng 11.1.0 và CoffeePOS entry/header đúng 1.0.0;
3. nếu CoffeePOS chưa active, gọi WordPress `activate_plugin('coffeepos/coffeepos.php')` để chính `register_activation_hook` của plugin chạy `Lifecycle::activate()`;
4. nếu retry từ `coffee_pos_provisioned` nhưng WordPress đã ghi CoffeePOS active, gọi lại `CoffeePOS\Core\Lifecycle::activate()` để plugin tự đảm bảo settings/capabilities/migrations/rewrite sau partial attempt;
5. bounded pinned-PHP verification process thứ hai load WordPress lại từ đầu; chỉ process mới này được dùng để chứng minh normal plugin bootstrap/hook wiring;
6. chỉ khi verifier pass mới persist `coffee_pos_activated` và trả `ProvisioningInfo.ready + coffeepos_active=true`.

CoffeePOS 1.0.0 `Lifecycle` tự sở hữu `Settings::ensureDefaults()`, `Capabilities::register()`, `Migrator::migrate()`, Router rewrite registration/flush, `coffeepos_rewrite_version` và `coffeepos_installed_version`. Desktop không chạy SQL tạo CoffeePOS table, không seed business defaults và không tự cấp CoffeePOS capability.

## Dependency preflight

Pinned CoffeePOS 1.0.0 chỉ tự enforce PHP >= 7.4 ở activation hook; `Environment::minimumWooCommerceVersion()` của artifact đang rỗng và WooCommerce availability được `Bootstrap` kiểm sau activation. Vì Desktop baseline đã pin WooCommerce 11.1.0, Phase 4.9 explicit preflight cả trước activation và trong fresh verifier:

- `woocommerce/woocommerce.php` tồn tại và active;
- plugin header version đúng 11.1.0;
- fresh WordPress process load class `WooCommerce` và `WC_VERSION` đúng 11.1.0.

Mismatch/non-active dependency trả bounded actionable error trong `logs/coffeepos.log` và journal không tiến qua `coffee_pos_provisioned`.

## Fresh-process plugin baseline

Verifier không gọi `Bootstrap` hoặc `RouteRegistrar` để tự làm cho test pass. Nó load `wp-load.php` trong process mới rồi đọc state do plugin đã đăng ký bình thường.

Các invariant bắt buộc:

- `active_plugins` chứa `coffeepos/coffeepos.php`, header/runtime version đúng 1.0.0 và required runtime classes autoload được;
- `coffeepos_installed_version` bằng `COFFEEPOS_VERSION`;
- `coffeepos_db_version` bằng `CoffeePOS\Infrastructure\Database\Migrator::SCHEMA_VERSION`;
- mọi table do `CoffeePOS\Infrastructure\Database\Schema::tableNames($wpdb->prefix)` công bố tồn tại vật lý;
- mọi option do `CoffeePOS\Infrastructure\Settings\Settings::optionNames()` công bố đã tồn tại;
- các role `coffeepos_cashier`, `coffeepos_kitchen`, `coffeepos_supervisor`, `coffeepos_manager` tồn tại với `read` + CoffeePOS capability; `administrator` và `shop_manager` có toàn bộ `Capabilities::all()`;
- `coffeepos_rewrite_version` bằng `Router::rewriteVersion()` và persisted `rewrite_rules` có CoffeePOS entry route;
- REST server thật sau `rest_api_init` chứa `/coffeepos/v1/health` và mọi route trong `RouteRegistrar::registeredRoutes()`.

Verifier lấy schema/table/settings/capability/rewrite/route contract từ classes của exact staged plugin; Rust không hard-code schema/table/default list. Pinned 1.0.0 snapshot hiện có Migrator schema `0.0.1` và rewrite version `1.0.0:3`.

## Retry và journal

Stage mới là `coffee_pos_activated`; field `coffeepos_version` vẫn là 1.0.0. Phase 4.8 journal ở `coffee_pos_provisioned` deserialize bình thường nhưng inspect thành `needs_repair + can_retry + coffeepos_active=false` cho tới khi activation verifier pass.

Nếu activation/verifier timeout hoặc exit nonzero:

- WordPress/database/plugin files được preserve;
- journal vẫn ở `coffee_pos_provisioned`;
- retry reuse exact managed plugin và credential;
- nếu WordPress đã ghi plugin active trong partial attempt, retry replay `Lifecycle::activate()` rồi chạy fresh verifier;
- không delete/reinstall CoffeePOS để che lỗi lifecycle.

Runtime restart không sửa journal hoặc `active_plugins`. Fresh E2E xác nhận CoffeePOS vẫn active sau restart và second provisioning vẫn idempotent, giữ WordPress/WooCommerce/CoffeePOS/unrelated-plugin sentinels.

## UI

`ProvisioningInfo` thêm `coffeepos_active`. Shell Phase 4.9 hiển thị CoffeePOS 1.0.0 `active/not active`; Ready wording chỉ xuất hiện sau journal `coffee_pos_activated`. Existing install/retry button vẫn gọi `provision_wordpress`, nên native lifecycle/provisioning locks tiếp tục chặn action xung đột.

## Validation

Static/unit:

    Rust tests: 34 passed, 0 failed, 2 ignored
    cargo fmt --check: PASS
    cargo clippy --locked --all-targets -- -D warnings: PASS
    npm run lint:ui: PASS
    npm run build:ui: PASS

Real staged Windows E2E:

    . .\scripts\use-local-rust.ps1
    cargo test --manifest-path .\src-tauri\Cargo.toml --locked provisioning::tests::staged_runtime_provisions_twice_stops_and_cleans_temp_store -- --ignored

Final run sau khi siết full plugin-declared REST route baseline:

    1 passed, 0 failed
    223.85s

Acceptance chứng minh fresh WordPress + WooCommerce + CoffeePOS activation; exact active versions; plugin schema/settings/capability/rewrite/REST baseline; legacy CoffeePOS `/health` route tồn tại và từ chối unauthenticated request bằng plugin-owned permission error; WordPress health + Woo Store API usable; CoffeePOS active qua runtime restart; process-death/startup-failure/occupied-port recovery cũ vẫn pass; second provisioning vẫn Ready và giữ toàn bộ sentinels.

## Ngoài phạm vi Phase 4.9

- `/wp-json/coffeepos/v1/system/status` machine-health endpoint, machine token bootstrap và native consumption: Phase 4.10.
- POS application URL/usability và Desktop POS WebView: Phase 5.1–5.2.
- Full-stack install interruption/recovery acceptance: Phase 4.11–4.12.
- CoffeePOS upgrade/adoption flow cho existing unmanaged/different version plugin.

## Gate sang Phase 4.10

Phase 4.10 phải build trên `coffee_pos_activated` baseline và không dùng journal/file presence thay application health. Nếu implementation machine-health sửa CoffeePOS code so với exact 1.0.0 artifact đã pin, phải bump plugin version, rebuild/repin archive hash và chạy lại acceptance Phase 4.8–4.10.
