# Phase 7.1 — Backup format

Phase 7.1 chốt định dạng backup portable cho CoffeePOS Desktop trước khi viết logic dump/copy dữ liệu. Mục tiêu của phase này là tạo một contract có thể validate độc lập, có version rõ ràng, không phụ thuộc đường dẫn máy nguồn và không bê nguyên secret Windows DPAPI sang profile/máy khác.

> **Trạng thái:** implementation Phase 7.1 đã có native encrypted backup-format writer + `inspect_backup` / `validate_backup`, strict schema/path/checksum/compatibility validation và focused acceptance tests. Database snapshot/copy data thật vẫn thuộc Phase 7.2/7.3.

## Mục tiêu

1. Định nghĩa một file backup duy nhất có schema/version rõ ràng.
2. Toàn bộ payload chứa store data phải được mã hóa trước khi rời máy.
3. Archive phải có manifest + checksum đủ để phát hiện file thiếu, file bị sửa hoặc archive không hoàn tất.
4. Restore có thể inspect/validate archive trước khi thay active store.
5. Backup không phụ thuộc absolute path, Windows user SID, DPAPI blob hoặc runtime binary của máy nguồn.
6. Compatibility phải fail closed khi target không chứng minh được đường restore/migration hợp lệ.

## Ranh giới phase

Phase 7.1 sở hữu:

- container/encryption contract;
- payload layout;
- manifest schema;
- checksum/path rules;
- compatibility rules;
- portable-secret policy;
- archive validator contract.

Phase 7.1 chưa dump database, chưa copy uploads và chưa thay active store. Database artifact thuộc 7.2, complete backup thuộc 7.3 và restore transaction thuộc 7.4.

## Định dạng file

User-facing extension:

~~~text
*.coffeepos-backup
~~~

Phase 7 dùng một encrypted container chứa ZIP payload. Encryption phải dùng format/library chuẩn đã được triển khai rộng rãi; không tự thiết kế cipher hoặc nonce protocol riêng.

Baseline cho schema 1:

- **age passphrase encryption** cho outer container;
- passphrase KDF/AEAD theo age format;
- ZIP chỉ tồn tại bên trong encrypted stream;
- không có unencrypted backup mode trong Phase 7;
- filename bên ngoài có thể chứa timestamp, nhưng không chứa store name/admin email.

Tên gợi ý:

~~~text
CoffeePOS-backup-YYYYMMDD-HHMMSS.coffeepos-backup
~~~

Backup password:

- người dùng nhập và xác nhận khi tạo backup;
- không persist vào config, log, backup manifest hoặc analytics;
- không dùng Windows DPAPI làm khóa duy nhất cho user backup vì archive phải restore được sang Windows profile khác;
- wrong password phải trả lỗi authentication rõ ràng trước mọi mutation restore;
- frontend phải clear password sau khi native operation hoàn tất/hủy/lỗi.

Phase 7.4 có thể dùng cùng container với random recovery password được bảo vệ bằng DPAPI cho **internal pre-restore recovery snapshot**. Snapshot nội bộ đó chỉ phục vụ rollback trên profile hiện tại, không được quảng bá là portable user backup.

## Payload schema 1

Sau khi decrypt, payload là ZIP với layout canonical:

~~~text
manifest.json
inventory.json
database/
└── store.sql
uploads/
└── ...
config/
└── store.json
secrets/
└── administrator.json
~~~

Không có entry nào được phép nằm ngoài các prefix trên.

### manifest.json

Manifest là source of truth của archive:

~~~json
{
  "schema_version": 1,
  "backup_id": "uuid",
  "created_at": "RFC3339 UTC",
  "kind": "portable_store_backup",
  "source": {
    "desktop_version": "0.1.0",
    "runtime_version": "2026-09-19.windows-dev.2",
    "target": "x86_64-pc-windows-msvc",
    "php_version": "8.4.25",
    "mariadb_version": "11.4.13",
    "wordpress_version": "7.1",
    "woocommerce_version": "11.1.0",
    "coffeepos_version": "1.0.1",
    "provisioning_schema": 1,
    "app_config_schema": 1
  },
  "database": {
    "format": "mariadb_logical_sql",
    "database_name": "coffeepos",
    "entry": "database/store.sql"
  },
  "uploads": {
    "root": "uploads/"
  },
  "store_config": {
    "entry": "config/store.json"
  },
  "administrator_secret": {
    "entry": "secrets/administrator.json",
    "present": true
  },
  "compatibility": {
    "minimum_restore_schema": 1,
    "requires_explicit_migration": false
  },
  "inventory_entry": "inventory.json",
  "total_files": 4,
  "total_uncompressed_bytes": 456,
  "warnings": []
}
~~~

`total_files` và `total_uncompressed_bytes` chỉ tính các **data entry** được liệt kê trong `inventory.json`; không tính `manifest.json` và `inventory.json`. `warnings` là danh sách **warning code được schema allowlist**, không phải free-form text. Schema 1 hiện cho phép `unmanaged_extensions_excluded`; UI tự map code này sang nội dung hiển thị. Cách này giữ manifest/IPC không mang secret, user path hay absolute source path.

Schema 1 cố định logical database name là `coffeepos`. Writer không nhận arbitrary database name và validator reject archive khai báo tên database khác.

Không đặt absolute source path, Windows username, SID, data-root hoặc runtime executable path vào manifest.

### inventory.json

Inventory liệt kê mọi **data entry**, không gồm manifest.json và inventory.json:

~~~json
{
  "schema_version": 1,
  "total_files": 4,
  "total_uncompressed_bytes": 456,
  "entries": [
    {
      "path": "database/store.sql",
      "size_bytes": 123,
      "sha256": "..."
    }
  ]
}
~~~

Yêu cầu:

- inventory không tự liệt kê/hash chính nó;
- manifest.json và inventory.json được bảo vệ integrity bởi authenticated encryption của outer age container;
- path dùng slash chuẩn trong archive;
- không có duplicate path sau normalize/case-fold theo Windows rule;
- SHA256 tính trên exact uncompressed bytes của entry;
- size trong inventory phải khớp exact bytes khi validate;
- validator phải reject missing entry, extra executable/control entry ngoài schema, checksum mismatch hoặc size mismatch.

Không dùng filename/hash của archive như bằng chứng duy nhất rằng backup hợp lệ.

## Store config portable

config/store.json chỉ chứa dữ liệu store cần tái tạo native config:

~~~json
{
  "schema_version": 1,
  "store_name": "My Coffee",
  "administrator": {
    "username": "owner",
    "email": "owner@example.com"
  }
}
~~~

Không copy raw config/app.json vì một số field là preference của profile đích. Khi restore:

- store_name + admin identity được khôi phục theo backup;
- startup view giữ/default theo profile đích;
- bind/network mode dùng policy của target, không bê cấu hình LAN từ máy nguồn;
- runtime port luôn được chọn lại;
- provisioning/repair journal được tạo lại từ kết quả restore, không copy journal cũ.

## Secret policy

Current protected files dùng Windows DPAPI nên không portable. Phase 7 không copy trực tiếp:

- config/database-runtime.secret;
- config/database-wordpress.secret;
- config/machine-token.secret;
- pending machine-token secret;
- pending onboarding/repair admin secret;
- bootstrap secret;
- raw repair protected snapshots.

Restore 7.4 phải:

1. tạo database runtime + WordPress DB credential mới trên target;
2. generate managed wp-config.php mới với credential target;
3. generate WordPress salts mới;
4. generate/rebind CoffeePOS machine token mới và update server-side hash trong staging database;
5. protect các secret mới bằng secret store của target profile.

Administrator password là ngoại lệ duy nhất của schema 1 vì Desktop có user-facing **Sao chép mật khẩu quản trị** và cần giữ trải nghiệm sau chuyển profile. secrets/administrator.json nằm **bên trong encrypted payload**:

