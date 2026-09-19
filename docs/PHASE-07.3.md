# Phase 7.3 — Uploads/config backup

Phase 7.3 biến contract 7.1 + database primitive 7.2 thành complete portable backup mà người dùng có thể tạo từ **Hệ thống → Sao lưu và khôi phục**. Operation phải giữ database, uploads và store config trong cùng một maintenance snapshot, mã hóa toàn bộ payload và chỉ báo thành công sau khi archive cuối đã validate.

> **Trạng thái:** đặc tả đã chốt cho implementation Phase 7.3. Restore/cutover thuộc Phase 7.4.

## Mục tiêu

1. Tạo complete encrypted .coffeepos-backup từ store đã quản lý.
2. Database dump, uploads, store identity và admin secret thuộc cùng một snapshot không có application writer.
3. Không bundle runtime/core/plugin code hoặc raw local config.
4. Backup chạy qua native Save As, có progress/cancel/failure cleanup rõ ràng.
5. Archive chỉ final thành công sau khi validator Phase 7.1 mở lại và validate pass.
6. Runtime trở lại đúng trạng thái trước operation.

## Entry gate

Portable user backup được phép khi:

- provisioning state là ready;
- không có provisioning/repair/restore transaction active hoặc interrupted chưa reconcile;
- không có pending admin credential transaction;
- không có pending machine-token transaction;
- app config/store identity đọc được;
- active WordPress admin secret đọc được;
- database runtime + WordPress credential đọc được và preflight pass;
- runtime artifact chứa exact dump/import tools cần thiết;
- uploads path + config paths nằm trong managed data root và không có unsafe reparse escape.

Application health không bắt buộc phải “all green” nếu lỗi chỉ thuộc service startup, nhưng database preflight + managed ownership phải đủ để tạo consistent backup. Nếu provisioning needs_repair/corrupt ownership, user backup bị blocked để tránh đóng gói trạng thái không xác định như một bản portable hợp lệ.

## Dữ liệu được đưa vào backup

### 1. Database

database/store.sql từ Phase 7.2.

Database dump chứa WordPress/WooCommerce/CoffeePOS business data. Không dump MariaDB system database hoặc live datadir.

### 2. Uploads

Managed root:

~~~text
<data_root>/uploads/
~~~

Giữ relative path + exact file bytes.

Không preserve:

- NTFS ACL;
- alternate data streams;
- symlink/junction/reparse point;
- absolute source path.

Archive metadata có thể giữ safe mtime nếu implementation cần, nhưng restore correctness không phụ thuộc mtime.

### 3. Store config projection

config/store.json theo schema 7.1, được tạo từ validated native config/live store identity.

Không copy raw:

- config/app.json;
- config/provisioning.json;
- config/repair.json;
- config/restore.json;
- generated runtime config;
- logs.

### 4. Administrator secret

secrets/administrator.json theo 7.1, chỉ nằm bên trong encrypted payload.

Không copy các DPAPI file khác.

## Unmanaged site content

Phase 7 portable backup là **store-data backup**, không phải executable-site image.

Native inspect site/wp-content:

- managed WooCommerce/CoffeePOS files: excluded, target restore lấy từ pinned artifacts;
- managed mu-plugin/runtime bridge: excluded, target regenerate;
- unmanaged plugin/theme executable code: excluded;
- uploads phải nằm ở managed uploads root và được include riêng.

Nếu phát hiện unmanaged plugin/theme directory, manifest thêm warning metadata:

~~~json
{
  "warnings": [
    {
      "code": "unmanaged_site_code_not_included",
      "items": ["plugin-slug"]
    }
  ]
}
~~~

Không đưa full path vào warning.

UI phải nói rõ backup không mang theo custom plugin/theme code. Initial Phase 7 không tự copy PHP executable content vào portable archive.

## Consistent snapshot

Phase 7.3 giữ BackupMaintenanceLease của 7.2 xuyên suốt complete operation.

Sequence:

~~~text
User confirms backup password
        ↓
Native Save As
        ↓
Preflight entry gate + destination
        ↓
Acquire backup maintenance lease
        ↓
Drain/stop Caddy + PHP + cron + MariaDB
        ↓
Start MariaDB-only maintenance
        ↓
Logical database dump
        ↓
Stop MariaDB-only maintenance
        ↓
Copy/read uploads while all app writers remain stopped
        ↓
Capture store config + administrator secret
        ↓
Build encrypted container
        ↓
Validate temporary final archive
        ↓
Atomic finalize destination
        ↓
Cleanup plaintext/temp state
        ↓
Restore runtime_was_running
~~~

Điểm quan trọng: runtime/web/cron không được restart giữa database dump và uploads/config capture.

