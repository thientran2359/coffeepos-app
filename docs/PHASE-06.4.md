# Phase 6.4 — Log viewer/export

Phase 6.4 bổ sung **Hệ thống → Nhật ký** để người vận hành xem log chẩn đoán ngay trong CoffeePOS Desktop và xuất một support bundle có giới hạn, đã loại bỏ credential trước khi rời khỏi máy. Đây là công cụ chẩn đoán của Desktop/runtime; không mở database, uploads, dữ liệu đơn hàng hoặc WordPress admin cho frontend.

> **Trạng thái implementation 2026-09-19:** native log catalog/read/export, fail-closed protected-secret redaction, support bundle schema 1 + Windows Save As và UI **Hệ thống → Nhật ký** đã được triển khai. Focused Rust logs tests 11/11 + cargo check/fmt + UI lint/build pass; manual Windows Save As/export + interaction acceptance vẫn chờ người dùng smoke-test nên phase chưa được đánh dấu hoàn thành theo Definition of Done.

## Mục tiêu

Phase 6.4 cần giải quyết bốn việc:

1. Người dùng có thể mở **Hệ thống → Nhật ký** và đọc log theo nhóm dễ hiểu mà không phải vào AppData hoặc terminal.
2. Viewer chỉ nhận text đã qua redaction từ native; raw credential không được đi qua IPC chỉ để hiển thị log.
3. Người dùng có thể xuất một file support bundle chứa đủ runtime/provisioning evidence để chẩn đoán lỗi phổ biến mà không kèm credential, database, uploads hoặc dữ liệu nghiệp vụ.
4. Xem/export log không được start/stop/restart runtime, chạy repair, thay đổi provisioning journal hoặc mutate store.

Phase này không biến Desktop thành hệ thống observability đầy đủ. Viewer ưu tiên sự cố setup/runtime/health/repair gần nhất và giữ thao tác đơn giản.

## Ranh giới ownership

Desktop sở hữu:

- discovery các file log do CoffeePOS Desktop/runtime/provisioning tạo;
- đọc log theo allowlist;
- paging/tail bounded;
- redaction trước IPC và trước export;
- metadata chẩn đoán của Desktop/runtime;
- tạo support bundle cục bộ;
- native Save As/export lifecycle.

CoffeePOS/WooCommerce tiếp tục sở hữu business/domain data. Phase 6.4 không query order/customer/payment tables để “làm log rõ hơn”, không dump database và không đọc uploads.

## Nguồn log

Native dùng allowlist cố định. Không nhận filename/path tùy ý từ frontend.

| log_id | File managed | Nhãn UI | Nguồn |
| --- | --- | --- | --- |
| application | logs/application.log | Ứng dụng | shell/config/lifecycle event của Desktop |
| runtime | logs/runtime.log | Runtime | runtime manager event |
| web_server | logs/web-server.log | Web server | Caddy lifecycle/output |
| php | logs/php.log | PHP | PHP FastCGI stdout/stderr |
| database | logs/database.log | Database | MariaDB stdout/stderr |
| cron | logs/cron.log | Tác vụ nền | WordPress cron/background worker |
| provisioning | logs/provisioning.log | Thiết lập | database/site/WordPress provisioning event |
| wordpress | logs/wordpress.log | WordPress | optional managed WordPress bootstrap log nếu tồn tại |
| woocommerce | logs/woocommerce.log | WooCommerce | WooCommerce activation/setup |
| coffeepos | logs/coffeepos.log | CoffeePOS | plugin activation/bootstrap/repair-related local bootstrap output |

File trong allowlist chưa tồn tại là trạng thái bình thường: catalog trả exists=false, viewer hiển thị **Chưa có dữ liệu**. Không tạo file chỉ vì người dùng mở Nhật ký.

Không scan toàn bộ logs/ rồi tự đưa file lạ lên UI. File ngoài allowlist có thể đến từ plugin/dev tool khác và chưa có contract privacy.

## Native contract

Phase 6.4 bổ sung ba capability chính. Tên Rust/Tauri command cụ thể có thể điều chỉnh theo convention hiện tại, nhưng payload/authority phải giữ contract dưới đây.

### 1. Catalog

~~~text
get_log_catalog() -> LogCatalog
~~~

Conceptual payload:

~~~json
{
  "generated_at": 0,
  "logs": [
    {
      "id": "runtime",
      "label": "Runtime",
      "exists": true,
      "size_bytes": 18432,
      "modified_at": 0
    }
  ]
}
~~~

Frontend chỉ nhận log_id, nhãn và metadata cần cho UI. Không cần trả absolute filesystem path.

Catalog là read-only và không giữ lifecycle mutex lâu. Nó không chạy health probe hoặc runtime maintenance chỉ để cập nhật màn hình log.

### 2. Read page/tail

~~~text
read_log_page({
  log_id,
  cursor,
  direction,
  max_lines
}) -> LogPage
~~~

Yêu cầu:

- log_id phải resolve qua allowlist native;
- mặc định mở ở tail mới nhất;
- mỗi response bounded theo cả số dòng và byte, target ban đầu tối đa khoảng 200 dòng hoặc 64 KiB text sau redaction;
- cursor là opaque token do native tạo, không phải byte offset/path frontend tự điều khiển;
- không load toàn bộ file lớn vào RAM để trả qua IPC;
- invalid/stale cursor trả lỗi đọc có thể retry, không fallback sang path do frontend truyền;
- viewer có thể **Tải dòng cũ hơn**; Phase 6.4 không cần live-stream từng dòng.

Conceptual payload:

~~~json
{
  "log_id": "runtime",
  "lines": [
    "1726720000 [event] runtime start requested"
  ],
  "older_cursor": "opaque",
  "has_older": true,
  "truncated": false,
  "redaction_count": 0
}
~~~

lines đã được redaction trong native. Raw bytes không được trả song song trong field debug.

### 3. Export support bundle

~~~text
export_support_bundle(destination_from_native_save_dialog) -> SupportBundleResult
~~~

Export chỉ bắt đầu sau khi người dùng bấm **Xuất gói hỗ trợ** và chọn vị trí lưu qua native Save As flow. Frontend không được tự gửi arbitrary source directory/file list cho native.

Tên file mặc định:

~~~text
CoffeePOS-support-YYYYMMDD-HHMMSS.zip
~~~

Nếu người dùng cancel Save As, command trả trạng thái cancelled; UI không báo lỗi.

Export chạy ngoài UI thread. Hai lần bấm liên tiếp không tạo hai export song song từ cùng màn hình; action disable trong lúc đang tạo bundle.

## Redaction contract

Redaction là boundary bắt buộc của Phase 6.4 và có **hai lớp**.

### Lớp 1 — khi ghi log

Giữ cơ chế hiện có:

- runtime stdout/stderr đi qua redact_log_text() trước khi append;
- log runtime có size bound;
- event log của application/provisioning không được format credential/config secret vào message;
- command arguments không được chứa plaintext secret khi implementation hiện tại đã dùng env/stdin/protected state.

Phase 6.4 phải rà lại các writer provisioning/plugin đang redirect stdout/stderr trực tiếp vào file. Writer nào có thể emit credential phải được chuyển qua redacting writer/pump hoặc chứng minh output script không thể in secret.

### Lớp 2 — khi đọc/export

Mọi log vẫn phải đi qua native redaction lần nữa trước IPC/export, kể cả file đã được redact khi ghi. Không được coi on-disk log hiện tại là trusted-safe input.

Redactor tối thiểu xử lý:

- password, passwd, pwd assignments;
- token, secret, API key style assignments;
- Authorization/Bearer/Basic headers;
- Cookie và Set-Cookie;
- URL/DSN có embedded username/password;
- WordPress auth/session cookie values;
- known CoffeePOS machine token;
- known WordPress/runtime database protected password;
- known admin protected password/pending password khi secret tồn tại;
- protected pending credential còn sót từ onboarding/repair transaction;
- giá trị secret được load trong native chỉ để exact-match redact.

Known protected secret được load vào memory cục bộ cho redaction, không serialize vào bundle manifest, error message hoặc IPC. Nếu protected-secret file **không tồn tại hợp lệ** theo state hiện tại, native tiếp tục với pattern redaction. Nếu protected-secret file tồn tại nhưng không đọc/giải mã được, cả viewer lẫn export phải **fail closed** cho source có thể chứa raw process output: read_log_page trả structured read error và không trả log text; export abort + cleanup temp. Không được “bỏ qua cho xong” vì raw/legacy log có thể chứa secret value không khớp generic keyword pattern.

