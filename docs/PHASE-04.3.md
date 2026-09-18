# Phase 4.3 — Open WordPress test (Windows-first)

## Trạng thái

**Hoàn thành Windows-first ngày 2026-09-18.**

Native/UI contract, real staged dynamic-port acceptance và system-browser flow trên app development đều đã pass. Gate sang Phase 4.4 đã mở.

## Mục tiêu

Cho phép người dùng mở site WordPress local từ CoffeePOS Desktop bằng đúng URL của runtime đang chạy:

```text
http://127.0.0.1:<dynamic-port>/
```

Không hard-code port, không nhận URL tùy ý từ frontend và không nạp WordPress vào management WebView của shell.

## Native contract đã triển khai

`RuntimeManager::wordpress_url()` chỉ trả URL khi đồng thời thỏa:

```text
runtime = running
wordpress_health = healthy
http_port = current managed port
```

Nếu runtime chưa chạy, WordPress chưa healthy hoặc không có managed HTTP port, native trả lỗi component `wordpress`, operation `open` thay vì tạo URL giả hoặc dùng port cũ.

Tauri command `open_wordpress`:

- không nhận URL từ frontend;
- kiểm `ProvisioningState::Ready` trước khi mở;
- refresh runtime hiện tại;
- lấy URL từ `RuntimeManager::wordpress_url()`;
- trên Windows mở URL bằng `ShellExecuteW` với system browser;
- trả lại đúng URL đã mở cho frontend;
- dùng lifecycle lock để không chạy đồng thời với install/start/stop/restart.

Command đã được đăng ký trong `tauri::generate_handler!`, `src-tauri/build.rs` và capability `allow-open-wordpress`.

## UI contract đã triển khai

Runtime card có action **Mở WordPress**.

Button chỉ enabled khi:

```text
provisioning = ready
runtime = running
wordpress_health = healthy
http_port != null
```

Frontend chỉ gọi:

```text
invoke("open_wordpress")
```

Frontend không truyền URL. Sau khi native mở thành công, UI hiển thị URL thực tế được native trả về. Khi runtime transition/stop/fail, action bị disable theo runtime state hiện tại.

## Dynamic-port acceptance

Real staged acceptance trong `provisioning::tests::staged_runtime_provisions_twice_stops_and_cleans_temp_store` đã có các assertion cho Phase 4.3:

- lấy managed WordPress URL từ port runtime hiện tại;
- stop runtime làm `wordpress_url()` trả lỗi;
- chiếm port HTTP cũ bằng `TcpListener`;
- start lại phải chọn HTTP port mới;
- WordPress health phải trở lại `healthy` trên port mới;
- managed URL phải chứa port mới, không dùng port cũ;
- `/wp-admin/` phải redirect tới `/wp-login.php` trên origin/port mới;
- response `/wp-login.php` phải chứa origin mới và không còn origin cũ;
- static asset `/wp-includes/css/dashicons.min.css` phải trả HTTP 200 trên port mới.

Test dùng store disposable dưới `src-tauri/target/phase3-e2e`, không dùng dữ liệu vận hành của người dùng.

## Validation — Windows x64 2026-09-18

Các check đã pass sau implementation:

| Kiểm tra | Kết quả |
| --- | --- |
| `npm run lint:ui` | PASS |
| `npm run build:ui` | PASS |
| `cargo test --locked` | PASS: 22 passed, 0 failed, 2 ignored |
| `cargo clippy --locked --all-targets -- -D warnings` | PASS |
| `cargo fmt --check` | PASS |
| `git diff --check` | PASS |
| `runtime::tests::wordpress_url_requires_current_healthy_running_runtime` | PASS |
| `runtime::tests::wordpress_probe_response_accepts_valid_partial_login_page` | PASS |
| Tauri permission/build wiring cho `open_wordpress` | PASS |

Real staged acceptance mới nhất:

```powershell
. .\scripts\use-local-rust.ps1
cargo test --manifest-path .\src-tauri\Cargo.toml --locked provisioning::tests::staged_runtime_provisions_twice_stops_and_cleans_temp_store -- --ignored
```

**PASS: 1 passed, 0 failed, 84.46s** trên code cuối cùng. Một lần chạy trước đó từng gặp MariaDB exit `0x80000003`, nhưng lỗi này không tái hiện trong các rerun sau; diagnostics được giữ trong ignored E2E để lần fail tương lai có `provisioning.log` trước khi TempDir bị cleanup.

Trong quá trình nghiệm thu đã phát hiện và sửa hai lỗi thực tế:

1. Router CoffeePOS chỉ trả `false` cho file thật, không cho directory thật. Vì vậy `/wp-admin/` bị route vào root `index.php` và trả homepage `200`. Router hiện trả `false` cho cả `is_file($local)` và `is_dir($local)`, để PHP built-in server xử lý `/wp-admin/` đúng và WordPress redirect/login bình thường.
2. Health probe `/wp-login.php` có thể mất hơn 10 giây trên PHP built-in server vì WordPress tự spawn WP-Cron loopback. Probe hiện dùng `/wp-login.php?doing_wp_cron=coffeepos-health`, đúng theo core WordPress `spawn_cron()` để bỏ self-loop cron chỉ cho request health; site thật không bị disable WP-Cron. Probe cũng chấp nhận response hợp lệ ngay khi đã đọc đủ status `200` + `loginform`, không phụ thuộc connection close.

App development thực tế cũng đã được kiểm bằng Windows UI Automation:

- provisioning state: `ready`;
- Start runtime → Open WordPress enabled;
- click **Mở WordPress** gọi native command thật;
- system browser Chrome mở site WordPress với address bar `127.0.0.1:58852`, đúng current managed HTTP port;
- Stop runtime → **Mở WordPress** disabled ngay.

## Definition of Done

Các mục sau đã pass trên Windows:

- real staged provisioning/runtime acceptance chạy hết;
- port cũ bị chiếm → runtime chọn port mới và WordPress vẫn healthy;
- redirect và generated origin dùng port mới;
- static assets được xác nhận dùng đúng origin hiện tại;
- system-browser action được click/test thật từ `npm run dev` và mở đúng managed URL;
- stop runtime → action bị disable và native từ chối mở;
- reload/restart và dynamic-port fallback không dùng stale URL/port;
- final lint/build/test/clippy/format/diff checks pass.

## Ngoài phạm vi

- đăng nhập wp-admin: chưa bắt buộc ở Phase 4.3;
- WooCommerce: Phase 4.4–4.6;
- CoffeePOS plugin: Phase 4.7–4.10;
- POS WebView riêng: Phase 5.2;
- auto-start runtime khi mở app: Phase 5.4;
- packaged production runtime/browser flow: Phase 9.

## Gate sang Phase 4.4

**Đã mở.** Phase tiếp theo là **Phase 4.4 — WooCommerce artifact**. Không kéo scope WooCommerce ngược vào Phase 4.3.
