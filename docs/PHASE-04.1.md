# Phase 4.1 — Provisioning UI (Windows-first)

## Trạng thái

**Hoàn thành Windows-first ngày 2026-09-17.** Provisioning UI đã được nối với native Phase 3, fresh install đã chạy trực tiếp từ Tauri app và restart vẫn đọc lại `ready` mà không reinstall.

Phase 4.1 chỉ nối giao diện desktop hiện tại với native WordPress provisioning đã hoàn thành ở Phase 3. Không thêm WooCommerce, CoffeePOS, POS WebView hoặc packaging trong phase này.

## Mục tiêu

Người dùng có thể mở CoffeePOS Desktop trên một store mới, thấy rõ rằng WordPress chưa được cài, bấm một nút cài đặt và theo dõi kết quả provisioning thật từ native layer.

Flow mục tiêu:

```text
Mở app
  ↓
Đọc shell/config
  ↓
get_provisioning_info
  ↓
Not installed
  ↓
[ Cài đặt WordPress ]
  ↓
provision_wordpress
  ↓
Installing
  ↓
Ready hoặc Error
```

## Native contract dùng lại

Phase 4.1 dùng các Tauri command đã tồn tại từ Phase 3:

- `get_provisioning_info`
- `provision_wordpress`

Không tạo một provisioning path khác trong frontend và không chạy PHP/MariaDB bằng shell command từ UI.

`ProvisioningInfo` phải được frontend biểu diễn tối thiểu bằng các field hiện có:

```text
state
wordpress_version
admin_username
can_retry
last_error
```

Các state Phase 4.1 cần xử lý:

```text
not_installed
installing
ready
needs_repair
```

Frontend không được tự suy luận `ready` từ việc process đã spawn hoặc HTTP port đang mở; trạng thái provisioning phải đến từ native contract.

Phase 4.1 bổ sung `can_retry` vào `ProvisioningInfo` để UI không mặc định chạy provisioning lại cho mọi trạng thái repair. `get_provisioning_info` cũng phản ánh `installing` khi native provisioning mutex đang được giữ, nên WebView reload trong cùng process không biến một operation đang chạy thành fake `needs_repair`.

## UI scope

### Not installed

Hiển thị rằng môi trường WordPress chưa được cài và bật action cài đặt.

Yêu cầu:

- Nút cài đặt enabled khi không có provisioning operation đang chạy.
- Text giải thích ngắn gọn rằng thao tác sẽ tạo local database và WordPress store.
- Không mô tả WooCommerce/CoffeePOS là đã được cài trong phase này.

### Installing

Trong lúc `provision_wordpress` đang chạy:

- Disable nút Install để ngăn double-submit.
- Disable các runtime action có thể xung đột với provisioning.
- Hiển thị trạng thái đang cài đặt bằng `aria-live` hiện có hoặc vùng status tương đương.
- Không fake progress percentage nếu native layer chưa cung cấp progress event thực tế.

Phase 4.1 chỉ cần trạng thái tổng quát `Installing`; progress theo từng bước có thể làm ở Phase 4.2 nếu cần contract native riêng.

### Ready

Khi command trả về `ready`:

- UI hiển thị WordPress đã được cài thành công.
- Hiển thị version WordPress khi có.
- Không tiếp tục hiển thị nút Install như một hành động fresh-install.
- Refresh state phải tiếp tục trả Ready mà không chạy provisioning lại.

Phase 4.1 chưa cần nút mở WordPress; action đó thuộc Phase 4.3.

### Needs repair / Error

Nếu `get_provisioning_info` trả `needs_repair`, hoặc `provision_wordpress` trả lỗi:

- Không hiển thị Ready.
- Hiển thị nguyên ý nghĩa message/recovery từ native layer theo cách người dùng đọc được.
- Chỉ cho phép retry provisioning khi native trả `can_retry = true`; lỗi trạng thái/manifest không retryable chỉ cho phép kiểm tra lại state.
- Không tự xóa `site/`, `database/`, credentials hoặc provisioning journal để thử lại.

Phase 4.1 chưa triển khai repair engine; repair đầy đủ thuộc Phase 6.2.

## Hành vi khi app khởi động

Sau `get_shell_info`, frontend phải gọi `get_provisioning_info` trước khi quyết định trạng thái setup.

Các trường hợp cần đúng:

| Native state | UI mong đợi |
| --- | --- |
| `not_installed` | Hiển thị Install |
| `installing` | Hiển thị đang cài, disable action xung đột |
| `ready` | Hiển thị WordPress đã sẵn sàng |
| `needs_repair` | Hiển thị lỗi/recovery và Retry khi phù hợp |
| command error | Hiển thị lỗi rõ ràng, không giả state |

Reload/restart app trên store đã provision phải đọc lại `ready`; không yêu cầu người dùng cài lại.

## Quan hệ với Runtime UI hiện tại

Runtime controls hiện có là công cụ development và vẫn có thể tồn tại trong Phase 4.1, nhưng provisioning UI phải tránh để người dùng khởi động/dừng/restart runtime trong khi install đang chạy.

Phase 4.1 không cần thiết kế lại toàn bộ Runtime UI. Việc hợp nhất lifecycle WordPress/runtime và UX start/stop/retry đầy đủ thuộc Phase 4.2.

## File dự kiến thay đổi

Phạm vi implementation dự kiến tập trung ở:

```text
index.html
src/main.ts
src/styles.css
```

