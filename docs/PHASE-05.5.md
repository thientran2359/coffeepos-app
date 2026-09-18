# Phase 5.5 — Khởi động hằng ngày

Ngày triển khai: 2026-09-18.

## Mục tiêu

Khi người vận hành mở CoffeePOS Desktop trên một store đã provisioning hoàn tất, Desktop tự khởi động runtime hiện có và đưa Trang chính qua các trạng thái khởi động → kiểm tra → sẵn sàng. Người dùng không phải bấm **Khởi động** trong luồng hằng ngày.

Phase này chỉ tự start runtime khi app được mở hoặc reload trên installation ready. Nó không tự mở POS, không tự đăng nhập và không đăng ký chạy cùng Windows.

## Startup contract

~~~text
Load Desktop config
  ↓
Inspect provisioning
  ↓
ready?
  ├─ no  → giữ setup/recovery flow hiện có
  └─ yes → refresh native runtime state
             ↓
           stopped?
             ├─ yes → start_runtime
             └─ no  → chỉ render state hiện tại
~~~

- ProvisioningState::Ready là gate bắt buộc. not_installed, installing và needs_repair không auto-start.
- Frontend chỉ auto-start khi native get_runtime_info báo stopped.
- Nếu runtime đã running, reload/UI bootstrap chỉ refresh; không gọi start lần nữa và không respawn child.
- Nếu runtime đang starting hoặc stopping, bootstrap chỉ render trạng thái hiện tại.
- start_runtime tiếp tục dùng lifecycle mutex và RuntimeManager hiện có; không tạo đường startup backend thứ hai.
- Polling 2 giây chỉ refresh runtime và bị tạm ngưng trong lúc bootstrap. Nó không start runtime và không mở POS.
- Auto-start không gọi open_pos, nên reload/relaunch không tự sinh tab trình duyệt.

## Trạng thái UI

Installed store đang start dùng state Phase 5.3:

~~~text
Đang khởi động
Đang khởi động cửa hàng…

[ Đang khởi động… ]
~~~

Khi healthy, giữ UI Phase 5.4 với **Mở bán hàng**. Desktop không tự bấm action này.

Startup failure giữ recovery hiện có: Home hiển thị lỗi và **Thử lại** gọi start_runtime thật, không provisioning lại.

Setup/recovery chưa hoàn tất không auto-start; wizard/journal flow hiện có vẫn là nguồn sự thật.

## Reload, session và dữ liệu

- Reload frontend khi runtime đang chạy giữ process hiện tại; không start lại và không đổi port do frontend.
- Relaunch app sau lần đóng sạch sẽ bắt đầu từ native stopped rồi auto-start store hiện có.
- WordPress/CoffeePOS browser session không bị Desktop reset. Cookie còn hợp lệ thì POS tiếp tục dùng session; hết hạn thì auth hiện có yêu cầu login lại.
- Startup automation không thay store/account/password, machine token, provisioning journal hoặc dữ liệu POS.
- Saved startup_view vẫn được tôn trọng; auto-start chạy độc lập với view được chọn.

## Acceptance

1. Installed ready + stopped → mở/reload app → auto-start → Home progress → healthy → **Mở bán hàng**.
2. Runtime đã running → reload frontend → PID giữ nguyên; không tăng runtime start requested.
3. Polling/navigation không gọi start hoặc open POS.
4. Startup failure → Home có **Thử lại** và retry gọi start thật.
5. not_installed → Welcome/setup, không auto-start.
6. needs_repair hoặc setup gián đoạn → recovery hiện có, không auto-start/reinstall.
7. Auto-start không tự mở browser tab POS.
8. Startup automation không thay secrets, journal hoặc business data.
9. Saved startup view vẫn hoạt động như Phase 5.3.

## Validation hiện tại

Snapshot implementation đã pass bộ kiểm tra gọn:

~~~text
git diff --check — PASS
npm run lint:ui — PASS
npm run build:ui — PASS (7 modules)
cargo fmt --manifest-path .\src-tauri\Cargo.toml -- --check — PASS
cargo test --manifest-path .\src-tauri\Cargo.toml --locked runtime::tests::pos_url_requires_current_healthy_machine_route_and_port — PASS
~~~

Rust dùng toolchain local trong .tools/cargo + .tools/rustup vì shell hiện tại không có Cargo trên PATH. Focused compile cũng chạy Tauri build script và generate permission allow-open-pos, xác nhận capability bổ sung cho Phase 5.4 hợp lệ.

Manual acceptance còn chờ cho relaunch/reload giữ PID đúng, số log runtime start requested, injected startup failure → Retry và xác nhận auto-start không mở browser tab POS.

## Ngoài scope

- Tự chạy CoffeePOS Desktop cùng Windows/macOS login.
- Tự mở POS hoặc auto-login.
- Minimize/tray/exit/shutdown UX: Phase 5.6.
- Repair/log export đầy đủ: Phase 6.x.
- LAN bind/security: Phase 8.x.
