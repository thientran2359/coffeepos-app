# Provisioning — Phase 3 và Phase 4.x

Windows-first Phase 3 đã pin baseline WordPress core 7.1; PHP, database và WordPress không được tải/copy từ LocalWP hiện có.

Development baseline được tạo bằng `scripts/stage-wordpress-development.ps1` từ `https://wordpress.org/wordpress-7.1.zip`. Manifest checked-in pin SHA256 `d1ae02b5ae18428031ffc3943659fa87ab361d827f4aa804adf9276e4dc75df6` và official SHA1 `b2b81d9242a122a8c7104a92387794eb64fcde97`, sau đó xác minh `$wp_version` trong `wp-includes/version.php` là `7.1`. Trong target ignored `runtime/development/x86_64-pc-windows-msvc/`, `wordpress-manifest.json` nằm ở target root và trường `core_root` resolve tới core nguyên bản `wordpress/wordpress/`. File `license.txt`/`readme.html` nguyên bản đi cùng core. Không có WooCommerce/CoffeePOS trong bước staging Phase 3 này.

Phase 3 native provisioning hiện thực theo ensure semantics:

1. Resolve runtime/WordPress manifest chỉ từ đường dẫn tuyệt đối nằm trong `runtime/development` và kiểm tra schema/path escape.
2. Chỉ khởi tạo MariaDB khi datadir còn trống. Partial/corrupt datadir được giữ nguyên và trả lỗi repair rõ ràng.
3. Tạo database `coffeepos`, tài khoản runtime/WordPress và credential được bảo vệ bằng Windows DPAPI; bootstrap credential được xóa sau khi account đã xác minh.
4. Copy WordPress core đã pin vào `site/` qua staging directory, sinh `wp-config.php`, router và uploads bridge dưới ownership marker của CoffeePOS Desktop.
5. Start MariaDB/PHP qua Runtime Manager, chạy WordPress bootstrap để tạo schema và initial administrator, sau đó kiểm tra `/wp-login.php` thật trước khi ghi journal `wordpress_installed`.
6. Retry giữ nguyên datadir/site đã quản lý, không đổi credential hiện có và từ chối ghi đè site/config không thuộc CoffeePOS Desktop.

Uploads được tách khỏi core mutable tại application-data root. Baseline Phase 3 chỉ chứa WordPress core. UI provisioning thuộc Phase 4.1–4.3; WooCommerce thuộc Phase 4.4–4.6; CoffeePOS artifact/provisioning/activation/machine health đã hoàn thành ở Phase 4.7–4.10; full-stack idempotency đã hoàn thành ở Phase 4.11 và interruption recovery đã hoàn thành ở Phase 4.12. Xem [ROADMAP.md](ROADMAP.md).

Validation Windows x64 dùng staged runtime thật:

```powershell
. .\scripts\use-local-rust.ps1
cargo test --manifest-path .\src-tauri\Cargo.toml --locked provisioning::tests::staged_runtime_provisions_twice_stops_and_cleans_temp_store -- --ignored
cargo test --manifest-path .\src-tauri\Cargo.toml --locked provisioning::tests::staged_runtime_recovers_across_first_run_journal_boundaries -- --ignored
cargo test --manifest-path .\src-tauri\Cargo.toml --locked provisioning::tests::staged_runtime_blocks_retry_for_partial_wordpress_tables -- --ignored
```

Test tạo store tạm trong `src-tauri/target/phase3-e2e`, chạy fresh provisioning toàn baseline đến CoffeePOS machine health, rồi chạy full provisioning lần hai trên cùng store. Phase 4.11 mở rộng acceptance để snapshot và giữ nguyên DB runtime/WordPress/admin/machine credentials, `wp_users.user_pass` của admin, managed wp-config/salts, external uploads, WordPress/WooCommerce/CoffeePOS/unrelated-plugin sentinels, Woo/CoffeePOS ownership metadata, active plugin state, journal, một CoffeePOS store option và một row thật trong `wp_coffeepos_suspended_carts`; row business sentinel phải vẫn tồn tại đúng một lần sau rerun. Test cũng xác nhận PID/port/staging/probe được cleanup trước khi xóa store tạm.

