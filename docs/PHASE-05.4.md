# Phase 5.4 — Mở POS và đăng nhập

Ngày triển khai: 2026-09-18.

## Mục tiêu

Phase 5.4 nối trạng thái application healthy trên Trang chính với POS thật của CoffeePOS. Desktop chỉ chịu trách nhiệm xác minh runtime hiện tại và yêu cầu hệ điều hành mở URL POS an toàn. Form đăng nhập, cookie/session và nghiệp vụ bán hàng tiếp tục thuộc WordPress/CoffeePOS.

## Quyết định host

MVP dùng **trình duyệt hệ thống**.

- Desktop không nhúng POS vào management WebView.
- Đóng tab POS không dừng runtime.
- Desktop không suy đoán người dùng đã đăng nhập, đăng xuất, mở ca hay đóng tab.
- Sau khi native giao lệnh mở URL thành công, UI chỉ báo **Đã yêu cầu mở trình duyệt**.
- POS WebView riêng vẫn là lựa chọn tương lai; nếu bổ sung phải không có management IPC và phải có navigation/session acceptance riêng.

## Wireframe

### Healthy

```text
CoffeePOS

Trang chính
<Tên cửa hàng từ machine health>

Sẵn sàng
Hệ thống đã sẵn sàng

Cửa hàng đang chạy. Mở POS trong trình duyệt và đăng nhập nếu được yêu cầu.

[ Mở bán hàng ]  [ Cài đặt ]  [ Xem chẩn đoán ]
```

Sau khi native giao lệnh mở thành công:

```text
Đã yêu cầu mở trình duyệt. Đăng nhập trong CoffeePOS nếu được yêu cầu.
```

### Runtime/health chưa sẵn sàng

Giữ state machine của Phase 5.3. Không hiển thị action Mở bán hàng khi runtime đang dừng/chuyển trạng thái, WordPress chưa healthy hoặc CoffeePOS machine health chưa healthy. Start/Retry hiện có là đường phục hồi.

### Open failure

Giữ Trang chính và hiển thị lỗi actionable do native trả về. Refresh runtime ngay sau failure để action tiếp theo phản ánh state hiện tại. Không coi lỗi mở browser là lỗi provisioning và không reinstall/reset store.

## Native contract

Bổ sung:

- `open_pos()` không nhận URL/path từ frontend.
- Command giữ lifecycle guard để không chạy đồng thời với start/stop/restart/provisioning.
- Native refresh process state trước khi tạo URL để process death loại health cũ.
- Chỉ cho mở khi provisioning `ready`, runtime `running`, WordPress `healthy` và CoffeePOS machine-health `healthy`.
- `pos_path` lấy từ payload machine-health schema 1 đã được parser validate; origin lấy từ loopback host + dynamic HTTP port của runtime instance hiện tại.
- URL được resolve thành `http://127.0.0.1:<current-port><pos_path>`; frontend không thể cung cấp arbitrary URL.
- System browser opener chỉ xác nhận OS nhận yêu cầu mở URL. Nó không chứng minh tab render thành công hoặc user đã authenticated.

`open_wordpress` development action ở Chẩn đoán vẫn giữ cho regression/debug; Mở bán hàng là product action trên Home.

## Login/session

CoffeePOS/WordPress tiếp tục sở hữu login form và cookie/session. Desktop không tự đăng nhập bằng machine token, không inject admin password vào URL/form và không đọc auth cookie của browser.

Tài khoản quản trị ban đầu từ Phase 5.2 vẫn xem username/copy protected initial password trong Cài đặt. Nếu password đã đổi trong WordPress, snapshot ban đầu có thể không còn đúng và Desktop không reset nó.

## Acceptance

1. Installed store đang stopped: Home chỉ có Khởi động; không thể gọi `open_pos`.
2. Sau start + WordPress healthy + CoffeePOS healthy: Home có **Mở bán hàng**.
3. Bấm Mở bán hàng mở URL từ machine-health `pos_path` trên dynamic port hiện tại bằng system browser.
4. Nếu chưa có session, CoffeePOS/WordPress yêu cầu login; credential onboarding đăng nhập được và vào POS.
5. Logout rồi login lại vẫn dùng auth hiện có; Desktop không giữ trạng thái “đã đăng nhập”.
6. Session hết hạn quay về login theo WordPress/CoffeePOS; Home vẫn chỉ phản ánh runtime/application health.
7. Stop/restart hoặc process death loại khả năng mở POS cho tới khi health mới đạt.
8. Restart làm đổi HTTP port thì lần Mở bán hàng tiếp theo dùng port mới; không cache URL cũ trong frontend/config.
9. Browser open failure hiển thị lỗi và cho thử lại; không gọi provisioning.
10. POS thật chạy đồng thời với managed cron/background behavior hiện có; không thêm business logic bán hàng vào Desktop.

## Validation

Theo preference kiểm thử gọn cho Phase 5.x, snapshot implementation hiện tại đã pass:

```text
git diff --check — PASS
npm run lint:ui — PASS
npm run build:ui — PASS (7 modules)
cargo fmt --manifest-path .\src-tauri\Cargo.toml -- --check — PASS
cargo test --manifest-path .\src-tauri\Cargo.toml --locked runtime::tests::pos_url_requires_current_healthy_machine_route_and_port — PASS
cargo test --manifest-path .\src-tauri\Cargo.toml --locked runtime::tests::wordpress_url_requires_current_healthy_running_runtime — PASS
```

Rust checks dùng toolchain local trong `.tools/cargo` + `.tools/rustup` vì shell hiện tại không có `cargo` trên PATH.

Login/order/logout/session-expiry và native browser interaction là manual acceptance trên disposable store; chưa dùng kết quả code compile để đánh dấu Phase 5.4 hoàn thành Windows-first.

## Ngoài scope

- Auto-start runtime khi mở app: Phase 5.5.
- Minimize/exit/shutdown UX: Phase 5.6.
- POS WebView riêng.
- Staff-account management hoặc auto-login.
- Repair engine/log export đầy đủ: Phase 6.x.
- LAN exposure/security: Phase 8.x.