Vì uploads có thể lớn, initial Phase 7 chấp nhận maintenance downtime để ưu tiên consistency. Online copy/chunk snapshot là future optimization.

## Destination rules

Save As dùng native dialog. Frontend không gửi arbitrary source path/file list.

Destination:

- có extension .coffeepos-backup;
- được canonicalize ở native;
- không được nằm dưới uploads/, site/, database/, config/ hoặc logs/ vì operation đang snapshot các cây này;
- có thể nằm ngoài data root hoặc trong managed backups/;
- temporary output được tạo cùng filesystem/directory đủ gần destination để final rename/replace có semantics xác định;
- existing destination chỉ được thay theo native Save As overwrite confirmation.

Nếu destination là removable/network filesystem, implementation phải coi rename durability có thể khác local NTFS và chỉ báo success sau close/flush + final validation theo khả năng platform.

## Archive build

Không tạo unencrypted ZIP hoàn chỉnh trên disk.

Writer pipeline:

~~~text
payload entries
  → ZIP stream
  → age passphrase encryption
  → destination temporary file
~~~

Database temporary SQL là plaintext artifact duy nhất được phép trong operation theo contract 7.2; nó phải có private ACL và bị xóa trước runtime resume.

Uploads được stream trực tiếp vào encrypted payload; không duplicate toàn bộ uploads sang plaintext staging.

Writer/reader phải hỗ trợ ZIP64 + streaming. Không dùng lại support-bundle writer Phase 6.4 nếu path đó build ZIP trong memory hoặc dùng giới hạn 20 MiB dành cho diagnostic logs.

Trong lúc write:

- tính SHA256 + size mỗi payload entry;
- build inventory;
- track file count/bytes;
- reject source thay path type sang reparse point;
- bounded log/progress payload;
- không log archive password, admin password hoặc content.

## Change detection

Vì app writers đã stopped, uploads không nên đổi trong snapshot. Tuy vậy external process có thể sửa filesystem.

Cho mỗi upload file:

1. lstat path và reject reparse;
2. capture size + stable identity/mtime nếu platform có;
3. stream bytes;
4. stat lại;
5. nếu identity/size/mtime cho thấy file đổi trong khi đọc → backup fail với source_changed.

Không silently accept mixed version của cùng file.

Directory enumeration cũng phải reject entry xuất hiện ngoài managed root qua reparse.

## Preflight disk space

Trước quiesce, native estimate:

- current database size heuristic;
- uploads total bytes;
- free space destination;
- free space local temp cho database SQL.

Không cần dự đoán exact encrypted/compressed output, nhưng phải phát hiện trường hợp rõ ràng không đủ temp/destination space trước khi gây downtime.

Nếu free-space query không khả dụng, operation có thể tiếp tục với warning nếu filesystem writable; write failure vẫn phải cleanup/resume.

## Backup UI

Vị trí:

~~~text
Hệ thống
  └── Sao lưu và khôi phục
      ├── Sao lưu
      └── Khôi phục   # action thật bổ sung ở 7.4
~~~

Phase 7.3 chỉ enable **Sao lưu**. Không tạo restore button giả trước 7.4.

### Landing

~~~text
Hệ thống > Sao lưu và khôi phục

Sao lưu cửa hàng
Tạo một bản sao lưu mã hóa gồm dữ liệu cửa hàng, hình ảnh tải lên
và cấu hình cần thiết để khôi phục trên CoffeePOS Desktop tương thích.

[ Tạo bản sao lưu ]
~~~

Nếu có last-success metadata local:

~~~text
Lần sao lưu gần nhất
19/09/2026 13:20
Đã hoàn thành
~~~

Không cần scan arbitrary backup directory để dựng history. Local history chỉ là safe operation metadata.

### Password step

~~~text
Mật khẩu bản sao lưu
[ •••••••••••• ]

Nhập lại mật khẩu
[ •••••••••••• ]

Mật khẩu này cần khi khôi phục trên máy/profile khác.

[ Hủy ] [ Chọn nơi lưu ]
~~~

Password không persist sau operation.

### Progress

~~~text
Đang sao lưu…

Đang chuẩn bị dữ liệu cửa hàng
Đã xử lý 420 / 1.250 tệp

POS tạm dừng trong khi CoffeePOS tạo snapshot nhất quán.

[ Hủy ]
~~~

Progress không hiện filename có customer data nếu không cần.

### Success

~~~text
Đã sao lưu cửa hàng
File backup đã được kiểm tra và lưu tại vị trí bạn chọn.

[ Mở thư mục chứa file ]
~~~

Open folder chỉ dùng destination native result, không nhận path text từ frontend.

### Failure

~~~text
Không thể tạo bản sao lưu
<lý do ngắn>

Hệ thống đã khôi phục trạng thái chạy trước khi sao lưu.

[ Thử lại ]
~~~

