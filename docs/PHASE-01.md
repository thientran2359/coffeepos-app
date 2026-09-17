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

## Native acceptance — cần chạy trên từng target

- [ ] Windows x64: launch, UI first-run, data paths đúng.
- [ ] macOS arm64: launch/close/relaunch, WKWebView render/IPC.
- [ ] macOS x64: launch/close/relaunch, WKWebView render/IPC.
- [ ] Lưu tên có dấu; restart giữ nguyên; không đổi database/uploads sentinel.
- [ ] Tên sai bị từ chối; file JSON lỗi không bị reset; sửa file rồi retry phục hồi.
- [ ] Mở instance thứ hai: báo đang bị khóa; đóng instance đầu rồi retry thành công.
- [ ] Read-only config hoặc disk write failure: lỗi có hướng dẫn, dữ liệu cũ giữ nguyên.
- [ ] Đóng shell không để lại tiến trình app; phase này không spawn service.
- [ ] Bundled build dùng CSP production, IPC đọc/lưu hoạt động.
- [ ] App launch không cần Node/PHP/MariaDB global.

## Bằng chứng phiên 2026-09-17

Máy khảo sát Windows x64 có Node 22.16.0, npm 10.9.2, WebView2 153.0.4234.32. Thiếu Cargo/Rust và không phát hiện MSVC build toolchain đầy đủ. Không có máy macOS trong phiên.

Sau khảo sát đã cài Rust 1.98.1 + rustfmt **cục bộ trong `.tools/`**, không sửa PATH hệ thống. `scripts/use-local-rust.ps1` bật toolchain cho PowerShell hiện tại.

| Kiểm tra | Kết quả |
| --- | --- |
| npm dependency install / lockfile | Thành công |
| `npm run build:ui` | PASS: TypeScript + Vite production build |
| Browser preview tại loopback | PASS: banner preview hiển thị đúng, không lỗi/warning console quan sát được |
| Tauri CLI info | Đọc được project/config; phát hiện WebView2; xác nhận thiếu MSVC |
| Tauri icon generation | Thành công: PNG/ICO/ICNS từ SVG trong repo |
| `cargo fmt --check` | PASS sau rustfmt |
| `cargo generate-lockfile` | Thành công; Cargo.lock có 434 package resolution |
| `cargo check --locked` | BLOCKED: `linker link.exe not found` khi compile dependency build scripts |
| Native build lần đầu | BLOCKED: Cargo chưa có trong PATH lúc chạy; sau cài local Rust, blocker kế tiếp được xác nhận là MSVC linker |
| Rust tests / Clippy | Chưa chạy thành công do thiếu native linker; không tính là PASS |
| Native UI / app-data / persistence | Chưa kiểm chứng trực tiếp; cần MSVC và native launch |
| macOS compile / launch | Chưa chạy, không có macOS host |
| CI | Đã viết workflow, chưa dispatch/push |

**Phase 1 đã có implementation và tài liệu, chưa nghiệm thu hoàn toàn.** Không suy diễn browser preview thành bằng chứng Tauri IPC, filesystem persistence hoặc native app chạy được. Bước tiếp theo là cài MSVC Build Tools, chạy build/test/lint và checklist native Windows, sau đó kiểm chứng trên macOS.