Redactor thay cả dòng nhạy cảm hoặc value tương ứng bằng marker ổn định:

~~~text
[redacted]
~~~

Không dùng hash của secret trong bundle vì hash vẫn tạo identifier lâu dài không cần thiết cho support.

## Privacy ngoài credential

Support bundle phục vụ lỗi runtime/provisioning, không phải snapshot cửa hàng. Vì vậy:

- không export database dump;
- không export site/, wp-config.php, .env, PHP session files;
- không export uploads;
- không export browser cookies/local storage;
- không export backup;
- không export DPAPI/protected secret files;
- không export raw config JSON chỉ vì file đó “có vẻ không có password”;
- không query customer/order/payment data;
- không include WordPress salts/keys;
- không include full environment variables/process command lines.

Absolute user profile/data-root path phải được normalize trong exported text/metadata khi xuất hiện, ví dụ:

~~~text
C:\\Users\\Alice\\AppData\\Local\\CoffeePOS
→ %COFFEEPOS_DATA%
~~~

Loopback port, component version, phase/stage và error code là diagnostic data hợp lệ. Store name/admin email/username không cần cho Phase 6.4 nên không đưa vào support manifest.

## Support bundle schema

Bundle là ZIP schema-versioned, được tạo trong managed temp location rồi atomic finalize/copy sang destination khi tất cả bước đã pass redaction.

Conceptual layout:

~~~text
CoffeePOS-support-20260919-124500.zip
├── manifest.json
├── diagnostics/
│   ├── runtime.json
│   ├── health.json
│   ├── provisioning.json
│   └── repair.json
└── logs/
    ├── application.log
    ├── runtime.log
    ├── web-server.log
    ├── php.log
    ├── database.log
    ├── cron.log
    ├── provisioning.log
    ├── wordpress.log
    ├── woocommerce.log
    └── coffeepos.log
~~~

Chỉ include file tồn tại.

### manifest.json

Chỉ chứa metadata hỗ trợ:

~~~json
{
  "schema_version": 1,
  "created_at": "2026-09-19T12:45:00+07:00",
  "desktop_version": "0.1.0",
  "target": "x86_64-pc-windows-msvc",
  "runtime": {
    "php": "8.4.25",
    "mariadb": "11.4.13",
    "caddy": "2.11.4"
  },
  "installation_state": "ready",
  "runtime_state": "running",
  "redaction": {
    "schema_version": 1,
    "files_processed": 8,
    "replacements": 3
  }
}
~~~

Version trong manifest phải lấy từ native/pinned metadata hiện có, không parse bằng filename nếu đã có source of truth tốt hơn.

### diagnostics/runtime.json

Safe summary từ get_runtime_info/state hiện có:

- running/stopped/starting/stopping/error;
- component process presence;
- loopback HTTP/FastCGI/database port nếu đang có;
- cached health state/timestamp;
- last structured runtime error code/component/action/message đã redaction;
- không include process environment hoặc command line.

### diagnostics/health.json

Snapshot an toàn của component status hiện có cho Database/PHP/WordPress/WooCommerce/CoffeePOS. Export không tự start runtime để lấy health mới.

Nếu cached/safe snapshot không tồn tại:

~~~json
{
  "available": false,
  "reason": "no_cached_snapshot"
}
~~~

Không biến export thành một maintenance probe có side effect.

### diagnostics/provisioning.json

Native tạo projection allowlist từ provisioning state/journal:

- stage;
- ready/can_retry/needs_repair;
- recovery blocker code;
- expected component versions;
- timestamps/checkpoint cần cho diagnosis.

Không copy nguyên config/provisioning.json.

### diagnostics/repair.json

Nếu có repair state, chỉ include projection:

- transaction stage;
- target/action ids;
- committed/failed/blocked status;
- safe structured error;
- runtime-was-running flag;
- verifier result summary.

Không include pending credential, protected hash snapshot hoặc raw repair backup path.

## Size bounds

Export phải có giới hạn độc lập với log writer hiện tại vì một số provisioning/plugin log cũ có thể lớn hoặc được tạo trước khi bound tồn tại.

Target ban đầu:

- tối đa 2 MiB đọc từ mỗi log source cho bundle, ưu tiên tail mới nhất;
- tối đa 20 MiB uncompressed text/JSON tổng cộng;
- nếu source vượt limit, include tail + marker:

~~~text
[older log content omitted by support bundle size limit]
~~~

- manifest ghi source nào bị truncate;
- bundle ZIP cuối cùng không được chứa file tạm/raw source copy.

Viewer cũng dùng paging bounded nên không bị ảnh hưởng bởi source lớn.

## Lifecycle và concurrency

get_log_catalog và read_log_page là read-only filesystem operations, không cần serialize sau lifecycle lock nếu implementation không chạm mutable runtime state.

Export:

1. capture safe metadata snapshot ngắn;
2. đọc từng allowlisted log;
3. redact + normalize path trong memory/stream;
4. ghi vào temp bundle dưới managed data/temp directory;
5. validate archive entries + redaction result;
6. finalize ra destination đã chọn;
7. cleanup temp dù success/cancel/failure.

Không giữ lifecycle mutex suốt quá trình ZIP nhiều file. Runtime được phép tiếp tục ghi log trong lúc export; bundle là best-effort point-in-time snapshot. Mỗi source cần đọc an toàn đến EOF/size boundary tại thời điểm mở; log append sau đó có thể thuộc bundle kế tiếp.

Phase 6.4 không pause POS request, cron hoặc database chỉ để có log snapshot nhất quán.

## UI trong Hệ thống

**Nhật ký** là chức năng cấp hai trong **Hệ thống**, cùng cấp với **Chẩn đoán** và **Sửa chữa**.

Conceptual navigation:

~~~text
Hệ thống
  ├── Chẩn đoán
  ├── Sửa chữa
  └── Nhật ký
~~~

Không tạo top-level tab thứ tư.

### Landing

~~~text
Hệ thống > Nhật ký

Nhật ký hệ thống
Xem thông tin chẩn đoán gần đây hoặc xuất gói hỗ trợ khi cần xử lý sự cố.

[ Ứng dụng ] [ Runtime ] [ PHP ] [ Database ] [ Thiết lập ] ...

Runtime
Cập nhật gần nhất: 12:41

1726720000 [event] runtime start requested
1726720001 [event] database ready
1726720002 [event] web server ready

[ Tải dòng cũ hơn ]

[ Làm mới ]              [ Xuất gói hỗ trợ ]
~~~

Tab/chip nguồn log ở đây là navigation cấp ba trong trang Nhật ký, không phải global shell navigation. Với viewport hẹp có thể dùng select/list thay vì ép hàng chip tràn ngang.

### Không có dữ liệu

~~~text
Runtime

Chưa có dữ liệu nhật ký cho thành phần này.
Nhật ký sẽ xuất hiện khi thành phần được chạy hoặc kiểm tra.

[ Làm mới ]
~~~

### Đang đọc

Giữ log cũ đang hiển thị nếu có và báo trạng thái nhỏ **Đang làm mới…**; không blank toàn trang trong mỗi refresh.

### Lỗi đọc

~~~text
Không thể đọc nhật ký Runtime
CoffeePOS không thể đọc file nhật ký hiện tại.

[ Thử lại ]
~~~

Structured error có thể mở dưới **Chi tiết kỹ thuật**. Không hiện absolute path mặc định.

### Export

Khi người dùng bấm **Xuất gói hỗ trợ**:

~~~text
Đang tạo gói hỗ trợ…
CoffeePOS đang thu thập nhật ký và loại bỏ thông tin nhạy cảm.

[ action disabled ]
~~~

Success:

~~~text
Đã xuất gói hỗ trợ
File đã được lưu tại vị trí bạn chọn.

[ Mở thư mục chứa file ]
~~~

**Mở thư mục chứa file** chỉ dùng path do native export result trả về từ destination người dùng đã chọn. Không nhận path nhập tay từ text field.

Failure:

~~~text
Không thể xuất gói hỗ trợ
<lý do ngắn>

[ Thử lại ]
~~~

Cancel Save As quay về viewer yên lặng; không tạo banner lỗi.

## Refresh/polling

Phase 6.4 không cần poll log liên tục.

- mở Nhật ký: fetch catalog + tail log đang chọn;
- **Làm mới**: fetch tail mới;
- đổi source: fetch source đó;
- sau export: không tự refresh mọi source;
- không chạy 1-second polling tạo disk I/O liên tục.

Nếu sau này thêm live tail, đó là extension riêng và phải có backpressure/bounds.

## Error model