Nếu runtime resume fail, UI phải ưu tiên báo runtime recovery action thay vì tuyên bố hệ thống đã trở lại bình thường.

## Native command contract

Conceptual:

~~~text
get_backup_status() -> BackupStatus
create_backup(backup_password) -> BackupResult
cancel_backup(operation_id) -> CancelResult
~~~

create_backup tự mở native Save As hoặc dùng native-owned destination token; frontend không đưa arbitrary filesystem path.

Native giữ duplicate-operation guard. Reload/navigation chỉ reconnect/read status; không spawn operation mới.

Status payload chỉ chứa:

- operation id;
- stage;
- processed/estimated bytes/files;
- safe timestamps;
- cancelled/succeeded/failed;
- safe error.

Không chứa secret/source path.

## Crash/relaunch cleanup

Phase 7.3 cần lightweight backup transaction marker, ví dụ:

~~~text
config/backup.json
~~~

Marker không chứa backup password/admin password.

Stage đủ để relaunch biết:

- operation interrupted;
- plaintext DB temp có thể tồn tại ở owned path;
- destination temp có thể tồn tại;
- runtime trước operation có running hay không.

Relaunch cleanup:

1. không auto-start daily runtime trước khi cleanup backup transaction;
2. kill/wait owned DB-only/dump child nếu còn;
3. xóa owned plaintext temp + incomplete destination temp;
4. preserve finalized user file nếu marker chứng minh archive đã validate/finalize;
5. clear marker;
6. resume normal daily startup policy.

Không delete arbitrary file chỉ vì có extension .tmp; cleanup chỉ target owned path gắn operation id.

## Error model

~~~text
component = "backup"
action =
  "preflight"
  | "quiesce"
  | "database"
  | "uploads"
  | "archive"
  | "validate"
  | "finalize"
  | "cleanup"
  | "resume"
code = stable machine-readable code
message = safe explanation
recovery = safe next action
~~~

## Functional acceptance

1. Running healthy store → backup success → runtime trở lại running/healthy.
2. Stopped store → backup success → runtime vẫn stopped.
3. Backup chứa database/store.sql, uploads, config/store.json, secrets/administrator.json + valid manifest/inventory.
4. Random upload sentinel bytes restore/extract fixture khớp exact checksum.
5. Raw app/provisioning/repair config và DPAPI files không có trong payload.
6. Managed/unmanaged plugin PHP files không có trong payload.
7. Backup password/admin password không xuất hiện trong logs/IPC.
8. User backup file không thể inspect nội dung nếu thiếu/wrong password.
9. Destination trong uploads/site/database/config bị reject trước quiesce.
10. Source upload reparse/path escape bị reject.
11. File upload đổi giữa lúc đọc làm backup fail, không finalize partial archive.
12. Disk full/archive failure/cancel cleanup temp + plaintext SQL và resume runtime.
13. App crash/relaunch giữa operation cleanup đúng owned temp và không spawn duplicate.
14. Final file chỉ xuất hiện success sau Phase 7.1 validator pass.
15. Keyboard/focus/viewport hẹp/DPI thao tác được password, Save As, progress và result.

## Validation dự kiến

~~~powershell
. .\scripts\use-local-rust.ps1
cargo fmt --manifest-path .\src-tauri\Cargo.toml -- --check
cargo test --manifest-path .\src-tauri\Cargo.toml --locked backup
cargo check --manifest-path .\src-tauri\Cargo.toml --locked
npm run lint:ui
npm run build:ui
git diff --check
~~~

Manual Windows smoke trên disposable store:

1. tạo product/order + vài upload sentinel;
2. backup khi running;
3. mở file bằng validator với đúng/sai password;
4. backup khi stopped;
5. cancel Save As;
6. cancel giữa uploads;
7. inject destination write failure;
8. kill app ở transaction fixture/relaunch;
9. kiểm không orphan và runtime state được restore.

## Ngoài phạm vi

- Restore/cutover: Phase 7.4.
- Scheduled/automatic backup.
- Cloud sync/upload.
- Incremental backup.
- Backup custom plugin/theme executable code.
- Online zero-downtime snapshot.
- Production installer/bundle.
- macOS acceptance.

## Definition of Done

Phase 7.3 chỉ hoàn thành khi **Hệ thống → Sao lưu và khôi phục** tạo được một .coffeepos-backup mã hóa từ database + uploads + portable store config + admin secret trong cùng maintenance snapshot; archive writer không tạo plaintext ZIP đầy đủ; path/reparse/destination safety được enforce; temporary SQL/option files được cleanup; final archive được Phase 7.1 validator kiểm trước success; cancel/failure/crash không để orphan hoặc partial file được coi là backup; runtime quay lại đúng previous state; focused tests + Windows disposable-store smoke pass.
