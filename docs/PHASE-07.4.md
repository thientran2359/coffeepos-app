# Phase 7.4 — Restore

Phase 7.4 hoàn thiện Backup/Restore bằng một restore transaction có inspect trước, staging riêng, health verification và rollback rõ ràng. Restore không extract trực tiếp lên active store. Mọi mutation active data chỉ được bắt đầu sau khi archive đã decrypt/validate/compatibility pass và current store có recovery snapshot hợp lệ khi cần thay dữ liệu hiện có.

> **Trạng thái:** đặc tả đã chốt cho implementation Phase 7.4. Windows-first. Cross-profile Windows restore là acceptance bắt buộc; Windows ↔ macOS chưa được công bố.

## Mục tiêu

1. Người dùng có thể chọn portable backup và xem nội dung/compatibility trước khi xác nhận.
2. Restore được dùng cả từ installed store và fresh Windows profile.
3. Active store không bị overwrite trực tiếp từ archive.
4. Target database/site/uploads/config được dựng trong staging và health pass trước cutover.
5. Existing store có validated pre-restore recovery backup trước mutation.
6. Crash/failure ở mọi cutover stage có deterministic rollback/resume.
7. Target profile tạo secret/credential mới; không copy DPAPI blob máy nguồn.
8. Final live store phải pass provisioning + health sau activation.

## Entry points

### Installed store

~~~text
Hệ thống
  └── Sao lưu và khôi phục
      └── Khôi phục
~~~

### Fresh profile / chưa setup

Welcome flow của Phase 5.2 bổ sung secondary action khi Phase 7.4 có implementation thật:

~~~text
Chào mừng đến CoffeePOS

[ Thiết lập cửa hàng mới ]
[ Khôi phục từ bản sao lưu ]
~~~

Fresh restore không bắt user tạo một store giả trước rồi overwrite.

## Restore UX

### Bước 1 — Chọn file

Native Open dialog chỉ cho chọn file backup phù hợp.

Frontend không truyền arbitrary extraction directory.

### Bước 2 — Nhập mật khẩu

~~~text
Mật khẩu bản sao lưu
[ •••••••••••• ]

[ Kiểm tra bản sao lưu ]
~~~

Nút này chỉ inspect/validate; chưa stop runtime, chưa mutate store.

### Bước 3 — Review

~~~text
Khôi phục cửa hàng

Cửa hàng: My Coffee
Tạo lúc: 19/09/2026 13:20
WordPress: 7.1
WooCommerce: 11.1.0
CoffeePOS: 1.0.1
Uploads: 842 MB

Tương thích với phiên bản CoffeePOS Desktop hiện tại.

Khôi phục sẽ thay dữ liệu cửa hàng hiện tại.
CoffeePOS sẽ tạo một bản sao an toàn của trạng thái hiện tại trước khi thay.

[ Hủy ] [ Khôi phục ]
~~~

Nếu target fresh, copy cảnh báo “thay dữ liệu hiện tại” được đổi thành “tạo cửa hàng từ bản sao lưu”.

Warning unmanaged site code từ manifest phải được hiển thị.

### Progress

~~~text
Đang khôi phục…

✓ Đã kiểm tra bản sao lưu
✓ Đã tạo bản sao trạng thái hiện tại
• Đang chuẩn bị dữ liệu
○ Kiểm tra hệ thống
○ Hoàn tất

Không đóng máy trong khi dữ liệu đang được chuyển.
~~~

### Success

~~~text
Khôi phục hoàn tất
CoffeePOS đã kiểm tra dữ liệu và hệ thống đã sẵn sàng.

[ Mở Tổng quan ]
~~~

### Failure

Nếu chưa mutation active store:

~~~text
Không thể khôi phục
Store hiện tại chưa bị thay đổi.

[ Thử lại ]
~~~

Nếu failure sau cutover nhưng rollback pass:

~~~text
Không thể khôi phục
CoffeePOS đã đưa cửa hàng về trạng thái trước khi khôi phục.

[ Kiểm tra hệ thống ]
~~~

Nếu rollback không thể verify:

~~~text
Khôi phục cần xử lý
CoffeePOS đã giữ lại bản sao an toàn và dữ liệu phục hồi.
Không tiếp tục bán hàng cho tới khi trạng thái cửa hàng được kiểm tra.

[ Xem chi tiết kỹ thuật ]
~~~

Không báo success chỉ vì file đã copy xong.

## Read-only inspect trước mutation

inspect_restore_backup dùng validator Phase 7.1:

- decrypt/authenticate;
- schema/path/checksum validation;
- compatibility;
- safe manifest projection;
- required data presence;
- unsupported warning.