Native trả structured error tối thiểu theo:

~~~text
component = "logs"
action = "catalog" | "read" | "export" | "finalize"
message = safe user-facing diagnostic
recovery = safe next action
~~~

Các lỗi phổ biến:

- log directory không đọc được;
- source biến mất giữa catalog và read;
- invalid/stale cursor;
- disk full khi tạo temp/export;
- destination không ghi được;
- ZIP finalize fail;
- redaction safety check fail.

Redaction safety check fail phải abort export và cleanup temp. Không được “xuất phần còn lại” nếu contract không còn được bảo đảm.

## Security acceptance

1. Frontend không thể yêu cầu native đọc wp-config.php, arbitrary path hoặc file ngoài log allowlist qua sửa log_id.
2. Viewer IPC chỉ chứa redacted text; injected fixture có admin password, DB password, machine token, Authorization/Cookie đều không xuất hiện.
3. Export fixture chứa các secret trên không để plaintext xuất hiện trong bất kỳ ZIP entry nào, manifest hoặc filename.
4. Active + pending onboarding/repair credentials đều được exact-match redact khi tồn tại.
5. Protected-secret file tồn tại nhưng unreadable/corrupt làm viewer/export fail closed trước khi trả log text; secret file hợp lệ nhưng absent theo state được phân biệt và không tự coi là corruption.
6. Support bundle không chứa protected secret files, raw config, database, uploads, site files, browser state hoặc backup.
7. Absolute data-root/user-profile paths được normalize trong exported text/metadata.
8. Arbitrary file trong logs/ không thuộc allowlist không được tự động đóng gói.
9. Symlink/reparse-point source nếu xuất hiện phải bị từ chối khi target thoát khỏi managed logs directory; native không follow path escape.

## Functional acceptance

1. Healthy installed store mở **Hệ thống → Nhật ký**, xem được tail runtime.log và đổi qua PHP/Database mà không start/restart runtime.
2. Missing optional log hiển thị **Chưa có dữ liệu**, không tạo file rỗng.
3. File lớn đọc theo page bounded; **Tải dòng cũ hơn** không duplicate/skip page trong fixture ổn định.
4. Log đang được append trong lúc viewer mở vẫn refresh được; lỗi file rotation/truncate trả state có thể retry.
5. Export khi runtime running không dừng POS, Caddy, PHP, cron hoặc MariaDB.
6. Export khi runtime stopped vẫn tạo bundle từ evidence hiện có và không auto-start.
7. Bundle có manifest + safe diagnostics + các allowlisted log tồn tại; source vượt size bound có marker/truncation metadata.
8. Cancel Save As không để temp archive; disk/write/finalize failure cleanup temp và UI có action retry.
9. Double click export không tạo duplicate operation.
10. Reload/navigation trong lúc export không làm frontend tạo operation thứ hai; native result/failure vẫn deterministic.
11. Keyboard/focus/viewport hẹp/DPI giữ source selector, log viewport và export action thao tác được.

## Validation dự kiến

Theo preference hiện tại của dự án, validation tự động giữ focused:

~~~powershell
. .\scripts\use-local-rust.ps1
cargo fmt --manifest-path .\src-tauri\Cargo.toml --check
cargo test --manifest-path .\src-tauri\Cargo.toml --locked log_redaction
cargo test --manifest-path .\src-tauri\Cargo.toml --locked support_bundle
npm run lint:ui
npm run build:ui
git diff --check
~~~

Focused Rust tests nên có fixture secret canary duy nhất được inject vào tất cả pattern quan trọng rồi scan toàn bộ viewer payload + extracted ZIP text để chứng minh canary không tồn tại.

Manual Windows smoke tối thiểu:

1. mở store healthy → Hệ thống → Nhật ký → đổi 4–5 source;
2. start/stop runtime để tạo log mới rồi **Làm mới**;
3. export bundle khi running;
4. export bundle khi stopped;
5. mở ZIP và kiểm manifest/log;
6. thử cancel Save As;
7. thử destination read-only/disk-write failure có kiểm soát;
8. resize/keyboard/150% DPI hoặc WebView scale tương đương.

Không cần chạy lại full provisioning matrix cho mọi chỉnh sửa UI nhỏ.

### Bằng chứng implementation hiện tại — 2026-09-19

