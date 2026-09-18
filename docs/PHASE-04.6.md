# Phase 4.6 — WooCommerce activation (Windows-first)

## Trạng thái

**Hoàn thành Windows-first ngày 2026-09-18.**

Phase 4.6 activate exact managed WooCommerce `11.1.0` đã provision ở Phase 4.5, chạy setup/schema bắt buộc bằng PHP CLI có timeout, bỏ yêu cầu onboarding thủ công và chuyển WordPress cron/background jobs khỏi loopback self-request sang process PHP CLI do Desktop quản lý.

## Activation contract

Activation vẫn đi trong provisioning lifecycle hiện có, sau các bước:

```text
WordPress installed
→ WooCommerce files provisioned
→ WooCommerce activated + baseline verified
→ Ready
```

Journal có stage mới:

```text
woo_commerce_activated
```

Store chỉ được native báo `ready` khi journal đã đạt stage này và exact managed WooCommerce artifact vẫn hợp lệ. `ProvisioningInfo` có thêm `woocommerce_active`, nên UI không suy diễn activation chỉ từ file presence.

Nếu activation/setup fail, journal dừng ở `woo_commerce_provisioned`. WordPress core, database, WooCommerce files và ownership metadata được giữ nguyên để retry; provisioning không recopy/reset dữ liệu.

## Bounded PHP CLI activation

Native tạo temporary PHP bootstrap trong managed config directory rồi chạy bằng pinned PHP 8.4.25 + pinned `php.ini`; không gọi PHP từ `PATH` và không shell-expand command.

Activation command có timeout **90 giây**, stdout/stderr append vào:

```text
<app-local-data>/logs/woocommerce.log
```

Script xác minh trước/sau activation:

- managed plugin entry file tồn tại;
- plugin header đúng `11.1.0`;
- plugin được WordPress liệt kê active;
- `WooCommerce`, `WC_Install`, `WC()` load được;
- loaded WooCommerce version đúng `11.1.0`.

## Schema/setup baseline

WooCommerce plugin version và DB schema version không được giả định giống nhau. Với artifact 11.1.0 đã pin:

```text
WooCommerce plugin version: 11.1.0
WooCommerce DB version:     11.1.0-1
```

Phase 4.6 pin và verify riêng `11.1.0-1`. Đây là internal database update baseline của WooCommerce 11.1.0; `WC_Install::needs_db_update()` phải trả false trước khi Ready.

Baseline verifier yêu cầu:

- `woocommerce_version = 11.1.0`;
- `woocommerce_db_version = 11.1.0-1`;
- không còn WooCommerce DB update pending;
- core WooCommerce tables tồn tại, gồm sessions/order items/product lookup;
- Action Scheduler tables `actions`, `claims`, `groups`, `logs` tồn tại;
- Shop, Cart, Checkout và My Account pages tồn tại;
- role `shop_manager` tồn tại.

Nếu baseline chưa đủ, bootstrap gọi WooCommerce install/setup path thật. Action Scheduler schema được force-register nếu plugin activation chưa materialize đủ tables. Sau đó toàn bộ invariant được kiểm lại; thiếu bất kỳ invariant nào đều fail provisioning thay vì Ready giả.

## Onboarding baseline

CoffeePOS Desktop là local managed appliance nên fresh baseline không yêu cầu người vận hành vào wp-admin để hoàn tất WooCommerce setup wizard.

Trong lần activation baseline đầu tiên:

- WooCommerce automatic setup wizard redirect bị chặn;
- onboarding profile được đánh dấu `skipped` nếu chưa `completed`;
- setup task list được hidden;
- marketplace suggestions được tắt;
- usage tracking được đặt `no`;
- `_wc_activation_redirect` được xóa.

Các lựa chọn baseline này chỉ apply trước khi journal đạt `woo_commerce_activated`. Retry sau khi đã activate không ép lại onboarding/tracking settings nếu sau này người dùng chủ động thay đổi chúng.

## WP-Cron và background jobs

PHP built-in server của development runtime không phù hợp với WordPress/WooCommerce nested self-request cron: một request có thể tự gọi lại cùng single local server và gây delay/deadlock. Phase 4.6 giải quyết ở ownership boundary của Desktop:

