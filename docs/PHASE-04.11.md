# Phase 4.11 — Full install idempotency (Windows-first)

## Trạng thái

**Hoàn thành Windows-first ngày 2026-09-18.**

Phase 4.11 kiểm chứng toàn bộ provisioning path hiện có có thể chạy lại trên một store đã hoàn chỉnh mà không reset dữ liệu hoặc credential. Phase này không thêm artifact hay một installer path mới; nó siết invariant của store đã cài và mở rộng real staged E2E thành acceptance cho toàn stack DB → WordPress → WooCommerce → CoffeePOS → machine health.

## Contract idempotency

Lần provisioning thứ hai trên một store `ready` phải giữ nguyên:

- database datadir và dữ liệu nghiệp vụ đã có;
- protected runtime database credential;
- protected WordPress database credential;
- protected WordPress administrator credential và password hash của user `coffeepos_admin`;
- managed `wp-config.php`, bao gồm salts đã sinh từ lần cài đầu;
- external `uploads/` data;
- WooCommerce/CoffeePOS managed ownership metadata và plugin activation state;
- unrelated site/plugin files;
- active CoffeePOS machine token và server-side machine-health authority;
- provisioning journal ở completed stage.

`ProvisioningInfo.ready` tiếp tục chỉ là installation baseline. Live WordPress/CoffeePOS health vẫn do Runtime Manager và authenticated `/wp-json/coffeepos/v1/system/status` quyết định.

## Credential integrity fix

Audit Phase 4.11 phát hiện một trường hợp store hoàn chỉnh có thể báo `ready` dù `config/wordpress-admin.secret` đã mất. Nếu ép chạy provisioning lại, code cũ có thể tạo protected admin secret mới trong khi WordPress giữ password hash của user hiện hữu, làm native credential lệch khỏi credential thật.

Native provisioning hiện yêu cầu các protected runtime/WordPress database credentials và WordPress administrator credential phải đọc được trước khi một completed store được coi là `ready`. Khi journal đã tới `wordpress_installed`, provisioning chỉ load admin secret hiện hữu; missing/unreadable admin secret trả `needs_repair`, `can_retry=false` và không tự tạo replacement. Machine-token behavior của Phase 4.10 giữ nguyên: normal provisioning không rotate hoặc regenerate authority của completed store.

## Acceptance

Real staged E2E `staged_runtime_provisions_twice_stops_and_cleans_temp_store` dùng runtime development thật và chạy hai lần full provisioning trên cùng store tạm. Trước lần thứ hai test tạo/snapshot:

- một file trong external `uploads/`;
- site, WooCommerce, CoffeePOS và unrelated-plugin sentinels;
- option `coffeepos_store_name` với giá trị test;
- một row thật trong `wp_coffeepos_suspended_carts`;
- decrypted DB runtime/WordPress/admin/machine credentials;
- `wp_users.user_pass` của `coffeepos_admin`;
- managed `wp-config.php`;
- WooCommerce/CoffeePOS `.coffeepos-managed.json`;
- WordPress `active_plugins` và provisioning journal.

Sau full rerun, toàn bộ snapshot phải giữ nguyên, suspended-cart sentinel vẫn đúng một row, provisioning vẫn `ready`, WooCommerce/CoffeePOS vẫn active và machine-health credential vẫn dùng được. Unit regression riêng xác nhận installed WordPress không tự sinh lại admin secret khi file đã mất.

## Validation

Các gate áp dụng cho Phase 4.11:

```powershell
. .\scripts\use-local-rust.ps1
cargo test --manifest-path .\src-tauri\Cargo.toml --locked
cargo fmt --manifest-path .\src-tauri\Cargo.toml --check
cargo clippy --manifest-path .\src-tauri\Cargo.toml --locked --all-targets -- -D warnings
npm run lint:ui
npm run build:ui
cargo test --manifest-path .\src-tauri\Cargo.toml --locked staged_runtime_provisions_twice -- --ignored
```

Final acceptance ngày 2026-09-18:

```text
Rust tests: 42 passed, 0 failed, 2 ignored
npm run lint (TypeScript + cargo fmt --check + Clippy -D warnings): PASS
npm run build:ui: PASS
Real staged Windows full-install idempotency E2E: 1 passed, 0 failed, 222.39s
```

## Boundary sang Phase 4.12

Phase 4.11 chứng minh rerun của một installation hoàn chỉnh. Crash/failure injection giữa các commit boundary thuộc Phase 4.12. Đặc biệt, interruption ngay trong `wp_install()` có thể để partial WordPress tables; current bootstrap preserve và từ chối destructive reinstall. Phase 4.12 phải diễn tập boundary này cùng database/WooCommerce/CoffeePOS failure points và xác nhận Retry tiếp tục an toàn hoặc trả repair error rõ ràng mà không xóa store.