~~~json
{
  "schema_version": 1,
  "username": "owner",
  "password": "<plaintext only inside authenticated encrypted payload>"
}
~~~

Quy tắc:

- active admin secret phải đọc được trước khi tạo portable backup;
- nếu active admin secret mất/unreadable hoặc có pending admin transaction, backup bị blocked và hướng người dùng sang Phase 6.3 reset/recovery;
- password không được log, đưa vào progress payload hoặc lưu plaintext ngoài encrypted payload;
- DB dump vẫn chứa WordPress password hash bình thường; restore phải verify plaintext admin secret khớp restored account trước commit.

## Những gì không nằm trong backup

Schema 1 không bundle:

- PHP/Caddy/MariaDB binaries;
- WordPress core;
- managed WooCommerce/CoffeePOS plugin files;
- site/wp-config.php;
- generated router/mu-plugin runtime files;
- database datadir;
- logs/support bundle;
- backups khác;
- repair/provisioning transaction staging/backup directories;
- browser cookies/local storage/session;
- Windows DPAPI blobs;
- arbitrary executable code từ unmanaged plugin/theme.

Managed code được tái tạo từ exact pinned artifact của target restore.

Nếu phát hiện unmanaged plugin/theme code trong site/wp-content, schema 1 ghi warning/inventory metadata đủ để UI nói rõ code đó không nằm trong portable backup. Không âm thầm quảng bá restore là clone đầy đủ executable environment.

## Path safety

Archive writer và validator dùng cùng canonical path policy:

- chỉ relative UTF-8 path;
- reject absolute path, drive prefix, UNC, .. segment, empty normalized segment và NUL;
- reject symlink/junction/reparse-point source cho uploads;
- reject archive entry có symlink/hardlink/special-file metadata;
- reject duplicate path sau Windows case-insensitive normalization;
- reject trailing dot/space ambiguity và Windows reserved device names;
- reject normalized path dài vượt implementation bound;
- mỗi extracted path phải canonical-contain trong restore staging root;
- không follow reparse point đã tồn tại ở destination staging.

Validation path safety chạy **trước extract**.

## Defensive size bounds

Schema 1 phải dùng ZIP64-capable streaming writer/reader vì backup thật có thể vượt 4 GiB. Không reuse support-bundle ZIP path của Phase 6.4 nếu implementation đó giữ archive trong memory hoặc có bound dành riêng cho log bundle.

Manifest/inventory khai báo total_files + total_uncompressed_bytes. Validator fail trước extract nếu metadata vượt hard safety policy của implementation hoặc vượt khả năng disk hiện tại.

Initial hard guard có thể dùng:

- tối đa 500.000 payload entries;
- tối đa 1 TiB total uncompressed bytes;
- tối đa 256 GiB cho một entry;
- tối đa 1.024 UTF-8 bytes cho normalized archive path.
- `manifest.json` tối đa 1 MiB;
- `inventory.json` tối đa 128 MiB;
- `config/store.json` và `secrets/administrator.json` tối đa 1 MiB mỗi entry.

Các bound này là anti-abuse/corruption guard, không phải quota sản phẩm và áp dụng đồng thời. Vì vậy inventory byte bound có thể chặn trước entry-count bound nếu path metadata lớn. Writer và validator phải dùng cùng các control-metadata bound; writer fail trước khi tạo container nếu metadata vượt bound. Nếu sau này cần tăng, bump policy/test rõ ràng thay vì silently bỏ bound.

## Compatibility policy

Phase 7 schema 1 dùng conservative compatibility:

1. backup schema phải được target hỗ trợ;
2. backup từ component/schema mới hơn target mà không có explicit migration path → blocked;
3. downgrade database/plugin schema → blocked;
4. initial Phase 7 acceptance dùng exact WordPress/WooCommerce/CoffeePOS/database baseline hiện tại;
5. future forward migration phải đăng ký rõ source range → target version + migration id; không suy từ semantic version rồi tự thử;
6. architecture/source OS không quyết định compatibility vì payload là logical data, nhưng Windows ↔ macOS chưa được công bố cho tới khi có acceptance trên cả hai target.