```php
define('DISABLE_WP_CRON', true);
```

`wp-config.php` mới có flag này ngay từ đầu. Existing CoffeePOS-managed config được migrate bằng exact managed anchor; config bất thường không bị blind rewrite.

Desktop Runtime Manager thay thế web-triggered cron bằng managed process:

```text
pinned php.exe -c <pinned php.ini> <managed-site>/wp-cron.php
```

Worker:

- chạy ngay sau khi WordPress health trở thành healthy;
- được poll/lặp tối đa khoảng mỗi 60 giây khi runtime đang chạy;
- dùng cùng protected DB credential + current dynamic site URL;
- nằm trong Windows process containment như các managed children khác;
- được stop/cleanup cùng runtime;
- log lifecycle vào `runtime.log`, output/error vào `cron.log` nếu script có output.

Khi CoffeePOS Desktop/runtime đóng, cron/background jobs cũng dừng. Chúng resume ngay ở lần runtime healthy tiếp theo rồi tiếp tục interval trong khi app đang chạy. Không cần nested HTTP loopback request.

## Failure/retry acceptance trên development store

Trong triển khai Phase 4.6, development store Phase 4.5 chuyển đúng sang `needs_repair` vì journal chưa có activation stage.

Một activation verification đầu tiên phát hiện DB schema thực tế là `11.1.0-1` thay vì giả định `11.1.0`. Failure giữ journal ở:

```text
woo_commerce_provisioned
```

và giữ WordPress/database/plugin nguyên vẹn. Sau khi pin đúng schema contract và retry qua UI thật, store chuyển thành:

```text
ready
stage = woo_commerce_activated
WooCommerce 11.1.0 active
```

`logs/woocommerce.log` xác nhận:

```text
WooCommerce 11.1.0 active; schema/pages/Action Scheduler/onboarding baseline verified.
```

`runtime.log` cũng xác nhận managed cron/background worker được start và complete.

## Real staged E2E

Acceptance chạy trên disposable store bằng pinned PHP/MariaDB/WordPress/WooCommerce:

```powershell
. .\scripts\use-local-rust.ps1
cargo test --manifest-path .\src-tauri\Cargo.toml --locked provisioning::tests::staged_runtime_provisions_twice_stops_and_cleans_temp_store -- --ignored
```

Final run ngày 2026-09-18:

```text
1 passed, 0 failed
126.76s
```

E2E xác minh:

- fresh site activate exact WooCommerce `11.1.0`;
- `ProvisioningInfo.woocommerce_active = true`;
- journal stage `woo_commerce_activated`;
- managed `wp-config.php` có `DISABLE_WP_CRON`;
- WordPress health vẫn healthy sau activation/restart;
- WooCommerce Store API route trả HTTP 200;
- occupied dynamic HTTP-port fallback vẫn pass;
- second provisioning vẫn Ready + active;
- sentinel trong managed WooCommerce directory được giữ;
- unrelated plugin/sentinel được giữ;
- stop/restart/process cleanup acceptance cũ vẫn pass.

## Static validation

| Kiểm tra | Kết quả |
| --- | --- |
| `npm run lint:ui` | PASS |
| `npm run build:ui` | PASS |
| Rust unit tests | PASS: 26 passed, 0 failed, 2 ignored |
| `cargo clippy --locked --all-targets -- -D warnings` | PASS |
| `cargo fmt --check` | PASS |
| Real staged WooCommerce activation E2E | PASS: 1 passed, 126.76s |

## Ngoài phạm vi Phase 4.6

- CoffeePOS plugin artifact/source snapshot: Phase 4.7.
- CoffeePOS plugin provisioning/activation: Phase 4.8–4.9.
- CoffeePOS health endpoint: Phase 4.10.
- Full combined-stack idempotency/recovery: Phase 4.11–4.12.
- WooCommerce version upgrade giữa hai pinned releases khác nhau: explicit future upgrade flow; Phase 4.6 không silently replace artifact/version.

## Gate sang Phase 4.7

**Đã mở.** Phase tiếp theo là **Phase 4.7 — CoffeePOS artifact**. Artifact CoffeePOS phải có reproducible source/version/layout/hash contract riêng và không được copy ngẫu nhiên từ một LocalWP install đang chạy.
