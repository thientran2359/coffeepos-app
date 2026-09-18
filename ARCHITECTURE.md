# CoffeePOS Desktop — Kiến trúc

Ngày khảo sát: 2026-09-17. Trạng thái hiện tại: Phase 1–3 và Phase 4.1 Windows-first hoàn tất; native WordPress 7.1 provisioning và flow UI Install → Ready → restart Ready đã pass trên app thật. Mốc tiếp theo là Phase 4.2 — WordPress runtime UX. Roadmap chi tiết nằm tại [docs/ROADMAP.md](docs/ROADMAP.md).

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
                                       POS browser hoặc WebView / thiết bị LAN
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

PHP Windows có [binary và dependency riêng](https://www.php.net/manual/en/install.windows.manual.php); [PHP trên macOS](https://www.php.net/manual/en/install.macosx.php) cần được cung cấp riêng cho app. MariaDB có [Windows ZIP](https://mariadb.com/docs/server/server-management/install-and-upgrade-mariadb/installing-mariadb/binary-packages/installing-mariadb-windows-zip-packages). Hướng dẫn [macOS Homebrew](https://mariadb.com/docs/server/server-management/install-and-upgrade-mariadb/installing-mariadb/binary-packages/installing-mariadb-on-macos-using-homebrew) và [macOS PKG](https://mariadb.com/docs/server/server-management/install-and-upgrade-mariadb/installing-mariadb/binary-packages/installing-mariadb-server-pkg-packages-on-macos) không chứng minh có bundle portable phù hợp với app. Vì vậy nguồn artifact macOS vẫn phải được kiểm chứng riêng trước khi mở rộng acceptance sang macOS; không lấy binary Linux tarball dùng cho macOS.

## 3. Các quyết định

1. Tauri 2 + Rust; frontend TypeScript thuần/Vite. Không cần React hoặc UI framework cho shell này.
2. Node/npm chỉ dùng phát triển/build frontend. App thành phẩm không gọi Node, Composer, PHP hoặc MariaDB trên PATH.
3. Phase 1 chỉ có `main.rs` (IPC/app wiring) và `config.rs` (data paths, validation, persistence). Không tạo hàng loạt module rỗng. Thêm runtime, health, provisioning khi vertical slice cần.
4. Bundle immutable đặt trong resources của app. Dữ liệu mutable luôn ở app-local-data. Development runtime nằm riêng `runtime/development/`, không trở thành dependency sản phẩm.
5. Default loopback; LAN chỉ mở ở Phase 8.x. Không mở database ra LAN, không port-forward/cloud tunnel.
6. Tiến trình dùng `std::process::Command` với đường dẫn tuyệt đối và từng argument; Rust giữ quyền spawn, không đưa shell API cho WebView.

### PHP built-in server: requirement thay thế đã được chứng minh

`php -S` đã phù hợp để đưa vertical slice ban đầu lên chạy theo AGENTS.md. [PHP manual](https://www.php.net/manual/en/features.commandline.webserver.php) xác nhận server mặc định một luồng xử lý request, workers không được hỗ trợ trên Windows, và server không dành cho production.

Hệ quả cần đo: WordPress loopback/self-request có thể bị kẹt khi cùng worker đang chờ; cron, plugin HTTP call vào chính site, nhiều màn KDS/customer display và request dài có thể chặn POS. Đây là rủi ro kiến trúc thực tế, không chỉ vấn đề đóng gói.

Sau khi POS thật, auto-start và health polling cùng hoạt động, requirement thay thế đã xuất hiện: benchmark 2026-09-18 trên development runtime cho `/wp-login.php` khoảng `0.88–0.90s` với mẫu gần `1.96s`, trong khi static JS cùng origin chỉ khoảng `2–7ms`; runtime Windows vẫn có một PHP request worker. Phase 6.2 vì vậy phải chốt và triển khai local web-serving/FastCGI worker model có concurrency, đồng thời bật OPcache và tách health/status scheduling. `php -S` có thể tiếp tục phục vụ fixture/test nhỏ nhưng không còn là serving architecture mục tiêu cho POS trước Phase 8 LAN hoặc Phase 9 release. Xem [PHASE-06.2.md](docs/PHASE-06.2.md).

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

Phân biệt installation state (`not_installed / installing / ready / needs_repair`), runtime lifecycle (`stopped / starting / running / stopping` cùng trạng thái lỗi native) và application health. Đây là các trục trạng thái liên quan, không phải một enum tuyến tính chung. `provisioning.ready` có thể đi cùng runtime stopped. Runtime `running` chỉ xác nhận readiness DB/PHP hiện có; WordPress/plugin health phải được kiểm riêng trước khi báo ứng dụng sẵn sàng. Phase 4.2 hợp nhất cách trình bày các trạng thái này, không tự đổi ý nghĩa native contract đã có.

Startup giữ installation lock, kiểm manifest, mở database hiện có, spawn MariaDB, authenticated SQL readiness, spawn PHP, HTTP readiness rồi plugin health. Mỗi bước có timeout/cancellation. Bước sau lỗi phải dọn tiến trình đã tạo trong lần start đó. Crash dùng bounded retry/backoff, không restart loop vô hạn và không tự init lại database.

Shutdown chặn start mới, đóng request mới, dừng PHP rồi graceful database shutdown, đợi exit; timeout mới dùng terminate. Windows cần Job Object để kill descendants khi parent crash, ẩn console. macOS cần process group, SIGTERM/deadline/SIGKILL và cơ chế phát hiện parent mất; process group riêng tự nó không đảm bảo không orphan. Test kill/crash trước khi chốt implementation.

Phase 1 không chọn HTTP/database port vì chưa có service. Phase 2 thử port đã lưu, kiểm tra availability; nếu bận, chọn loopback ephemeral port, spawn có bounded retry cho race giữa probe và bind. Persist selected port sau readiness. Không chỉ kiểm TCP để xác nhận đúng service.

URL WordPress phải cập nhật có kiểm soát theo actual host/port trước health. Không search-replace tùy tiện serialized database. Lưu DB port riêng, luôn loopback. LAN canonical host, cookie scope và URL changes là hợp đồng Phase 8.x. Browser CORS không phải cơ chế xác thực.

## 6. Provisioning và phiên bản

Xem [PROVISIONING.md](docs/PROVISIONING.md). Bundle manifest phải có exact Desktop/runtime/PHP/MariaDB/WordPress/WooCommerce/CoffeePOS/schema versions, target triple, artifact hash, nguồn và license notices. Không dùng tên archive để suy luận compatibility; không tải “latest” khi mở app.

Development runtime Windows hiện pin PHP 8.4.25, MariaDB 11.4.13 và WordPress 7.1; bộ này đã chạy Phase 3 real-runtime E2E. WooCommerce/CoffeePOS vẫn phải pin exact version và chạy acceptance thực tế trong Phase 4.x; không giả định compatibility chỉ dựa header. Đối chiếu [WordPress requirements](https://wordpress.org/about/requirements/), [WooCommerce requirements](https://woocommerce.com/document/server-requirements/) và [PHP support](https://www.php.net/supported-versions.php) khi khóa từng artifact/release.

Frontend có npm lockfile. Rust Cargo.toml dùng major constraints và Cargo.lock được tạo bằng Rust 1.98.1 trong toolchain cục bộ `.tools/`. Build/test dùng `--locked`; lockfile phải được đưa vào Git và qua native CI trước milestone acceptance.

## 7. Security và WebView

Shell bundled `main` hiện expose các typed commands cho shell/config, runtime lifecycle và WordPress provisioning: `get_shell_info`, `save_store_name`, `get_runtime_info`, `start_runtime`, `stop_runtime`, `restart_runtime`, `get_provisioning_info`, `provision_wordpress`. Commands đưa vào `AppManifest::commands`, capability chỉ cấp cho `main`; không có remote origins, filesystem/shell plugins hoặc arbitrary shell command input. CSP hạn chế script và IPC; dev CSP cho Vite/HMR tại loopback. Theo [Tauri capabilities](https://v2.tauri.app/security/capabilities/), app commands cần explicit manifest nếu muốn capability kiểm soát.

Phase 5.1–5.3 xây shell, setup và trang chính theo [UI-UX.md](docs/UI-UX.md). Phase 5.4 chốt host POS trước code: browser là hướng đề xuất MVP; WebView là lựa chọn riêng. Nếu dùng POS WebView thì phải tách riêng, không cấp native management capability cho HTTP WordPress. Rust chỉ cho navigation tới đúng local origin đã được xác minh; external links phải có handling riêng. Không nạp WP content vào shell có quyền quản lý. Cookie staff/member do plugin sở hữu.

Provisioning tạo admin credential từ user input hoặc secure randomness, không log, không đưa password vào command line. Bí mật cần persist dùng Windows credential protection/macOS Keychain; wp-config/db client file cần quyền OS hạn chế. Health token không xuất hiện trên UI hoặc URL.

## 8. Backup, updates và phân phối

Database logical dump + uploads + cấu hình + version metadata; xem [BACKUP.md](docs/BACKUP.md). Không coi copy live datadir là backup portable. Desktop/runtime update và plugin/schema update riêng, có preflight/backup/migration/health và rollback phù hợp schema. Tuyệt đối không ghi đè user data từ template mới.

Phase 9.x mới hoàn thiện runtime bundle/installer/distribution. Windows phải hỗ trợ cài WebView2 offline nếu mục tiêu là fresh-machine offline; macOS phải ký cả executables/dylibs bên trong trước app và notarization. Theo [Windows installer](https://v2.tauri.app/distribute/windows-installer/) và [macOS bundle](https://v2.tauri.app/distribute/macos-application-bundle/). Skeleton package hiện chỉ chứa shell; chưa chứa runtime, signing hoặc offline prerequisite payload.

## 9. Roadmap và release gates

Roadmap chi tiết và Definition of Done nằm tại [docs/ROADMAP.md](docs/ROADMAP.md). Tóm tắt các nhóm milestone:

| Phase | Slice chạy được | Gate chính |
| --- | --- | --- |
| 1 ✅ | Desktop + config + app data | Native Windows shell/config chạy được |
| 2 ✅ | MariaDB → PHP → HTTP | Start/stop/restart/failure, không orphan |
| 3 ✅ | WordPress cục bộ | Fresh install, retry/idempotency, giữ dữ liệu cũ |
| 4.1–4.11 ✅ / 4.12 | Setup → WooCommerce → CoffeePOS | Installation/health/idempotency đã pass Windows-first; còn interruption recovery |
| 5.1–5.6 | UI/UX và mở bán hàng | Điều hướng, setup/tài khoản, trang chính, POS/login, startup và shutdown |
| 6.1–6.3 | Diagnostics/repair | Component health, safe repair, support logs |
| 7.1–7.4 | Backup/restore | Portable validated backup và restore E2E |
| 8.1–8.3 | LAN opt-in | Reachability, auth, DB/native control vẫn private |
| 9.1–9.4 | Distribution | Bundled runtime, installer, fresh-machine và upgrade safety |

Không gộp “code xong”, “build được”, “chạy được” và “đủ điều kiện release” thành một trạng thái. Mỗi subphase phải có runnable evidence phù hợp với scope trước khi đánh dấu hoàn thành. Bằng chứng hiện tại theo [PHASE-01.md](docs/PHASE-01.md), [PHASE-02.md](docs/PHASE-02.md) và [PHASE-03.md](docs/PHASE-03.md).
