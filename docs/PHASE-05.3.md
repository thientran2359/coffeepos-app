# Phase 5.3 — Trang chính và cài đặt ứng dụng

Phase 5.3 hoàn thiện khu vực store đã cài của CoffeePOS Desktop. Trang chính dùng trạng thái native/runtime thật để nói ngắn gọn cửa hàng đang dừng, đang khởi động, đang kiểm tra, sẵn sàng hay gặp lỗi và đưa ra đúng một hành động vận hành phù hợp. Cài đặt chỉ chứa preference thuộc Desktop; chi tiết PHP/database/port/path tiếp tục nằm ở Chẩn đoán.

## Phạm vi

- Store `ready + stopped` ở Trang chính có **Khởi động**; lỗi start gần nhất đổi hành động thành **Thử lại** nhưng vẫn gọi `start_runtime`, không provisioning lại.
- `starting` / `stopping` / health checking hiển thị trạng thái chờ thật và khóa submit xung đột.
- Runtime đang chạy nhưng WordPress/CoffeePOS health chưa đạt cho phép **Thử lại** bằng native health recheck; không reinstall và không reset credential.
- `healthy` chỉ báo **Hệ thống đã sẵn sàng**. Nút **Mở bán hàng** thuộc Phase 5.4 nên chưa xuất hiện.
- Home chỉ giữ tên cửa hàng đã xác minh, trạng thái dễ hiểu và điều hướng Cài đặt/Chẩn đoán. PHP, MariaDB, port, path, machine-health classification vẫn ở Chẩn đoán.
- Cài đặt Desktop có một preference thật: **Trang mở đầu** (`home`, `settings`, `diagnostics`). Mặc định `home`; config schema 1 cũ không có field này vẫn load thành `home`. Save dùng cùng atomic config persistence và có success/error feedback.
- `bind_host` vẫn local-only `127.0.0.1`. LAN control thuộc 8.x; auto-start runtime thuộc 5.5.

## Wireframe

### Đã cài, runtime dừng

```text
Trang chính
<tên cửa hàng đã xác minh nếu có>

Hệ thống đang dừng
Cửa hàng đã cài đặt và chưa chạy trên máy này.

[ Khởi động ]  [ Cài đặt ]  [ Xem chẩn đoán ]
```

### Đang khởi động / đang kiểm tra

```text
Đang khởi động cửa hàng…
[ Đang khởi động… ] disabled

hoặc

Đang kiểm tra cửa hàng…
[ Đang kiểm tra… ] disabled
```

### Sẵn sàng

```text
Hệ thống đã sẵn sàng
Cửa hàng đang chạy và đã qua kiểm tra ứng dụng.

[ Cài đặt ]  [ Xem chẩn đoán ]
```

### Lỗi có thể thử lại

```text
Không thể khởi động / Cửa hàng chưa sẵn sàng
Thông báo ngắn ở mức người vận hành.

[ Thử lại ]  [ Xem chẩn đoán ]
```

### Cài đặt

```text
Cài đặt Desktop

Trang mở đầu
[ Trang chính v ]

[ Lưu cài đặt ]
Đã lưu / lỗi native có thể hành động

Kết nối mặc định: Chỉ trên máy này
Phiên bản ứng dụng: ...
```

## Native contract

Tái sử dụng:

- `get_shell_info`
- `get_provisioning_info`
- `get_runtime_info`
- `start_runtime`
- `stop_runtime`
- `restart_runtime`

Bổ sung nhỏ cho Phase 5.3:

- `save_app_settings(startup_view)` persist `AppConfig.startup_view` atomically và trả `ShellInfo` mới.
- `retry_runtime_health()` chỉ chạy khi runtime đang `running`; refresh WordPress health rồi CoffeePOS machine health bằng cơ chế RuntimeManager hiện có. Command không gọi provisioning, không đổi installation journal và không reset account/secret.

RuntimeManager tiếp tục là nguồn sự thật. Khi start/stop/failure/process death, WordPress/CoffeePOS health cũ phải bị clear trước khi UI render trạng thái mới. Frontend có transition state riêng trong lúc command native đang block để không hiển thị healthy cũ trong lúc start/stop/retry.

## Acceptance

1. Store `ready + stopped` mở vào installed shell; Home hiển thị dừng và **Khởi động** gọi native start thật, không gọi provisioning.
2. Trong lúc start/stop/health retry, Home chuyển ngay sang trạng thái chờ và action bị khóa; double click không tạo lifecycle command thứ hai.
3. Start success đi qua checking rồi healthy; Home không lộ PHP/database/port/path và chưa có **Mở bán hàng**.
4. Start failure trở lại lỗi/stopped với health payload cũ bị loại; **Thử lại** chạy start thật.
5. Runtime running + WordPress/CoffeePOS health lỗi cho phép **Thử lại** health thật mà không reinstall; Diagnostics vẫn giữ chi tiết kỹ thuật.
6. Stop hoặc process death loại health cũ khỏi Home; trạng thái stopping/stopped hiển thị đúng.
7. Settings save `startup_view` có success/error feedback, giữ nguyên store/account metadata và runtime state; reload/relaunch chọn đúng view đã lưu. Config Phase 4/5.1/5.2 cũ mặc định Home.
8. Navigation Cài đặt/Chẩn đoán vẫn dùng chuột và bàn phím; layout hẹp vẫn cuộn tới action.

## Validation hiện tại

Implementation snapshot ngày 2026-09-18 đã pass bộ kiểm tra gọn theo yêu cầu của người dùng:

```text
git diff --check — PASS
npm run lint:ui — PASS
npm run build:ui — PASS (7 modules)
cargo fmt --manifest-path .\src-tauri\Cargo.toml -- --check — PASS
cargo test --manifest-path .\src-tauri\Cargo.toml --locked config::tests — PASS (9 passed, 0 failed)
```

Focused config suite bao gồm backward compatibility của schema 1 không có `startup_view` và persistence của startup view mà không đổi store metadata/bind host. Lượt này không chạy staged provisioning E2E hoặc native Tauri interaction acceptance dài; người dùng sẽ tự smoke-test flow Home/Settings trên app thật. Vì vậy Phase 5.3 đang ở trạng thái **đã triển khai, chờ manual acceptance**, chưa ghi là hoàn thành Windows-first theo Definition of Done của roadmap.

## Ngoài scope

- Mở POS/login/session: Phase 5.4.
- Auto-start runtime khi mở app: Phase 5.5.
- Minimize/exit/shutdown UX: Phase 5.6.
- Repair engine/log export đầy đủ: Phase 6.x.
- LAN bind/address/security: Phase 8.x.
- Đổi tên cửa hàng, tài khoản, nhân viên hoặc settings nghiệp vụ CoffeePOS trong Desktop.
