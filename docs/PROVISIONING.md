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

Uploads được tách khỏi core mutable tại application-data root. Baseline Phase 3 chỉ chứa WordPress core. UI provisioning thuộc Phase 4.1–4.3; WooCommerce thuộc Phase 4.4–4.6; CoffeePOS artifact/provisioning/activation đã hoàn thành ở Phase 4.7–4.9, machine health thuộc Phase 4.10; full-stack idempotency/recovery thuộc Phase 4.11–4.12. Xem [ROADMAP.md](ROADMAP.md).

Validation Windows x64 dùng staged runtime thật:

```powershell
. .\scripts\use-local-rust.ps1
cargo test --manifest-path .\src-tauri\Cargo.toml --locked provisioning::tests::staged_runtime_provisions_twice_stops_and_cleans_temp_store -- --ignored
```

Test tạo store tạm trong `src-tauri/target/phase3-e2e`, chạy fresh provisioning toàn baseline đến CoffeePOS activation, start/install/stop, chạy provisioning lần hai, kiểm tra WordPress/WooCommerce/CoffeePOS/unrelated-plugin sentinel vẫn còn và xác nhận PID/port/staging/probe được cleanup trước khi xóa store tạm.

Phase 4.8 native provisioning consume `coffeepos-manifest.json` + exact staged CoffeePOS `1.0.0`, preflight compatibility với WordPress `7.1`, PHP `8.4.25`, MariaDB `11.4.13`, WooCommerce `11.1.0`, rồi ensure plugin qua owned `coffeepos.provisioning` và atomic rename. Journal append `coffee_pos_provisioned`; CoffeePOS chưa active ở stage này. Existing Phase 4.6 journal không có `coffeepos_version` vẫn đọc được và chuyển `needs_repair + can_retry` cho tới retry.

Phase 4.9 tiếp tục cùng orchestration path bằng hai bounded pinned-PHP process. Activation process xác minh exact active WooCommerce 11.1.0 rồi gọi WordPress `activate_plugin` cho CoffeePOS 1.0.0; nếu retry sau một partial attempt đã để plugin active nhưng journal còn `coffee_pos_provisioned`, native gọi lại `CoffeePOS\Core\Lifecycle::activate()` để plugin tự đảm bảo defaults/capabilities/migration/rewrite. Fresh verification process sau đó load WordPress từ đầu và chỉ pass khi exact CoffeePOS runtime/autoload có mặt, `Migrator::SCHEMA_VERSION` khớp option và mọi `Schema::tableNames()` tồn tại vật lý, mọi `Settings::optionNames()` đã được seed, CoffeePOS roles/capabilities tồn tại, rewrite version/rules đúng và REST server thật chứa `/coffeepos/v1/health` cùng toàn bộ `RouteRegistrar::registeredRoutes()` của artifact. Native không tạo table/default/capability thay plugin. Journal chỉ chuyển sang `coffee_pos_activated` sau verifier pass; timeout/failure giữ stage provisioned để retry và giữ nguyên dữ liệu.

Pinned CoffeePOS 1.0.0 artifact của Phase 4.9 có schema `0.0.1` và rewrite version `1.0.0:3`. Đây là trạng thái của exact artifact SHA256 đã pin ở Phase 4.7; sibling plugin checkout đang phát triển không được dùng để suy diễn baseline Desktop. Nếu Phase 4.10 thay plugin code/schema để implement machine-health contract thì phải bump version/hash và repin theo gate Phase 4.7.

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
    "coffeepos": "1.0.0",
    "coffeepos_schema": "0.0.2"
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

Contract schema/token bootstrap/rotation/version/POS route này được chốt ở Phase 4.7. Implementation/nghiệm thu endpoint thuộc Phase 4.10. CoffeePOS `1.0.0` artifact pin ở 4.7 vẫn có user-facing `/wp-json/coffeepos/v1/health` cũ; route đó không thay machine-health contract. Khi 4.10 sửa plugin để implement contract mới, phải bump CoffeePOS version + artifact hash và chạy lại acceptance 4.8–4.10; không mutate staged artifact cũ. Desktop đọc kết quả endpoint, không tái tạo CoffeePOS domain/dependency logic trong Rust.

## Các gate tích hợp còn lại

- `ProvisioningInfo.ready` là trạng thái installation; không thay health sau mỗi lần start/restart ở 4.2. Trước khi có endpoint CoffeePOS, kiểm WordPress riêng trong phạm vi đã có.
- Phase 4.6 và 4.9 đã kiểm initialization/schema/plugin-owned baseline ngoài cờ plugin active. Phase 4.10 phải giữ ranh giới này: machine-health đọc trạng thái application thật, không thay activation verifier bằng file/journal metadata.
- Phase 4.11/4.12 là kiểm chứng tích hợp; từng ensure operation phải an toàn với retry ngay từ lúc triển khai. Cuối 4.12, UI điều phối setup toàn stack và recovery qua journal sau khi app bị đóng giữa chừng.
- Trước 5.2 chốt UX đặt/nhận credential ban đầu và login thật. Secret được bảo vệ trong native storage không có nghĩa người dùng đã đăng nhập được; machine health auth tách khỏi staff auth. Không reset credential khi restart hoặc retry.
