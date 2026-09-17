# CoffeePOS Desktop — Kiến trúc

Ngày khảo sát: 2026-09-17. Trạng thái: thiết kế nền tảng + mã nguồn Phase 1; runtime Phase 2–8 chưa triển khai.

## 1. Mục tiêu và ranh giới

Người vận hành cài một ứng dụng, mở cửa hàng cục bộ mà không tự cấu hình PHP, database hoặc WordPress. Desktop là lớp phân phối/vận hành, không thay thế plugin.

```text
Bundled shell UI (main, trusted)
        │ typed Tauri commands
        ▼
Rust: cấu hình → runtime lifecycle → provisioning → health → backup
        │ explicit executable paths + argument arrays
        ├── MariaDB (loopback only)
        └── PHP HTTP server → WordPress → WooCommerce → CoffeePOS
                                                    │
                                       POS WebView / thiết bị LAN
```

CoffeePOS/WooCommerce sở hữu sản phẩm, giỏ hàng, thanh toán, đơn, khách hàng, báo cáo, ca và REST API. Desktop chỉ quản lý tiến trình, file, cấu hình môi trường, sức khỏe, backup và cập nhật. Không sửa WordPress core. Không sao chép nghiệp vụ sang Rust.

## 2. Khảo sát môi trường

### Máy Windows hiện tại — kiểm tra trực tiếp

| Thành phần | Kết quả |
| --- | --- |
| OS | Windows, kernel 10.0.26200, x64 |
| PowerShell | 7.6.5 |
| Node / npm | 22.16.0 / 10.9.2 |
| WebView2 | 153.0.4234.32, registry xác nhận |
| Rust / Cargo | Không có trong PATH; không thấy Cargo ở vị trí rustup mặc định |
| MSVC | Không thấy vswhere hoặc cl; thư mục Visual Studio 2019 không có bộ build có thể dùng được qua kiểm tra hiện tại |
| Windows SDK | Có thư mục Windows Kits/10; chưa xác minh đủ thành phần build |
| PHP | PATH trỏ vào LocalWP PHP 8.2.27; không dùng làm runtime sản phẩm |
| MariaDB | Không thấy mariadbd trong PATH |
| Repository | Ban đầu chỉ có AGENTS.md; chưa có .git |

Không suy luận rằng app native chạy được chỉ vì WebView2 có mặt. macOS không có host trong phiên này; đánh giá macOS dựa tài liệu chính thức, không phải kết quả chạy thực tế.

### Ma trận thiết kế

| Chủ đề | Windows | macOS |
| --- | --- | --- |
| Mục tiêu ban đầu | Windows 11 x64 | macOS 13+; arm64 và x86_64 tách artifact |
| Development | Rust MSVC, Visual Studio C++ Build Tools + Windows SDK, Node/npm | Rust, Xcode Command Line Tools, Node/npm; Xcode đầy đủ khi công cụ ký/build cần |
| WebView | WebView2 | WKWebView của hệ điều hành |
| Người dùng cuối | Installer đảm bảo WebView2 và các VC runtime cần thiết | App đã ký/notarize, không cần Homebrew |
| PHP | Gói Windows x64 NTS, CLI + extensions/DLL cùng build | Build/stage riêng theo kiến trúc; không dùng PHP hệ thống |
| MariaDB | ZIP/binary bundle; init datadir riêng, không cài service hệ thống | Bundle được build/stage và kiểm định riêng theo kiến trúc |
| Dữ liệu | `%LOCALAPPDATA%/com.coffeepos.desktop/` | `~/Library/Application Support/com.coffeepos.desktop/` |
| Đóng gói cuối | NSIS/MSI, ký mã | .app/.dmg, ký nested binaries và notarize |

