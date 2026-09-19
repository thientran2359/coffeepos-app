# Phase 7.2 — Database backup

Phase 7.2 tạo primitive logical backup cho MariaDB của managed CoffeePOS store. Database phải được dump bằng tool đã pin từ runtime artifact, trong một maintenance window không còn POS request/cron writer, và dump phải có thể import vào database test để chứng minh không chỉ tạo ra một file SQL “có vẻ hợp lệ”.

> **Trạng thái:** đặc tả đã chốt cho implementation Phase 7.2. Phase này tạo database artifact + snapshot lifecycle primitive; complete encrypted backup thuộc Phase 7.3.

## Mục tiêu

1. Dùng mariadb-dump từ exact managed runtime, không dùng PATH/global MariaDB.
2. Chặn application writers trong toàn bộ database snapshot.
3. Không copy live database datadir làm portable backup.
4. Không lộ database credential qua command line, log hoặc frontend.
5. Dump có schema + data cần để tái tạo CoffeePOS/WooCommerce/WordPress database.
6. Dump được verify bằng import vào disposable database/profile trước khi Phase 7.2 được coi là hoàn thành.
7. Primitive lifecycle đủ để Phase 7.3 giữ store quiesced tiếp tục copy uploads/config cùng một snapshot.

## Runtime artifact

Current development MariaDB artifact đã có:

~~~text
runtime/development/x86_64-pc-windows-msvc/mariadb/bin/mariadb-dump.exe
runtime/development/x86_64-pc-windows-msvc/mariadb/bin/mariadb.exe
~~~

Phase 7.2 phải đưa path các tool backup/import vào runtime manifest source of truth thay vì dò filename ad-hoc:

~~~json
{
  "mariadb": {
    "server": "mariadb/bin/mariadbd.exe",
    "client": "mariadb/bin/mariadb.exe",
    "dump": "mariadb/bin/mariadb-dump.exe",
    "import": "mariadb/bin/mariadb.exe"
  }
}
~~~

ResolvedRuntime validate file tồn tại, nằm trong managed runtime root và thuộc exact staged artifact. Phase 9.1 chịu trách nhiệm bundle production cùng tool đã nghiệm thu; Phase 7 không lấy tool từ máy người dùng.

## Backup maintenance guard

Database dump không chạy song song với provisioning/repair/restore/runtime lifecycle mutation.

Native cần một operation guard dùng cùng authority với lifecycle hiện có:

~~~text
enter_backup_maintenance()
  -> BackupMaintenanceLease
~~~

Lease giữ tối thiểu:

- operation id;
- runtime_was_running;
- previous runtime state;
- current stage;
- cancellation flag hợp lệ;
- cleanup/resume responsibility.

Khi lease active:

- Start/Restart/Repair/Provision/Restore khác bị chặn ở native, không chỉ disable UI;
- daily auto-start không được spawn runtime;
- Close/Alt+F4 phải đi qua cleanup có giới hạn hoặc báo rõ operation chưa thể commit;
- reload/navigation không tạo backup thứ hai.

## Snapshot sequence

Initial Phase 7 ưu tiên consistency hơn zero-downtime.

Nếu runtime đang chạy:

~~~text
Acquire backup maintenance guard
        ↓
Stop accepting new managed operations
        ↓
Drain Caddy/PHP request theo Phase 5.6/6.2
        ↓
Stop cron
        ↓
Stop Caddy + PHP
        ↓
Graceful stop MariaDB
        ↓
Start MariaDB-only maintenance instance
        ↓
Wait authenticated database readiness
        ↓
Run logical dump
~~~

Nếu runtime đang stopped, không start Caddy/PHP/cron; chỉ start MariaDB-only maintenance instance cho dump.

Web/PHP/cron phải giữ stopped từ thời điểm snapshot bắt đầu cho tới khi Phase 7.3 copy xong uploads/config. Phase 7.2 API vì vậy không được tự resume runtime ngay sau dump khi caller đang giữ maintenance lease.