Inspect có thể dùng private temp để decrypt/seek ZIP nếu library cần, nhưng:

- owned random staging path;
- private ACL;
- cleanup khi inspect kết thúc;
- không extract vào active store;
- không start/stop runtime;
- no plaintext secret qua IPC.

Validator còn phải reject Windows path ambiguity trước extract: case-collision, trailing dot/space ambiguity, reserved device names và entry có metadata hardlink/symlink/reparse. Path hợp lệ trên source nhưng normalize thành cùng target path trên Windows phải bị coi là archive không hợp lệ.

Kết quả inspect chứa opaque restore_candidate_id để native bind confirm/apply với exact file identity + validated manifest hash. Frontend không thể inspect file A rồi đổi path sang file B khi apply.

Nếu source file size/mtime/identity thay đổi sau inspect, apply phải revalidate hoặc trả stale_candidate.

## Compatibility gate

Initial schema 1 restore chấp nhận:

- backup schema 1;
- exact supported WordPress/WooCommerce/CoffeePOS/database baseline;
- hoặc source version có explicit registered forward migration trên target.

Blocked:

- unknown/newer backup schema;
- target thiếu pinned artifact cần dựng staging;
- database/plugin schema downgrade;
- migration chain không đầy đủ;
- corrupt/tampered inventory;
- unsupported path type;
- admin portable secret thiếu/không khớp contract schema 1.

Cross-profile Windows không phải compatibility error; đó là use case chính của portable secret policy.

## Restore transaction journal

Native tạo:

~~~text
config/restore.json
~~~

Journal schema phải versioned và atomic persist. Không chứa plaintext password/secret.

Conceptual stages:

~~~text
planned
validated
runtime_stopped
recovery_backup_ready
staging_prepared
database_imported
uploads_restored
target_secrets_bound
staging_verified
cutover_started
active_swapped
active_verified
committed
cleanup
rollback_started
rolled_back
~~~

Journal lưu:

- transaction id;
- backup id + manifest hash;
- source file identity safe fingerprint;
- target compatibility/migration ids;
- original_state = existing_store | no_previous_store;
- runtime_was_running;
- owned staging/rollback relative paths;
- per-component cutover evidence cho site/database/uploads/config để biết chính xác component nào đã move/swap;
- stage;
- timestamps;
- safe error.

Không lưu backup passphrase/admin password/DB password/machine token.

original_state phải được persist atomically ở stage planned/validated, trước recovery snapshot và chắc chắn trước cutover_started. Relaunch không được suy original_state từ việc rollback folder có hay không.

Daily auto-start, repair, provisioning và backup mới phải bị gate khi restore journal chưa committed/reconciled.

## Pre-restore recovery snapshot

Nếu current profile có installed managed store cần bị thay:

1. acquire restore operation guard;
2. dùng archive/snapshot primitives của Phase 7.3 để tạo **internal recovery snapshot**, nhưng restore transaction sở hữu maintenance lease và **không chạy bước resume runtime của 7.3**;
3. random strong recovery password được tạo native;
4. password bảo vệ bằng target DPAPI trong transaction-owned protected state;
5. archive nằm dưới managed backups/restore-recovery/<transaction-id>/;
6. Phase 7.1 validator phải pass;
7. journal persist recovery_backup_ready;
8. chỉ sau đó mới được tạo/cutover staging.

Internal snapshot không dùng user backup password và không được quảng bá là portable.

Từ lúc bắt đầu recovery snapshot cho tới khi restore **committed** hoặc rollback đã **rolled_back + verify pass**, active store phải giữ stopped/admission-fenced liên tục:

- không restart Caddy/PHP/cron sau recovery_backup_ready;
- daily auto-start bị gate;
- POS/browser admission không được mở lại;
- Phase 7.3 cleanup được gọi theo restore-owned mode để cleanup temp nhưng không restore runtime_was_running;
- runtime_was_running chỉ được áp dụng lại sau commit hoặc verified rollback.

Nhờ đó recovery snapshot luôn là điểm cuối cùng của old store trước cutover; không thể phát sinh order/upload mới sau snapshot rồi bị mất khi swap.

Nếu current target là fresh/unprovisioned và không có store data đáng giữ, journal ghi explicit no_previous_store; không tạo backup rỗng giả.

Recovery snapshot không bị xóa ngay khi restore success. Initial policy giữ **latest successful pre-restore recovery snapshot** cho tới khi một restore mới tạo recovery snapshot thay thế hoặc user thực hiện cleanup flow tương lai. Không tự tích lũy vô hạn.

## Staging architecture

Restore dựng complete store trong owned staging root, ví dụ:

~~~text
backups/restore-staging/<transaction-id>/store/
├── site/
├── database/
├── uploads/
├── config/
└── logs/
~~~

Staging root phải nằm cùng local filesystem với active data root khi cần atomic directory moves cho cutover.

Không dùng active site/database/uploads làm staging.

## Dựng target staging store

### 1. Managed code

Target tạo WordPress/WooCommerce/CoffeePOS files từ exact pinned artifact đã verify theo compatibility gate.

Không extract WordPress/plugin code từ backup.

### 2. Fresh MariaDB datadir

Initialize database datadir mới trong staging.

Tạo **new target credentials**:

- runtime DB credential;
- WordPress DB credential.

Credential không lấy từ source DPAPI blob.

Tạo application DB rồi import database/store.sql bằng pinned mariadb client.

Không import mysql system database/user tables.

### 3. Uploads

Extract uploads entries vào staging/uploads sau path/checksum validation.

Không follow/create symlink/reparse.

### 4. Generated config

Generate target wp-config.php mới:

- target DB credential;
- staging runtime origin/dynamic port contract;
- new WordPress salts;
- existing managed markers;
- no absolute source path.

Generate router + managed uploads bridge bằng current target templates.

### 5. App/store config

Apply portable store identity:

- store_name;
- admin username/email.

Preserve target-local preference khi hợp lệ:

- startup view;
- future local-only/LAN policy.

Fresh target dùng default local preferences.

### 6. Administrator secret

Decrypt administrator password chỉ trong native restore transaction.

Sau database import:

- verify backup username tồn tại;
- verify password khớp restored WordPress password hash qua managed local PHP bootstrap;
- nếu mismatch → staging blocked, không reset âm thầm;
- protect password bằng target profile secret store.

### 7. Machine token

Không reuse source machine token.

Generate new token trên target, persist protected pending state trước server-side update, update CoffeePOS machine-token hash trong staging database qua pinned local PHP, probe authenticated health, rồi promote active token theo transaction semantics tương tự Phase 4.10/6.3.

### 8. Database credential binding

WordPress DB user/password được target tạo mới; wp-config trỏ credential mới.

Runtime DB user/password cũng target-local.

Restore phải verify cả hai accounts bằng authenticated SELECT 1 trước staging health.

## Migration

Schema 1 initial exact-version restore có migration stage nhưng có thể no-op.

Future migration:

- explicit source range;
- ordered migration ids;
- idempotent/restart-safe hoặc journaled;
- chạy trong staging;
- backup source archive không bị mutate;
- failure giữ current active store nguyên vẹn.

Không “activate plugin rồi hy vọng plugin tự migrate đúng” làm migration contract duy nhất.

## Staging health

Start staging trong **maintenance verification mode** trên isolated dynamic loopback ports. Mode này chỉ chạy các process/probe cần cho verification; **không chạy WP-Cron/background jobs, không mở POS/browser admission và không expose endpoint ra LAN**. Không dùng normal daily-start path nếu path đó tự spawn cron.

Verify tối thiểu:

- MariaDB authenticated readiness;
- PHP/web readiness;
- WordPress loads;
- WooCommerce exact active baseline;
- CoffeePOS exact active baseline;
- machine-health authenticated;
- store name/admin identity;
- selected database sentinel/invariant;
- uploads bridge phục vụ fixture file.

Staging runtime phải stop sạch trước cutover.

Health failure → delete/preserve staging evidence theo error policy, current active store chưa bị thay.

## Cutover

Chỉ chạy khi:

- archive validation pass;
- recovery snapshot ready/no_previous_store;
- staging build pass;
- staging health pass;
- staging runtime stopped;
- active runtime stopped;
- không còn managed Caddy/PHP/cron/MariaDB child của staging hoặc active store;
- restore journal fsync/persist cutover_started.

Cutover dùng owned same-volume moves, không file-by-file overwrite live tree.

Conceptual:

~~~text
active/site      → rollback/site
active/database  → rollback/database
active/uploads   → rollback/uploads

staging/site      → active/site
staging/database  → active/database
staging/uploads   → active/uploads

atomically write target config + protected secrets
        ↓
persist active_swapped
~~~

logs/ và backups/ là target-local và không swap từ source archive.

desktop.lock ở data-root không được move.

Config cutover là per-file atomic persist với protected rollback copy/snapshot vì active config directory còn chứa restore journal/backups state.

## Final active verification

Sau swap:

1. start **restore maintenance verification mode** trên active paths;
2. giữ WP-Cron/background jobs disabled và POS/browser admission blocked;
3. run provisioning inspection;
4. run Phase 6.1-equivalent health probes qua private loopback verification ports;
5. verify admin secret/login bootstrap;
6. verify machine token;
7. verify upload sentinel/access;
8. stop verification runtime sạch, chứng minh không còn managed child;
9. persist active_verified;
10. persist committed;
11. chỉ sau commit mới release restore admission gate.

Sau committed:

- nếu runtime_was_running, start normal RuntimeManager rồi trở về daily health scheduling;
- nếu previous store stopped, giữ runtime stopped;
- fresh restore áp dụng normal post-setup startup policy.

Không mở normal Caddy/PHP/cron/POS trước committed. Điều này giữ restored state ổn định trong toàn bộ rollback window.

## Rollback

Failure sau cutover_started nhưng trước committed:

### Existing previous store

1. persist rollback_started;
2. stop any target active runtime;
3. move failed restored trees sang transaction quarantine;
4. move rollback/site/database/uploads về active;
5. restore old config/protected secret snapshots;
6. verify previous provisioning state + health bằng maintenance verification mode;
7. stop verification runtime;
8. persist rolled_back;
9. nếu runtime_was_running và rollback đã verify, start normal runtime;
10. giữ recovery snapshot + failed staging/quarantine evidence nếu cleanup không an toàn.

Không xóa rollback evidence trước khi old store đã verify.

### no_previous_store

Nếu journal ghi target ban đầu là fresh/unprovisioned:

1. persist rollback_started;
2. stop verification/target runtime và chứng minh không còn managed child;
3. quarantine hoặc xóa **chỉ** site/database/uploads/config artifacts có transaction ownership marker của restore này;
4. restore pre-restore local app/preferences snapshot nếu có; không cố tìm rollback/site/database/uploads không hề tồn tại;
5. xóa target protected secrets chỉ khi journal chứng minh chúng được transaction này tạo mới;
6. verify provisioning trở về explicit NotInstalled/fresh state;
7. verify không còn managed store runtime/process;
8. persist rolled_back;
9. release admission gate về welcome/setup flow.

Expected absence của rollback trees trong no_previous_store là hợp lệ, không được phân loại thành missing rollback evidence.

Nếu rollback verify fail, app vào needs_recovery state và **không daily auto-start**. UI hướng người dùng tới Hệ thống/nhật ký và giữ mọi owned recovery evidence.

## Crash recovery

Relaunch đọc restore journal **trước** daily startup.

Policy:

- validated/recovery_backup_ready/staging_* trước cutover → active store chưa đổi; cleanup/resume staging an toàn;
- cutover_started → inspect exact active/rollback/staging ownership markers rồi complete swap hoặc rollback deterministic;
- active_swapped chưa active_verified → chỉ verify restored state bằng maintenance verification mode; nếu không chứng minh được thì rollback;
- rollback_started → hoàn tất rollback;
- committed → cleanup leftover staging theo ownership marker, không rollback thành công cũ.

Recovery classification phải đọc original_state từ journal. Với no_previous_store, absence của old rollback trees/secrets là expected; recovery chỉ yêu cầu transaction-owned restored artifacts có thể quarantine/remove an toàn và fresh state có thể verify.

Nếu evidence thiếu/mâu thuẫn khiến native không thể chứng minh deterministic continue/rollback, recovery classification là **blocked**, runtime giữ stopped và UI đưa ra hành động phục hồi rõ ràng. Không đoán transaction state từ một folder còn sót.

Không suy stage chỉ từ folder name. Journal + transaction marker + expected hash/backup id phải khớp.

## Native command contract

Conceptual:

~~~text
inspect_restore_backup(backup_password) -> RestoreInspection
apply_restore(candidate_id, backup_password) -> RestoreResult
get_restore_status() -> RestoreStatus
cancel_restore(operation_id) -> CancelResult
~~~

Native Open dialog thuộc inspect command/flow.

candidate_id bind:

- selected file canonical identity;
- backup id;
- encrypted file hash/size/mtime;
- validated manifest hash;
- compatibility decision.

Apply rechecks candidate trước mutation.

Frontend không gửi target data root, extraction root, SQL command hoặc artifact path.

## Cancellation

Cancel được hỗ trợ theo stage:

- trước maintenance/cutover: cleanup staging rồi return cancelled;
- đang tạo recovery backup: dùng cleanup semantics Phase 7.3;
- sau staging health nhưng trước cutover: cleanup staging, current store unchanged;
- sau cutover_started: không “cancel” bằng cách bỏ dở; request chuyển thành rollback-to-previous-store transaction.

UI disable close action có thể gây abandoned mutation; Alt+F4/close phải đi qua existing shutdown/operation guard và chờ bounded cleanup/rollback contract.