Các mốc OS trên là lựa chọn sản phẩm ban đầu, chưa phải ma trận đã chứng nhận. Chỉ mở rộng Windows ARM hoặc OS cũ khi mọi runtime dependency được kiểm định. Tauri dùng WebView hệ thống; xem [prerequisites](https://v2.tauri.app/start/prerequisites/) và [WebView versions](https://v2.tauri.app/reference/webview-versions/). Đường dẫn thực tế lấy từ [app_local_data_dir](https://docs.rs/tauri/latest/tauri/path/struct.PathResolver.html), không ghép HOME/AppData bằng tay.

PHP Windows có [binary và dependency riêng](https://www.php.net/manual/en/install.windows.manual.php); [PHP trên macOS](https://www.php.net/manual/en/install.macosx.php) cần được cung cấp riêng cho app. MariaDB có [Windows ZIP](https://mariadb.com/docs/server/server-management/install-and-upgrade-mariadb/installing-mariadb/binary-packages/installing-mariadb-windows-zip-packages). Hướng dẫn [macOS Homebrew](https://mariadb.com/docs/server/server-management/install-and-upgrade-mariadb/installing-mariadb/binary-packages/installing-mariadb-on-macos-using-homebrew) và [macOS PKG](https://mariadb.com/docs/server/server-management/install-and-upgrade-mariadb/installing-mariadb/binary-packages/installing-mariadb-server-pkg-packages-on-macos) không chứng minh có bundle portable phù hợp với app. Vì vậy nguồn artifact macOS vẫn là việc phải kiểm chứng ở Phase 2; không lấy binary Linux tarball dùng cho macOS.

## 3. Các quyết định

1. Tauri 2 + Rust; frontend TypeScript thuần/Vite. Không cần React hoặc UI framework cho shell này.
2. Node/npm chỉ dùng phát triển/build frontend. App thành phẩm không gọi Node, Composer, PHP hoặc MariaDB trên PATH.
3. Phase 1 chỉ có `main.rs` (IPC/app wiring) và `config.rs` (data paths, validation, persistence). Không tạo hàng loạt module rỗng. Thêm runtime, health, provisioning khi vertical slice cần.
4. Bundle immutable đặt trong resources của app. Dữ liệu mutable luôn ở app-local-data. Development runtime nằm riêng `runtime/development/`, không trở thành dependency sản phẩm.
5. Default loopback; LAN chỉ mở ở Phase 7. Không mở database ra LAN, không port-forward/cloud tunnel.
6. Tiến trình dùng `std::process::Command` với đường dẫn tuyệt đối và từng argument; Rust giữ quyền spawn, không đưa shell API cho WebView.

### PHP built-in server: quyết định có điều kiện

Giữ `php -S` cho vertical slice theo AGENTS.md. [PHP manual](https://www.php.net/manual/en/features.commandline.webserver.php) xác nhận server mặc định một luồng xử lý request, workers không được hỗ trợ trên Windows, và server không dành cho production.

Hệ quả cần đo: WordPress loopback/self-request có thể bị kẹt khi cùng worker đang chờ; cron, plugin HTTP call vào chính site, nhiều màn KDS/customer display và request dài có thể chặn POS. Đây là rủi ro kiến trúc thực tế, không chỉ vấn đề đóng gói.

Trước LAN/release: đo concurrency, timeout, loopback, checkout và polling trên Windows/macOS. Nếu không đạt, trình bày bằng chứng và cập nhật quyết định kiến trúc trước khi thêm web server/worker model khác. Chưa đổi sang Nginx/Apache trong Phase 1. Chưa tuyên bố runtime production-ready.

## 4. File và cấu hình

```text
app resources/                 # immutable; future runtime bundles
  runtime/<target>/<version>/
  wordpress-template/

app_local_data_dir()/          # implemented directories in Phase 1
  desktop.lock                 # OS file lock held for native store lifetime
  config/app.json
  site/
  database/
  uploads/
  logs/application.log
  backups/
```

`com.coffeepos.desktop` là identifier khởi đầu; phải chốt trước phân phối vì đổi identifier làm đổi đường dẫn dữ liệu. Development hiện dùng cùng identifier; không chạy development với dữ liệu production sau này nếu chưa có profile tách biệt.

Schema Phase 1:

```json
{
  "schema_version": 1,
  "store_name": "My Coffee",
  "bind_host": "127.0.0.1"
}
```

`store_name` là nhãn setup dự kiến, không phải nguồn sự thật cho tên cửa hàng sau provisioning. Khi WordPress có mặt, tên cửa hàng do plugin trả về. Không lưu password/token trong file này.

Lần đầu tạo directory/default config; mở lại giữ nguyên. Parse/schema sai hoặc bind LAN trái phase thì báo lỗi, không reset. Save validate ở Rust, ghi tempfile cùng directory, flush rồi atomic replace; chỉ cập nhật in-memory sau save thành công. `fs2` khóa chéo platform; `tempfile` xử lý replacement thay vì tự triển khai khác nhau cho Windows/macOS. Mutex serialize IPC trong cùng process. Đây không phải cơ chế chống mất điện hoàn chỉnh hoặc backup database.

Log Phase 1 chỉ ghi timestamp và sự kiện cố định; không ghi tên cửa hàng, credential hoặc cấu hình. Log là best-effort, chưa có export/rotation. Phase 2 phải thêm bounded logs và diagnostics cho child processes.

## 5. Lifecycle, ports và readiness — Phase 2+

State machine dự kiến: `not_installed → installing → stopped → starting → running → stopping → stopped`; lỗi kèm component, operation, recovery. `running` chỉ sau tất cả readiness đạt, không sau spawn.

Startup giữ installation lock, kiểm manifest, mở database hiện có, spawn MariaDB, authenticated SQL readiness, spawn PHP, HTTP readiness rồi plugin health. Mỗi bước có timeout/cancellation. Bước sau lỗi phải dọn tiến trình đã tạo trong lần start đó. Crash dùng bounded retry/backoff, không restart loop vô hạn và không tự init lại database.

Shutdown chặn start mới, đóng request mới, dừng PHP rồi graceful database shutdown, đợi exit; timeout mới dùng terminate. Windows cần Job Object để kill descendants khi parent crash, ẩn console. macOS cần process group, SIGTERM/deadline/SIGKILL và cơ chế phát hiện parent mất; process group riêng tự nó không đảm bảo không orphan. Test kill/crash trước khi chốt implementation.

Phase 1 không chọn HTTP/database port vì chưa có service. Phase 2 thử port đã lưu, kiểm tra availability; nếu bận, chọn loopback ephemeral port, spawn có bounded retry cho race giữa probe và bind. Persist selected port sau readiness. Không chỉ kiểm TCP để xác nhận đúng service.

URL WordPress phải cập nhật có kiểm soát theo actual host/port trước health. Không search-replace tùy tiện serialized database. Lưu DB port riêng, luôn loopback. LAN canonical host, cookie scope và URL changes là hợp đồng Phase 7. Browser CORS không phải cơ chế xác thực.

## 6. Provisioning và phiên bản

Xem [PROVISIONING.md](docs/PROVISIONING.md). Bundle manifest phải có exact Desktop/runtime/PHP/MariaDB/WordPress/WooCommerce/CoffeePOS/schema versions, target triple, artifact hash, nguồn và license notices. Không dùng tên archive để suy luận compatibility; không tải “latest” khi mở app.

Ứng viên khảo sát: PHP 8.4 và MariaDB 11.4; đây là hướng thử nghiệm, chưa pin hoặc cam kết compatibility. Exact WordPress/WooCommerce/CoffeePOS phải chọn từ bộ đã chạy acceptance thực tế; không giả định plugin hiện có tương thích chỉ dựa header. Đối chiếu [WordPress requirements](https://wordpress.org/about/requirements/), [WooCommerce requirements](https://woocommerce.com/document/server-requirements/) và [PHP support](https://www.php.net/supported-versions.php) khi khóa release.

Frontend có npm lockfile. Rust Cargo.toml dùng major constraints và Cargo.lock được tạo bằng Rust 1.98.1 trong toolchain cục bộ `.tools/`. Build/test dùng `--locked`; lockfile phải được đưa vào Git và qua native CI trước milestone acceptance.

## 7. Security và WebView

Shell bundled `main` có đúng hai command: đọc shell info và đổi store label. Commands đưa vào `AppManifest::commands`, capability chỉ cấp cho `main`; không có remote origins, filesystem/shell plugins hoặc arbitrary path inputs. CSP hạn chế script và IPC; dev CSP cho Vite/HMR tại loopback. Theo [Tauri capabilities](https://v2.tauri.app/security/capabilities/), app commands cần explicit manifest nếu muốn capability kiểm soát.

Phase 5 tạo POS WebView riêng, không cấp native management capability cho HTTP WordPress. Rust chỉ cho navigation tới đúng local origin đã được xác minh; external links phải có handling riêng. Không nạp WP content vào shell có quyền quản lý. Cookie staff/member do plugin sở hữu.

Provisioning tạo admin credential từ user input hoặc secure randomness, không log, không đưa password vào command line. Bí mật cần persist dùng Windows credential protection/macOS Keychain; wp-config/db client file cần quyền OS hạn chế. Health token không xuất hiện trên UI hoặc URL.

## 8. Backup, updates và phân phối

Database logical dump + uploads + cấu hình + version metadata; xem [BACKUP.md](docs/BACKUP.md). Không coi copy live datadir là backup portable. Desktop/runtime update và plugin/schema update riêng, có preflight/backup/migration/health và rollback phù hợp schema. Tuyệt đối không ghi đè user data từ template mới.

Phase 8 mới tối ưu installer. Windows phải hỗ trợ cài WebView2 offline nếu mục tiêu là fresh-machine offline; macOS phải ký cả executables/dylibs bên trong trước app và notarization. Theo [Windows installer](https://v2.tauri.app/distribute/windows-installer/) và [macOS bundle](https://v2.tauri.app/distribute/macos-application-bundle/). Skeleton package hiện chỉ chứa shell; chưa chứa runtime, signing hoặc offline prerequisite payload.

## 9. Roadmap và release gates

| Phase | Slice chạy được | Gate chính |
| --- | --- | --- |
| 1 | Desktop + config + app data | Launch/close/relaunch Windows và macOS, config giữ nguyên, lỗi đọc/ghi rõ ràng |
| 2 | MariaDB → PHP → HTTP | Start/stop/restart/failure, không orphan, binary portable từng platform |
| 3 | WordPress cục bộ | Fresh install, retry, giữ dữ liệu cũ |
| 4 | WooCommerce + CoffeePOS | Activation, health contract, POS route thật |
| 5 | POS trong WebView | Login/navigation/cookie, remote content không có IPC quản lý |
| 6 | Backup/restore | Validate trước replace, phục hồi khi restore lỗi |
| 7 | LAN opt-in | Reachability, firewall, auth, concurrency + loopback |
| 8 | Installer/app | Fresh OS không dev tools, signing, upgrade giữ store |

Không gộp “code xong”, “build được”, “chạy được” và “đủ điều kiện release” thành một trạng thái. Bằng chứng Phase 1 theo [PHASE-01.md](docs/PHASE-01.md).