Restore không lấy binary từ backup. Target phải có pinned artifacts hợp lệ cho version/migration path mà nó hỗ trợ.

## Native validation contract

Tên command có thể theo convention implementation, nhưng contract cần hai lớp:

~~~text
inspect_backup(file_selected_by_native_dialog, backup_password)
  -> BackupInspection

validate_backup(file_selected_by_native_dialog, backup_password)
  -> BackupValidation
~~~

Frontend không truyền arbitrary extraction directory hay list entry.

Inspection trả safe metadata:

- backup id;
- created_at;
- store name;
- source versions;
- database/upload byte counts;
- compatibility result;
- warnings;
- can_restore.

Không trả administrator password hoặc decrypted SQL qua IPC.

Validation phải stream/decrypt với bounds hợp lý, validate ZIP central directory, path policy, manifest/inventory schema và mọi checksum. Unknown field chỉ được chấp nhận khi schema quy định forward-compatible; schema 1 mặc định reject contract-critical unknown structures.

## Failure model

Structured error tối thiểu:

~~~text
component = "backup"
action = "inspect" | "decrypt" | "validate"
code = stable machine-readable code
message = safe user-facing message
recovery = safe next action
~~~

Các lỗi chính:

- wrong password/authentication failure;
- unsupported backup schema;
- insufficient restore disk space / unavailable disk-capacity check;
- incompatible component/schema version;
- corrupt/truncated encrypted container;
- invalid ZIP;
- path escape/duplicate entry;
- missing manifest/inventory/required entry;
- checksum/size mismatch;
- unsupported custom executable content warning/blocker theo policy.

Không include decrypted secret/data trong error string.

## Functional acceptance

1. Valid schema-1 fixture decrypt + inspect + validate pass.
2. Wrong password fail trước extract/mutation.
3. Flip một byte trong encrypted file → authentication/validation fail.
4. Tamper inventory/checksum inside test fixture → fail.
5. Missing database/store.sql → fail.
6. Archive có ../, absolute path, duplicate case-variant hoặc symlink entry → fail.
7. Backup từ unsupported-newer schema/version → blocked với reason rõ.
8. Manifest không chứa absolute user/data-root path.
9. DPAPI secret blobs không xuất hiện trong archive.
10. Admin plaintext secret chỉ tồn tại bên trong encrypted payload và không xuất hiện trong filename/log/IPC.

## Validation dự kiến

Giữ focused:

~~~powershell
. .\scripts\use-local-rust.ps1
cargo fmt --manifest-path .\src-tauri\Cargo.toml -- --check
cargo test --manifest-path .\src-tauri\Cargo.toml --locked backup_format
cargo check --manifest-path .\src-tauri\Cargo.toml --locked
npm run lint:ui
npm run build:ui
git diff --check
~~~

Test fixture không dùng store thật. Secret canary phải scan cả file encrypted output, decrypted test payload, logs và IPC projection theo đúng boundary được phép.

## Ngoài phạm vi

- Database dump/import implementation: Phase 7.2.
- Complete user backup + Save As/progress UI: Phase 7.3.
- Restore/cutover/rollback: Phase 7.4.
- Cloud upload/sync.
- Scheduled automatic backup.
- Incremental/deduplicated backup.
- Unencrypted export.
- Backup arbitrary WordPress plugin/theme executable code.
- macOS acceptance.

## Definition of Done

Phase 7.1 chỉ hoàn thành khi schema 1 được implementation bằng encrypted portable container; validator có thể inspect/validate archive độc lập; manifest/inventory/checksum/path/compatibility rules được enforce ở native layer; wrong password/tamper/path escape fail closed; secret policy không copy DPAPI blob và admin plaintext chỉ tồn tại trong encrypted payload; focused tests chứng minh archive hợp lệ pass và fixture corrupt/incompatible bị từ chối trước mọi restore mutation.