## Security

1. Backup passphrase/admin password không log.
2. Decrypted SQL/admin secret không qua frontend.
3. All extraction path validated before write.
4. No source DPAPI secret copied.
5. Restore creates target DB/machine credentials.
6. Native only executes pinned MariaDB/PHP artifacts with explicit args.
7. No arbitrary SQL supplied by frontend.
8. Decrypted staging ACL hạn chế current user.
9. Temp/recovery paths ownership-bound và cleanup không follow reparse.
10. Backup may contain personal/order data; UI không preview customer/order records.

## Functional acceptance

### Happy path

1. Backup fixture store có product/order/upload.
2. Restore sang **Windows user/profile khác**.
3. New profile không có source DPAPI files.
4. Restore staging health pass.
5. POS mở được; store name đúng.
6. Product/order sentinel tồn tại.
7. Upload sentinel phục vụ đúng bytes.
8. Admin login bằng original backup password của WordPress pass.
9. Desktop Copy admin password dùng target-protected restored admin secret.
10. Machine-health auth dùng **new target machine token**.
11. DB accounts dùng **new target credentials**.

### Existing-store replacement

12. Current store A có sentinel riêng.
13. Restore backup store B.
14. Pre-restore recovery snapshot A validate pass trước cutover.
15. B success → active store là B.
16. Recovery snapshot A vẫn tồn tại theo retention policy.

### Failure/rollback

17. Wrong backup password → no mutation.
18. Checksum/path/compatibility fail → no mutation.
19. DB import fail → current store unchanged.
20. Upload extract fail → current store unchanged.
21. Staging health fail → current store unchanged.
22. Inject failure sau first directory swap → rollback A pass.
23. Inject failure sau active_swapped trước health → rollback A pass.
24. Kill app ở từng restore journal boundary chính → relaunch deterministic resume/rollback, không daily auto-start sớm.
25. Rollback failure fixture giữ recovery evidence và app không tự chạy store không xác minh.
26. Fresh-profile restore inject failure sau cutover_started/active_swapped → rollback về NotInstalled, không đòi rollback trees không tồn tại; relaunch vẫn vào welcome/setup flow.
27. Crash/relaunch fixture chứng minh journal original_state được persist trước cutover và recovery phân biệt existing_store với no_previous_store chỉ từ journal + ownership evidence.

### Lifecycle/UI

28. Restore khi source runtime running: after failed restore/rollback, old runtime trở lại running nếu verify pass.
29. Restore khi source runtime stopped: không để old/new runtime running sau rollback/success ngoài one-time verify.
30. Duplicate click/reload không tạo restore thứ hai.
31. Close/Alt+F4 trong restore không orphan MariaDB/Caddy/PHP.
32. Keyboard/focus/resize/150% DPI thao tác được picker/password/review/confirm/progress/result.

## Validation dự kiến

~~~powershell
. .\scripts\use-local-rust.ps1
cargo fmt --manifest-path .\src-tauri\Cargo.toml -- --check
cargo test --manifest-path .\src-tauri\Cargo.toml --locked restore
cargo check --manifest-path .\src-tauri\Cargo.toml --locked
npm run lint:ui
npm run build:ui
git diff --check
~~~

Manual Windows acceptance dùng disposable profiles:

1. profile A tạo store + sentinel → backup;
2. profile B fresh → restore;
3. login/POS/order/upload verify;
4. profile B tạo store khác → restore đè có recovery backup;
5. thử wrong password/corrupt archive;
6. inject staging import/health failure;
7. inject cutover failure + relaunch recovery;
8. Task Manager/ports kiểm không orphan.

Không failure-test trên cửa hàng đang vận hành.

## Ngoài phạm vi

- Windows ↔ macOS portability claim trước khi có macOS acceptance.
- Restore backup từ unknown newer schema.
- Database downgrade.
- Restore arbitrary custom plugin/theme executable code.
- Merge hai store.
- Selective table/order/product restore.
- Cloud backup/sync.
- Point-in-time/binlog recovery.
- Automatic scheduled restore.

## Definition of Done

Phase 7.4 chỉ hoàn thành khi user có thể inspect + restore encrypted backup từ installed store và fresh Windows profile; archive được validate/compatibility-check trước mutation; existing store có validated internal recovery snapshot; target được dựng trong isolated staging với fresh DB credentials/salts/machine token + target-protected admin secret; staging health pass trước cutover; active cutover có journal + rollback deterministic; failure/crash ở các boundary chính giữ hoặc khôi phục previous store; final active provisioning + health pass; Windows cross-profile E2E và failure/rollback smoke pass.