Phase 4.12 thêm staged recovery E2E trên store tạm riêng: inject interruption sau từng side effect nhưng trước journal commit, stop/drop native state, recreate trên cùng `data_root`, rồi Retry. Database/site/WordPress/WooCommerce/CoffeePOS/machine-health đều phải resume tới `ready` mà giữ nguyên protected credentials. Một E2E riêng seed partial `wp_*` table sau `site_ready`; bootstrap thật phải preserve table, persist `partial_wordpress_install`, relaunch báo `can_retry=false` và normal provisioning từ chối chạy tiếp.

Phase 4.8 native provisioning consume `coffeepos-manifest.json` + exact staged CoffeePOS `1.0.0`, preflight compatibility với WordPress `7.1`, PHP `8.4.25`, MariaDB `11.4.13`, WooCommerce `11.1.0`, rồi ensure plugin qua owned `coffeepos.provisioning` và atomic rename. Journal append `coffee_pos_provisioned`; CoffeePOS chưa active ở stage này. Existing Phase 4.6 journal không có `coffeepos_version` vẫn đọc được và chuyển `needs_repair + can_retry` cho tới retry.

Phase 4.9 tiếp tục cùng orchestration path bằng hai bounded pinned-PHP process. Activation process xác minh exact active WooCommerce 11.1.0 rồi gọi WordPress `activate_plugin` cho CoffeePOS 1.0.0; nếu retry sau một partial attempt đã để plugin active nhưng journal còn `coffee_pos_provisioned`, native gọi lại `CoffeePOS\Core\Lifecycle::activate()` để plugin tự đảm bảo defaults/capabilities/migration/rewrite. Fresh verification process sau đó load WordPress từ đầu và chỉ pass khi exact CoffeePOS runtime/autoload có mặt, `Migrator::SCHEMA_VERSION` khớp option và mọi `Schema::tableNames()` tồn tại vật lý, mọi `Settings::optionNames()` đã được seed, CoffeePOS roles/capabilities tồn tại, rewrite version/rules đúng và REST server thật chứa `/coffeepos/v1/health` cùng toàn bộ `RouteRegistrar::registeredRoutes()` của artifact. Native không tạo table/default/capability thay plugin. Journal chỉ chuyển sang `coffee_pos_activated` sau verifier pass; timeout/failure giữ stage provisioned để retry và giữ nguyên dữ liệu.

Pinned CoffeePOS 1.0.0 artifact của Phase 4.9 có schema `0.0.1` và rewrite version `1.0.0:3`. Phase 4.10 nâng exact managed artifact lên CoffeePOS `1.0.1`, build từ clean plugin commit `6de371a104aca6ae426ebea78a97460820dc4d65`, checked-in ZIP SHA256 `67e3f268ffd29946cfb4fdce13d6e7ad12caaf3ca7af007cff2177940d8e4a64`. Endpoint mới không đổi plugin DB schema nên `Migrator::SCHEMA_VERSION` vẫn là `0.0.1`. Native chỉ tự động upgrade đúng managed 1.0.0 Phase 4.9 artifact sang exact 1.0.1 này qua owned staging/backup; unmanaged, corrupt hoặc version/hash khác được preserve và từ chối.

## CoffeePOS machine-health contract

`GET /wp-json/coffeepos/v1/system/status`

```json
{
  "schema_version": 1,
  "status": "healthy",
  "wordpress": true,
  "woocommerce": true,
  "coffeepos": true,
  "database": true,
  "versions": {
    "wordpress": "7.1",
    "woocommerce": "11.1.0",
    "coffeepos": "1.0.1",
    "coffeepos_schema": "0.0.1"
  },
  "store": { "name": "My Coffee" },
  "pos_path": "/pos/"
}
```