Standalone Phase 7.2 smoke test được phép gọi cleanup lease sau khi dump để stop DB-only instance và restore runtime_was_running.

## MariaDB-only maintenance

Không dùng production runtime start() bình thường vì start() sẽ đưa Caddy/PHP/cron trở lại.

Primitive database-only phải:

- dùng cùng datadir managed hiện tại;
- bind database loopback-only;
- dùng dynamic available port;
- track Child/Job containment như lifecycle hiện có;
- wait readiness bằng authenticated query;
- stop graceful qua runtime DB credential;
- kill + bounded wait khi graceful stop fail;
- không để orphan process sau success/failure/cancel.

Database-only maintenance không chạy migration hoặc provisioning.

## Credential handling

Dump dùng WordPress DB account hiện tại vì account này có authority trên application database. Password lấy từ protected config/database-wordpress.secret.

Không truyền password trực tiếp trong process argument.

Preferred execution:

1. tạo temporary MariaDB client option file trong managed private temp;
2. ACL chỉ current Windows user;
3. nội dung gồm loopback host/port/user/password;
4. gọi mariadb-dump trực tiếp với --defaults-extra-file=<temp>;
5. không echo command line hoặc option-file content vào log;
6. cleanup option file ở success/failure/cancel;
7. nếu protected credential unreadable hoặc authenticated query fail, backup blocked trước khi tạo dump.

Environment variable chứa plaintext credential không phải contract mặc định.

## Dump contract

Phase 7 backup application database:

~~~text
coffeepos
~~~

Không dump system databases mysql, performance_schema, information_schema hoặc sys.

Dump phải bao gồm:

- table definitions;
- table data;
- indexes/constraints;
- triggers thuộc application database;
- charset/collation information cần import đúng.

Dump không được chứa:

- CREATE USER/GRANT từ system database;
- source machine filesystem path;
- runtime credential ngoài data đã tồn tại hợp lệ trong WordPress application tables.

Execution options phải được pin trong implementation/test và ưu tiên deterministic logical output:

- single transaction/quick streaming khi engine hỗ trợ;
- skip table locks vì application writers đã bị chặn;
- utf8mb4;
- binary/blob safe representation;
- explicit database selection;
- no shell interpolation.

Nếu phát hiện object/engine mà option set hiện tại không thể snapshot đầy đủ, operation phải blocked với reason rõ thay vì silently bỏ object.

## Plaintext SQL staging

Logical dump chứa toàn bộ business/customer/order data nên là sensitive.

Trong Phase 7.2 có thể cần temporary plaintext SQL để test primitive. Quy tắc:

- chỉ trong managed temp dưới data root/backups staging hoặc OS temp đã kiểm soát;
- random operation directory;
- restrictive ACL cho current user;
- filename không chứa store/customer data;
- không trả path cho frontend trừ safe operation metadata;
- cleanup bằng RAII/transaction cleanup ở success/failure/cancel;
- crash recovery phát hiện stale owned temp theo operation marker và cleanup trước backup tiếp theo;
- logs chỉ ghi byte count/hash/status.

Phase 7.3 phải consume dump vào encrypted container và xóa plaintext staging trước khi resume runtime.

## Dump verification

File tồn tại + non-zero chưa đủ.

Native verification theo hai cấp.

### Structural verification

- mariadb-dump exit code 0;
- stdout/stderr bounded + redacted;
- SQL file non-empty;
- SHA256 + byte size được tính;
- expected WordPress core table baseline tồn tại trong dump representation hoặc được xác minh sau import.

### Restore-to-disposable verification

Focused acceptance tạo disposable MariaDB datadir/profile:

1. initialize clean MariaDB;
2. tạo clean target database + target test DB user;
3. import dump bằng pinned mariadb client;
4. query invariant set.

Invariant tối thiểu:

- WordPress options table tồn tại;
- users/usermeta tồn tại;
- WooCommerce required tables/baseline tồn tại;
- CoffeePOS tables/options cần cho current plugin baseline tồn tại;
- selected sentinel fixture row/order option giữ đúng;
- table count và selected row/hash values khớp source fixture.

Không log row chứa customer/admin credential.

## Error model

~~~text
component = "backup_database"
action =
  "quiesce"
  | "start_database"
  | "dump"
  | "verify"
  | "cleanup"
  | "resume"
code = stable machine-readable code
message = safe explanation
recovery = safe next action
~~~

Các lỗi chính:

- runtime cannot drain/stop;
- DB credential missing/unreadable/mismatch;
- dump tool missing/hash/runtime manifest mismatch;
- database-only startup/readiness fail;
- dump child non-zero/timeout;
- disk full/write failure;
- unsupported database object/engine;
- cleanup cannot stop child;
- previous runtime cannot resume.

Nếu dump fail, active database/datadir phải được preserve nguyên trạng.

## Cancellation

User cancel trước quiesce: kết thúc yên lặng.

Sau khi maintenance đã bắt đầu:

- cancellation trở thành cleanup request;
- terminate dump child bounded nếu đang chạy;
- delete temp option/dump;
- stop database-only child;
- nếu runtime_was_running thì start lại runtime + normal health scheduling;
- UI chỉ báo cancelled sau cleanup đã kết thúc hoặc báo cleanup error rõ ràng.

Không bỏ process đang chạy chỉ vì frontend đóng modal.

## Functional acceptance

1. Healthy running store → quiesce → logical dump pass → cleanup → runtime trở lại running/healthy.
2. Healthy stopped store → chỉ MariaDB maintenance start → dump → DB stop; Caddy/PHP/cron không spawn.
3. Dump không dùng PATH; xóa/rename managed dump tool fixture làm operation fail rõ.
4. DB password không xuất hiện trong process args captured test/log.
5. Wrong protected DB credential block trước dump.
6. Dump import vào disposable DB pass invariant set.
7. Fixture sentinel WooCommerce/CoffeePOS data giữ đúng sau import.
8. Disk-write failure cleanup plaintext temp + database-only process.
9. Cancel giữa dump cleanup child/temp và restore previous runtime state.
10. Forced dump child failure không mutate live datadir.
11. Repeated backup không để stale DB-only process/port/option file.

## Validation dự kiến

~~~powershell
. .\scripts\use-local-rust.ps1
cargo fmt --manifest-path .\src-tauri\Cargo.toml -- --check
cargo test --manifest-path .\src-tauri\Cargo.toml --locked backup_database
cargo check --manifest-path .\src-tauri\Cargo.toml --locked
git diff --check
~~~

Manual Windows smoke dùng **disposable store/profile**:

1. tạo sentinel product/order/config;
2. backup database khi runtime running;
3. import dump vào disposable target;
4. query sentinel;
5. lặp khi runtime stopped;
6. inject dump failure/cancel;
7. Task Manager/port check không còn orphan.

Không chạy failure test trên store vận hành thật.

## Ngoài phạm vi

- Copy uploads/config: Phase 7.3.
- Final encrypted .coffeepos-backup writer: Phase 7.3.
- User restore/cutover: Phase 7.4.
- Physical mariabackup/datadir snapshot.
- Point-in-time/binlog restore.
- Online zero-downtime backup.
- Scheduled backup.
- Production runtime bundling: Phase 9.1.

## Definition of Done

Phase 7.2 chỉ hoàn thành khi native dùng exact pinned mariadb-dump từ managed runtime; backup maintenance guard ngăn application writers/lifecycle conflict; database-only process có readiness/cleanup/orphan guarantees; DB credential không lộ qua args/log/IPC; logical dump chứa application schema/data đầy đủ theo contract; dump import vào disposable MariaDB tái tạo sentinel data; failure/cancel giữ live datadir nguyên vẹn và restore runtime_was_running; focused automated checks + Windows disposable-store smoke pass.
