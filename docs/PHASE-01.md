# Phase 1 — Desktop Shell

## Phạm vi

- Tauri 2, một cửa sổ bundled shell, UI TypeScript/Vite không framework.
- Rust command `get_shell_info`, `save_store_name` với capability chỉ cho main.
- OS app-local-data: site, database, uploads, config, logs, backups.
- Schema 1, default 127.0.0.1; không chứa secret hoặc cổng runtime giả.
- Validate ở Rust, atomic config replacement, installation lock, lỗi có recovery.
- UI first-run, lỗi/retry và lưu cấu hình. Install bị disabled với lý do rõ ràng.

Ngoài phạm vi: process manager PHP/MariaDB, WordPress provisioning, WooCommerce/CoffeePOS activation, POS WebView, LAN, backup/restore, signing/release. Không có chức năng mô phỏng runtime chạy thành công.

## Kiểm thử đã viết

Rust tests sử dụng thư mục tạm biệt lập:

1. Fresh setup, save/reopen và dữ liệu store có sẵn không bị thay đổi; path Unicode.
2. JSON hỏng/schema tương lai bị từ chối, giữ nguyên bytes.
3. Tên rỗng/quá dài/control characters không thay đổi memory/disk.
4. Instance thứ hai bị khóa, mở được sau khi instance đầu nhả lock.

## Native acceptance

- [x] Windows x64: launch, UI first-run, data paths đúng.
- [ ] macOS arm64: launch/close/relaunch, WKWebView render/IPC.
- [ ] macOS x64: launch/close/relaunch, WKWebView render/IPC.
- [ ] Lưu tên có dấu; restart giữ nguyên; không đổi database/uploads sentinel.
- [ ] Tên sai bị từ chối; file JSON lỗi không bị reset; sửa file rồi retry phục hồi.
- [ ] Mở instance thứ hai: báo đang bị khóa; đóng instance đầu rồi retry thành công.
- [ ] Read-only config hoặc disk write failure: lỗi có hướng dẫn, dữ liệu cũ giữ nguyên.
- [ ] Đóng shell không để lại tiến trình app; phase này không spawn service.
- [ ] Bundled build dùng CSP production, IPC đọc/lưu hoạt động.
- [ ] App launch không cần Node/PHP/MariaDB global.

## Bằng chứng Windows 2026-09-17

Khảo sát ban đầu phát hiện Rust/MSVC chưa sẵn sàng trong shell. Sau đó project đã có Rust toolchain cục bộ trong `.tools/`, MSVC build đã hoạt động, và `scripts/use-local-rust.ps1` bật Cargo/Rust cho PowerShell hiện tại mà không cần PATH hệ thống.

| Kiểm tra | Kết quả |
| --- | --- |
| npm dependency install / lockfile | Thành công |
| `npm run build:ui` | PASS: TypeScript + Vite production build |
| Browser preview tại loopback | PASS: banner preview hiển thị đúng, không lỗi/warning console quan sát được |
| Native Tauri app launch Windows | PASS; app đã được mở và test trực tiếp |
| Tauri icon generation | Thành công: PNG/ICO/ICNS từ SVG trong repo |
| `npm run lint` / `cargo fmt --check` / Clippy | PASS trong validation hiện tại |
| `cargo generate-lockfile` | Thành công; Cargo.lock có 434 package resolution |
| `cargo check --locked` | PASS trong validation Phase 2/3 |
| Native release no-bundle build | PASS; tạo `coffeepos-desktop.exe` |
| Rust tests | PASS; test suite được mở rộng thêm ở Phase 2/3 |
| Native UI / app-data / persistence | PASS theo Windows-first flow; shell được dùng làm nền cho runtime/provisioning |
| macOS compile / launch | Chưa chạy, không có macOS host |
| CI | Không dùng làm bằng chứng thay cho native Windows validation trong phiên này |

**Phase 1 đã hoàn thành theo scope Windows-first.** Native Windows build/run đã được mở khóa trong các phase sau và shell hiện được dùng để chạy Runtime Manager/WordPress provisioning. Các mục macOS vẫn là acceptance riêng cho target đó; không dùng trạng thái Windows-first để suy luận macOS đã pass.
