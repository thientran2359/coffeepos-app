# Runtime — hợp đồng Phase 2 (chưa triển khai)

Nguồn sự thật kiến trúc: [ARCHITECTURE.md](../ARCHITECTURE.md).

- Runtime production resolve từ Tauri resource directory + verified manifest; không lấy từ PATH/LocalWP/Homebrew.
- Development resolve qua cấu hình developer riêng, đường dẫn tuyệt đối bên `runtime/development/`; không yêu cầu PHP/DB global.
- Manifest theo target: `x86_64-pc-windows-msvc`, `aarch64-apple-darwin`, `x86_64-apple-darwin`. Mỗi artifact pin version/hash/source/license và dependent DLL/dylib.
- PHP candidate 8.4 NTS trên Windows; test mysqli, curl, openssl, mbstring, intl, zip, gd, fileinfo, DOM/XML và extension requirements của exact WP/WC/plugin bundle. Pin php.ini/CA trust, không kế thừa cấu hình PHP máy người dùng.
- MariaDB candidate 11.4; explicit defaults-file, basedir/datadir/port/bind-address, minimal privileges và credential riêng. Không đọc my.ini/my.cnf global. Windows init utility và macOS init scripts phải được thử trên bundle thật.
- MariaDB readiness là authenticated SQL probe; PHP readiness là response nhận diện đúng instance; application health do plugin xác nhận.
- PHP document root là `site/`; router script thuộc desktop layer phải xử lý permalink, static files, path traversal và chặn file nhạy cảm, không sửa WP core.
- Mỗi start attempt có timeout, stderr redaction, cleanup toàn bộ child đã tạo. Shutdown/crash/parent-kill phải test riêng trên từng OS.
- Log tách application/runtime/php/database/wordpress, có size bounds và export redacted.

Trước khi sang provisioning phải chứng minh bundle chạy trên máy không có LocalWP/PHP/MariaDB dev tools. Trên macOS kiểm tra `otool -L`, rpath và dependencies để loại đường dẫn `/opt/homebrew` hoặc build-machine paths.
