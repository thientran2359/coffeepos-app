# Phase 6.2 — Hiệu năng runtime và độ mượt

Phase 6.2 xử lý bottleneck đã xuất hiện sau khi POS thật, auto-start, shutdown drain và health diagnostics cùng chạy trên runtime local. Mục tiêu là làm Desktop vẫn phản hồi trong lúc service start/restart/health, đồng thời bỏ giới hạn xử lý request tuần tự của PHP built-in server trước khi CoffeePOS đi sang LAN/release.

## Baseline đã quan sát

Trên development runtime Windows x64 hiện tại ngày 2026-09-18:

- WordPress/POS đang được serve bằng `php.exe -S 127.0.0.1:<port>` với một PHP process; `PHP_CLI_SERVER_WORKERS` không dùng được trên Windows.
- `php.ini` staged chưa bật OPcache cho CLI web server.
- frontend gọi `get_runtime_info` mỗi 2 giây; `RuntimeManager::refresh()` có thể kích hoạt CoffeePOS machine-health theo interval 5 giây.
- `start_runtime`, `stop_runtime`, `restart_runtime` và health commands hiện đi qua native command đồng bộ trong khi bên dưới có process wait, socket I/O và readiness loop.
- Đo trực tiếp cùng runtime đang chạy cho thấy `/wp-login.php` khoảng `0.88–0.90s` ở các request thường và có mẫu gần `1.96s`; file JS tĩnh cùng origin chỉ khoảng `2–7ms`. Điều này chỉ ra bottleneck chính nằm trên đường PHP → WordPress/WooCommerce/CoffeePOS thay vì static file I/O.

Các số trên là baseline của máy development hiện tại để so sánh trước/sau trên cùng máy, không phải SLA cố định cho mọi máy người dùng.

## Mục tiêu

1. Desktop UI không bị giữ bởi lifecycle/readiness/health I/O.
2. Poll trạng thái thường xuyên chỉ đọc process/runtime state rẻ; không tạo WordPress/CoffeePOS request định kỳ một cách ngầm định.
3. Health application chạy có kiểm soát, không overlap và không tranh request path của POS với tần suất cao.
4. PHP bytecode cache được bật và xác minh trên runtime phục vụ web.
5. Runtime Windows có request concurrency thật trước LAN/release; `php -S` chỉ còn là development/fixture path nếu vẫn cần cho test nhỏ.
6. Giữ nguyên security, dynamic port, drain/shutdown, crash containment, provisioning và machine-health contracts đã có.

## Phạm vi implementation

### 1. Tách fast status khỏi application health

`get_runtime_info` phải chỉ refresh state của managed child/process và trả snapshot đã cache. Nó không được tự gọi WordPress/CoffeePOS HTTP endpoint chỉ vì frontend poll.

Application health được cập nhật theo hai đường:

- bắt buộc sau start/restart và khi người dùng bấm **Kiểm tra lại**;
- background health scheduler riêng khi runtime stable, có minimum interval hợp lý, không overlap request trước và dừng ngay khi lifecycle transition bắt đầu.

Full component diagnostics của Phase 6.1 vẫn là action riêng trong **Hệ thống → Chẩn đoán**; poll shell không biến thành diagnostics ngầm.

### 2. Không block Tauri UI thread

Các command có process wait, filesystem I/O, TCP/HTTP probe hoặc shutdown drain phải chạy ngoài UI/main thread bằng async command + blocking worker phù hợp. Lifecycle/provisioning mutex hiện có vẫn là nguồn serialize operation; thay đổi scheduling không được cho phép hai start/stop/restart chạy cùng lúc.

Trong lúc auto-start hoặc restart đang diễn ra, cửa sổ phải vẫn resize được, navigation **Tổng quan / Cấu hình / Hệ thống** vẫn phản hồi và trạng thái progress vẫn render được.

### 3. Bật và xác minh OPcache

Runtime PHP config phải bật OPcache cho SAPI thực tế dùng để serve WordPress. Với path CLI còn tồn tại, phải kiểm `opcache.enable_cli` thay vì chỉ thêm extension nhưng không active cache.

Staging/doctor phải xác minh module/config từ chính bundled PHP. Không đọc `php.ini` hoặc extension từ máy người dùng.

### 4. Chuyển production serving sang mô hình concurrent

Phase này chốt và triển khai một web-serving stack local có request concurrency trên Windows, dùng artifact pin/version/hash/license như các runtime component khác. Kiến trúc mục tiêu là web server local + FastCGI/PHP worker model hoặc giải pháp tương đương đã chứng minh tương thích WordPress/WooCommerce/CoffeePOS.

Trước khi code phần này, ghi ngắn quyết định chọn server/worker trong `ARCHITECTURE.md`, gồm:

- footprint và khả năng bundle offline;
- cách spawn/track nhiều process trên Windows;
- loopback bind và dynamic HTTP port;
- FastCGI/PHP worker count mặc định và giới hạn tài nguyên;
- static file serving, permalink/router behavior và request size/timeouts;
- compatibility với WordPress admin, REST, uploads và CoffeePOS routes;
- graceful drain và forced cleanup;
- license và đường nâng cấp macOS sau baseline Windows.

Không thêm Nginx/Apache/Caddy hay một server khác chỉ theo sở thích. Lựa chọn phải dựa trên benchmark + lifecycle/security contract của CoffeePOS Desktop. Kết quả bắt buộc của Phase 6.2 là runtime dùng cho POS không còn phụ thuộc vào single-process `php -S`.

### 5. Giữ đúng lifecycle và shutdown semantics

Phase 5.6 admission gate phải được thiết kế lại cho mô hình nhiều worker sao cho:

- request mới bị từ chối khi shutdown bắt đầu;
- request đã được nhận có bounded drain;
- không worker/web-server process nào bị bỏ mồ côi;
- Windows Job Object bao phủ các process mới;
- start sau crash dọn stale drain state an toàn;
- MariaDB chỉ shutdown sau khi web/PHP request handling đã dừng theo contract.

Không được suy luận drain hoàn tất từ một probe chỉ đứng sau một queue duy nhất như với `php -S`; worker pool phải có readiness/drain contract riêng.

### 6. Chỉ tune MariaDB/Plugin khi có bằng chứng

Sau các thay đổi trên, benchmark lại POS/WordPress. Chỉ thêm MariaDB config hoặc tối ưu CoffeePOS endpoint nếu profile cho thấy DB/query/plugin còn là bottleneck đáng kể. Không đưa tuning tùy đoán vào runtime baseline.

## Non-goals

- LAN bind/public network: Phase 8.
- Repair file/config/plugin: Phase 6.3.
- Log viewer/export support bundle: Phase 6.4.
- Installer/runtime production packaging hoàn chỉnh: Phase 9.
- Viết lại POS frontend hoặc business logic sang Rust/Desktop.
- Object cache/Redis hoặc service mới nếu chưa có profile chứng minh nhu cầu.

## Acceptance

Ưu tiên focused/lightweight automated checks; performance và cảm giác sử dụng cuối cùng được người dùng manual smoke-test trên app thật.

1. Start/restart runtime trong app thật: cửa sổ vẫn resize, đổi navigation và repaint progress bình thường; không có freeze do native lifecycle command giữ UI thread.
2. Poll `get_runtime_info` trong trạng thái Running không tạo HTTP request tới WordPress/CoffeePOS và không spawn cron ngoài cadence đã định.
3. Background health không overlap chính nó; lifecycle start/stop/restart có thể chặn/cancel lần health kế tiếp mà không deadlock.
4. Bundled PHP xác nhận OPcache active trong web-serving SAPI; restart giữ behavior đúng và không lấy config global.
5. So benchmark cùng máy, cùng store, warm cache trước/sau. Ghi p50/p95 cho một request động đại diện (`/wp-login.php` hoặc endpoint POS tương đương) và một static asset. Dynamic warm p50 phải cải thiện ít nhất khoảng 2× so baseline hiện tại hoặc có bằng chứng tương đương từ POS flow nếu endpoint benchmark thay đổi do server migration.
6. Chạy ít nhất 4 request động đồng thời. Tổng thời gian không được tăng gần tuyến tính như single-worker queue; log/process evidence phải chứng minh request được xử lý concurrent.
7. POS thật: mở cashier, load catalog/API, tìm sản phẩm/thêm giỏ và submit một đơn test không bị health/status polling tạo hàng chờ dài phía sau request unrelated.
8. Shutdown khi có request đang chạy: admission gate, bounded drain, forced-cleanup fallback và MariaDB stop vẫn đúng; sau exit không còn web/PHP/MariaDB child do Desktop quản lý.
9. Restart/relaunch giữ dynamic URL, WordPress login/session behavior và provisioning data hiện có; không reinstall/reset credential.
10. Focused Rust/TypeScript checks, runtime staging/doctor liên quan và `git diff --check` pass. Không cần rerun toàn bộ staged provisioning E2E nếu thay đổi không chạm provisioning contract.

## Definition of Done

Phase 6.2 chỉ được đánh dấu hoàn thành khi có cả bốn bằng chứng: Desktop responsive trong lifecycle, status/health scheduling không tranh request vô ích, OPcache thực sự active, và POS runtime xử lý request concurrent bằng serving stack đã pin/managed. Chỉ đổi timeout hoặc giảm polling mà vẫn giữ `php -S` single-process không đủ để hoàn thành phase.