Schema version `1` là contract Desktop/Plugin cho Phase 4.10. `status` là `healthy` khi toàn bộ dependency bắt buộc ready và `degraded` khi endpoint vẫn chạy nhưng một dependency không ready. Các version phải phản ánh component đang load thật, không suy diễn từ filename/artifact metadata.

`pos_path` phải là path tương đối cùng origin và được plugin sinh từ CoffeePOS Router/settings thật. Desktop chỉ chấp nhận path bắt đầu bằng `/`, không có scheme/host và resolve nó trên origin runtime đã kiểm chứng; không hardcode `/pos/` như domain contract.

HTTP 200 chỉ khi required components ready, 503 khi endpoint còn chạy nhưng dependency lỗi, 401 khi thiếu/sai machine credential. Nếu PHP/WP/DB lỗi tới mức endpoint không chạy hoặc plugin bị inactive thì desktop phân loại transport/bootstrap failure; không giả health OK.

Request dùng header `X-CoffeePOS-Machine-Token`. Desktop tạo token từ 32 random bytes và encode lowercase hex; plaintext chỉ giữ trong native protected storage (Windows DPAPI trong Windows-first flow), không đưa vào WebView, query string, process arguments hoặc log. Plugin chỉ giữ SHA-256 của token trong WordPress option `coffeepos_desktop_machine_token_sha256` và dùng `hash_equals` khi so sánh.

Bootstrap token chạy bằng bounded pinned-PHP CLI sau khi CoffeePOS đã active: native truyền token qua stdin cho local bootstrap script, script chỉ persist SHA-256 rồi xóa buffer/temporary script theo lifecycle provisioning. Rotation giữ cả active + pending plaintext token trong protected native storage, cập nhật WordPress hash qua cùng local CLI, probe endpoint bằng pending token rồi mới promote; nếu probe/update fail thì local CLI restore old hash và pending token bị bỏ. Không reset token khi restart hoặc retry provisioning bình thường.

Không dùng quyền admin/staff session để auth machine-health. Khi LAN bật endpoint vẫn bắt buộc machine token. Response không chứa secret, filesystem path, user/session data hoặc dữ liệu khách; `store.name` chỉ là store identity đã cấu hình.

Contract schema/token bootstrap/rotation/version/POS route này được chốt ở Phase 4.7 và được implement ở CoffeePOS `1.0.1` trong Phase 4.10. User-facing `/wp-json/coffeepos/v1/health` cũ vẫn giữ nguyên staff/session authorization và không thay machine-health contract. Desktop đọc kết quả `/system/status`, validate schema/HTTP/POS path rồi phân loại application health; Rust không tái tạo CoffeePOS domain/dependency logic.

## Các gate tích hợp còn lại

- `ProvisioningInfo.ready` là trạng thái installation; không thay health sau mỗi lần start/restart. Runtime vẫn kiểm WordPress riêng, sau đó probe CoffeePOS machine-health khi WordPress healthy và xóa health cũ khi stop/start failure/process death.
- Phase 4.6 và 4.9 đã kiểm initialization/schema/plugin-owned baseline ngoài cờ plugin active. Phase 4.10 phải giữ ranh giới này: machine-health đọc trạng thái application thật, không thay activation verifier bằng file/journal metadata.
- Phase 4.11 đã chứng minh completed-store full rerun giữ nguyên credential, uploads, plugin ownership/state và business data. Phase 4.12 đã chứng minh failure/interruption recovery ở từng commit boundary và UI điều phối retry qua journal sau relaunch. Journal có optional `recovery_blocker`; partial WordPress tables từ interrupted `wp_install()` chuyển sang `needs_repair + can_retry=false`, được preserve và không bị normal provisioning tự xóa/reinstall. Readiness của completed store vẫn yêu cầu protected DB/admin/machine credentials đọc được.
- Phase 5.2 triển khai UX đặt/nhận credential ban đầu; Phase 5.4 tích hợp mở POS/login thật theo [UI-UX.md](UI-UX.md). Secret được bảo vệ trong native storage không có nghĩa người dùng đã đăng nhập được; machine health auth tách khỏi staff auth. Không reset credential khi restart hoặc retry.