- get_log_catalog chỉ expose 10 source cố định; file ngoài allowlist không được scan/đóng gói. Source path được canonicalize và phải nằm trong managed logs/.
- read_log_page trả tối đa 200 dòng/page với 64 KiB post-redaction cap và bounded reverse scan; cursor gắn file size/mtime, bị từ chối khi stale hoặc trỏ giữa dòng. Tail đang ghi dở bị bỏ khỏi snapshot để prefix của exact secret không thể lọt qua IPC.
- Viewer và export load active/pending protected DB/admin/machine secrets chỉ để exact-match redact. Secret file absent hợp lệ được bỏ qua; file tồn tại nhưng unreadable/corrupt/symlink/decrypt-fail làm operation fail closed trước khi trả log text.
- Support bundle schema 1 gồm safe manifest, runtime/health/provisioning/repair projections và tail của allowlisted logs; không copy raw config/database/uploads/site/backups/protected secrets. Bound là 2 MiB/source, 20 MiB uncompressed tổng; archive path được validate, temp file cleanup theo RAII và finalize qua destination temp.
- Export dùng Windows native Save As, native AtomicBool chống duplicate export qua reload/double-click, chỉ đọc cached RuntimeManager info nếu lock sẵn có và không start/stop/probe runtime.
- Export normalize COFFEEPOS data root + USERPROFILE cho cả slash/backslash và match path case-insensitive trên Windows; regression giữ Unicode text nguyên vẹn.
- UI **Hệ thống → Nhật ký** có source selector, empty/loading/error states, **Tải dòng cũ hơn**, **Làm mới**, export status/cancel và frontend in-flight gating; viewer giữ tối đa 1.000 dòng để tránh tăng DOM/memory vô hạn.

Validation đã chạy:

~~~powershell
. .\scripts\use-local-rust.ps1
cargo fmt --manifest-path .\src-tauri\Cargo.toml -- --check
cargo test --manifest-path .\src-tauri\Cargo.toml --locked logs::tests
cargo check --manifest-path .\src-tauri\Cargo.toml --locked
npm run lint:ui
npm run build:ui
git diff --check
~~~

Kết quả: Rust logs tests **11/11 PASS**, cargo fmt/check PASS, UI typecheck/build PASS. Manual Windows Save As/export trên app thật, cancel/error destination, keyboard/resize/DPI và mở ZIP bằng flow người dùng vẫn còn chờ acceptance; automated checks không thay thế bước này.

## Thứ tự triển khai đề xuất

1. **6.4A — Safe log catalog/read:** allowlist, bounded tail/paging, read-time redaction, path containment.
2. **6.4B — Redaction hardening:** gom runtime/provisioning writer policy, exact protected-secret redaction, fixture/canary tests.
3. **6.4C — Support bundle:** safe diagnostic projections, ZIP schema 1, size bounds, temp/finalize/cleanup.
4. **6.4D — Hệ thống → Nhật ký:** source selector, viewer states, refresh, native Save As/export UX.
5. **6.4E — Windows acceptance:** running/stopped export, cancel/failure cleanup, keyboard/resize/DPI, manual ZIP inspection.

Các nhãn A–E là implementation slices bên trong Phase 6.4, không thay numbering chính trong ROADMAP.md.

## Ngoài phạm vi

- Gửi/upload bundle tự động tới server/support service.
- Cloud telemetry/crash-report upload.
- Full-text search trên toàn bộ lịch sử log.
- Live streaming log theo từng dòng/WebSocket.
- Database dump hoặc business-data export.
- Backup/restore store: Phase 7.
- LAN/network packet diagnostics: Phase 8.x.
- Installer/update logs từ production installer/update service: Phase 9.x khi các component đó tồn tại.

## Definition of Done

Phase 6.4 chỉ được đánh dấu hoàn thành khi **Hệ thống → Nhật ký** dùng native allowlist để xem log thật theo page/tail bounded; mọi text qua IPC đã được redaction; export tạo support bundle schema-versioned từ safe diagnostics + allowlisted logs, có export-time redaction bắt buộc, size bounds, temp cleanup và native Save As; fixture secret canary chứng minh admin/DB/machine token/auth cookie không xuất hiện trong viewer/bundle; bundle không chứa database/uploads/site/config/protected secrets; viewer/export không mutate lifecycle/store; và manual Windows smoke xác nhận success/empty/error/cancel/running/stopped + keyboard/resize/DPI.