Chỉ sửa native Rust nếu Phase 4.1 phát hiện contract hiện tại không đủ để UI thể hiện đúng trạng thái hoặc lỗi. Nếu phải đổi contract native, thay đổi phải nhỏ, typed và có test phù hợp.

Không chỉnh WordPress core, WooCommerce, CoffeePOS plugin hoặc runtime artifact trong Phase 4.1.

## Validation bắt buộc

### Static/build checks

```powershell
npm run lint
npm run build:ui
```

Nếu có thay đổi Rust:

```powershell
. .\scripts\use-local-rust.ps1
cargo test --manifest-path .\src-tauri\Cargo.toml --locked
cargo clippy --manifest-path .\src-tauri\Cargo.toml --locked --all-targets -- -D warnings
```

### Native manual acceptance — Windows x64

Trên development runtime đã stage:

1. Mở app bằng `npm run dev` với một store chưa provision.
2. Xác nhận UI hiển thị Not installed và nút Install enabled.
3. Bấm Install đúng một lần.
4. Xác nhận UI chuyển sang Installing và action xung đột bị disable.
5. Chờ provisioning thật hoàn tất.
6. Xác nhận UI chuyển sang Ready và hiển thị WordPress version.
7. Reload/restart app.
8. Xác nhận app đọc lại Ready, không yêu cầu reinstall.
9. Xác nhận PHP/MariaDB không bị spawn thêm do frontend polling/state refresh ngoài lifecycle native dự kiến.

### Error acceptance

Ít nhất một lỗi native an toàn để diễn tập phải được hiển thị đúng message/recovery và UI quay về trạng thái có thể retry sau khi operation kết thúc. Không phá store thật chỉ để tạo lỗi test.

## Definition of Done

Phase 4.1 chỉ được đánh dấu hoàn thành khi tất cả điều kiện sau đạt:

- UI gọi `get_provisioning_info` khi bootstrap.
- Fresh store hiển thị đúng `not_installed`.
- Nút Install gọi `provision_wordpress` thật.
- Double-submit và runtime action xung đột bị chặn trong khi install.
- Success hiển thị `ready` từ native response, không fake-ready.
- Error/`needs_repair` hiển thị thông tin phục hồi có thể hành động.
- Restart app trên store đã cài nhận lại `ready` và không reinstall.
- `npm run lint` và `npm run build:ui` pass.
- Native manual acceptance Windows x64 pass trên provisioning thật.
- Tài liệu Phase 4.1 được cập nhật từ `Chưa triển khai` sang kết quả thực tế và ghi bằng chứng validation.

## Bằng chứng validation — Windows x64 2026-09-17

| Kiểm tra | Kết quả |
| --- | --- |
| `npm run lint` với workspace-local Rust | PASS |
| `npm run build:ui` | PASS |
| Rust tests | PASS: 20 passed, 0 failed, 2 real-runtime tests ignored mặc định |
| Clippy `-D warnings` + rustfmt | PASS |
| Tauri release `--no-bundle` build | PASS |
| Fresh app state | PASS: UI hiển thị `not_installed`, WordPress 7.1 và nút Install enabled |
| Bấm Install một lần | PASS: UI chuyển `installing`, Install/runtime/settings action xung đột bị khóa |
| Native provisioning thật | PASS: database + WordPress hoàn tất và UI nhận `ready` |
| Ready details | PASS: WordPress 7.1, admin `coffeepos_admin`, không còn fresh-install action |
| Restart app | PASS: `get_provisioning_info` đọc lại `ready`, không reinstall |
| Cleanup sau đóng dev app | PASS: không còn CoffeePOS PHP/MariaDB/app process từ acceptance run |
| Error acceptance | PASS: tạm rút development WordPress manifest tạo native error an toàn; UI hiển thị message/recovery và action `Kiểm tra lại trạng thái` |
| Error recovery | PASS: restore manifest rồi `Kiểm tra lại trạng thái` trở về `ready` mà không gọi full provisioning |

Native contract được chỉnh nhỏ trong Phase 4.1 vì contract cũ chưa đủ cho UI an toàn:

- `ProvisioningInfo.can_retry` phân biệt retryable provisioning với repair/state error.
- `needs_repair` mang `last_error`/recovery thay vì chỉ có state mơ hồ.
- `get_provisioning_info` dùng provisioning mutex để báo live `installing` khi operation native đang chạy.

Không thêm WooCommerce, CoffeePOS, POS route/WebView hoặc release runtime packaging trong acceptance này.

## Ngoài phạm vi

- Progress chi tiết theo từng bước provisioning: Phase 4.2 nếu cần.
- Đồng bộ UX start/stop/retry toàn runtime: Phase 4.2.
- Nút mở WordPress/local URL: Phase 4.3.
- WooCommerce artifact/provision/activation: Phase 4.4–4.6.
- CoffeePOS artifact/provision/activation/health: Phase 4.7–4.10.
- Full-stack idempotency/recovery: Phase 4.11–4.12.
- POS WebView: Phase 5.x.
- Repair engine: Phase 6.2.

## Gate sang Phase 4.2

Chỉ bắt đầu Phase 4.2 sau khi một người dùng có thể thực hiện thành công flow sau trực tiếp từ app development trên Windows:

```text
Fresh store
  → mở CoffeePOS Desktop
  → Install
  → WordPress provisioning thật
  → Ready
  → restart app
  → vẫn Ready
```
