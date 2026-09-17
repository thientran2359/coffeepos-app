# Development

## Windows

1. Cài [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/), chọn **Desktop development with C++**, MSVC x64/x86 và Windows SDK.
2. Cài Rust stable bằng [rustup](https://rustup.rs/), default host `x86_64-pc-windows-msvc`.
3. Cài Node phiên bản phù hợp `engines` trong package.json; máy khảo sát dùng 22.16.0.
4. Cần [WebView2 Runtime](https://developer.microsoft.com/en-us/microsoft-edge/webview2/); máy khảo sát đã có. Mở lại terminal sau cài toolchain.

Không dùng PHP/MariaDB trong LocalWP làm dependency desktop. Phase 1 không cần hai runtime này.

## macOS

```sh
xcode-select --install
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Cài Node/npm, mở terminal mới. Chạy build native trên máy Mac cho kiến trúc tương ứng; không coi build Windows là bằng chứng chạy macOS. Homebrew không phải dependency ứng dụng. Full Xcode/signing identities có thể cần cho quy trình distribution; Phase 1 local shell chưa cần Apple Developer account.

## Lệnh dự án

```sh
npm ci
npm run doctor
npm run dev
```

`dev` dùng Vite loopback 1420 với strictPort; đây chỉ là cổng frontend development, không phải cổng WordPress. Nếu cổng này bận, đóng Vite instance cũ hoặc đổi đồng thời vite.config.ts, devUrl và devCsp. Runtime port sẽ được cấp riêng trong Phase 2.

```sh
npm run build:ui
npm run build
npm run package
npm run lint
npm test
```

`build` tạo native executable; `package` chỉ bundle shell trên máy hiện tại (chưa có POS runtime, offline prerequisites hoặc release signing). npm và Cargo lockfile đã có, build/test dùng `--locked`. Commit cả hai lockfile khi đưa dự án vào Git.

Phiên khảo sát đã cài riêng Rust 1.98.1 + rustfmt trong `.tools/` (Git ignored), không đổi PATH hệ thống. Có thể dùng lại trong PowerShell hiện tại:

```powershell
. ./scripts/use-local-rust.ps1
rustup component add clippy
npm run doctor
```

Vẫn cần cài MSVC Build Tools theo mục Windows trước khi native build/test. Những máy khác nên dùng rustup thông thường; `.tools/` không được phân phối cùng sản phẩm.

`npm run dev:ui` xem UI bằng trình duyệt. Banner preview nói rõ không có native persistence; không thay bằng fake success. Native shell mới đọc/ghi dữ liệu thật.

## Dữ liệu và lỗi

Rust gọi `app.path().app_local_data_dir()`:

- Windows: `%LOCALAPPDATA%/com.coffeepos.desktop/`
- macOS: `~/Library/Application Support/com.coffeepos.desktop/`

`config/app.json` giữ store label và local bind host. Đổi tên → lưu → đóng/mở lại để kiểm persistence. Không xóa data directory để sửa lỗi startup. Cấu hình hỏng được giữ nguyên: copy lưu lại rồi sửa JSON/restore bản đúng và bấm Thử lại.

Nếu instance khác giữ `desktop.lock`, đóng instance đó rồi retry. Sự tồn tại của lock file không đồng nghĩa bị khóa: OS tự nhả khóa khi process thoát. Không xóa lock file trong lúc process đang chạy.

Log Phase 1: `logs/application.log`, chỉ sự kiện không chứa secret. Log write là best-effort. Không có PHP/DB child process ở phase này.

## Native CI / acceptance

Workflow `.github/workflows/check.yml` chuẩn bị build/test trên Windows + macOS arm64/x64. Workflow chưa chạy khi chưa push repo. CI compile không thay cho native launch/close/relaunch và thao tác UI thật; checklist ở PHASE-01.md.
