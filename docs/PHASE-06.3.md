# Phase 6.3 — Repair flow

Phase 6.3 biến trạng thái `needs_repair` và các lỗi managed-store có thể khôi phục thành một flow sửa chữa thật trong **Hệ thống**. Repair phải dựa trên bằng chứng ownership/version hiện có, dùng đúng artifact/runtime đã pin và luôn ưu tiên giữ nguyên database, uploads, credential hợp lệ và dữ liệu nghiệp vụ. Trường hợp native không chứng minh được ownership hoặc không có đường rollback an toàn phải dừng ở trạng thái **Không thể tự sửa** thay vì đoán rồi ghi đè.

> **Trạng thái implementation 2026-09-19:** native RepairPlan/apply flow, repair journal crash recovery, managed WordPress/plugin repair, explicit admin-password reset, pending machine-token recovery và UI **Hệ thống → Sửa chữa** đã được triển khai. Focused Rust repair tests + UI typecheck/build + format/diff checks pass; manual Windows disposable-store acceptance vẫn chờ người dùng smoke-test nên phase chưa được đánh dấu hoàn thành theo Definition of Done.

## Mục tiêu

1. Người vận hành nhìn thấy chính xác thành phần nào có thể sửa tự động, thành phần nào cần hành động thủ công và lý do.
2. Repair chỉ thay phần CoffeePOS Desktop quản lý và chỉ khi ownership/compatibility đủ bằng chứng.
3. Không xóa/reinitialize MariaDB datadir, WordPress tables, uploads, dữ liệu WooCommerce/CoffeePOS hoặc plugin không thuộc CoffeePOS Desktop.
4. Không dùng repair để âm thầm reset database/admin credential, machine token hoặc store identity.
5. Repair file/plugin cùng version dùng artifact đã pin, staging + atomic swap và verify trước khi kết luận thành công.
6. Runtime/lifecycle không chạy song song với mutation repair; UI vẫn phản hồi trong lúc inspect/apply/verify.
7. Sau repair phải chạy lại provisioning inspection + Phase 6.1 health để chứng minh store trở về trạng thái dùng được.

## Nguyên tắc an toàn

Repair không đồng nghĩa với “reinstall”. Mọi action phải thuộc một trong ba lớp:

| Kết quả inspect | Ý nghĩa | UI/action |
| --- | --- | --- |
| `repairable` | Native có đủ ownership + version + recovery evidence và có mutation/rollback xác định | Cho phép **Sửa chữa** |
| `requires_input` | Có thể repair nhưng cần người dùng cung cấp dữ liệu mới hoặc xác nhận impact, ví dụ reset admin credential | Thu input/xác nhận trước khi apply |
| `blocked` | Ownership/compatibility/credential authority không đủ hoặc repair có nguy cơ phá dữ liệu | Không có nút auto-repair; giải thích bước tiếp theo |

Không được suy `repairable` chỉ từ việc file/path mang đúng tên. Journal, managed marker, ownership metadata, pinned manifest và live verifier phải được dùng theo từng loại target.

## Phạm vi repair

### 1. Managed generated files

Các file Desktop sinh ra có thể được restore khi provenance của store đã được chứng minh:

- `config/wordpress-router.php`;
- `site/wp-content/mu-plugins/coffeepos-desktop-runtime.php`;
- runtime-generated config chỉ chứa deterministic managed content và không chứa secret;
- `wp-config.php` theo policy riêng bên dưới.

Router/uploads bridge bị thiếu có thể được tạo lại nếu provisioning journal + managed WordPress site khớp store hiện tại. Nếu path đã tồn tại nhưng không có managed marker đúng, repair phải `blocked` và preserve file.

Với `wp-config.php`:

- file có managed marker nhưng layout managed bị thiếu/hỏng có thể repair bằng canonical template khi native đọc được các protected DB credentials và chứng minh site thuộc store hiện tại;
- nếu có thể đọc các salts hiện hữu thì phải giữ nguyên chúng;
- nếu file đã mất hoàn toàn nhưng ownership của store vẫn được chứng minh độc lập, UI phải báo rõ việc tạo config mới sẽ tạo salts mới và làm các WordPress session hiện tại hết hiệu lực; đây là `requires_input`, không chạy ngầm;
- file tồn tại nhưng không có CoffeePOS managed marker luôn `blocked`; Phase 6.3 không phải adoption/import flow.

### 2. WordPress core cùng pinned version

Repair được phép restore file core bị thiếu/corrupt khi tất cả điều kiện sau đúng:

- journal xác nhận store đã qua `site_ready`/WordPress install trên cùng managed `data_root`;
- WordPress version mục tiêu đúng pinned baseline của Desktop hiện tại;
- managed config/site identity khớp;
- repair chỉ restore WordPress core file set từ artifact đã verify.

Không được xóa hoặc replace toàn bộ `site/`. `wp-content/`, `wp-config.php`, uploads và file không thuộc core set phải được preserve. Phase 6.3 không upgrade/downgrade WordPress version.

### 3. WooCommerce và CoffeePOS plugin files

WooCommerce/CoffeePOS repair chỉ làm **same-version file repair**, không phải plugin upgrade:

- destination còn tồn tại: `.coffeepos-managed.json` phải parse được và khớp schema, slug, version và pinned artifact SHA256;
- destination bị mất hoàn toàn: journal phải chứng minh plugin exact version trước đó thuộc managed store và target path hiện đang trống;
- ownership metadata thiếu/unreadable trong một destination đang tồn tại, slug/schema không tương thích hoặc version/hash khác pinned baseline → `blocked`;
- plugin khác trong `wp-content/plugins` tuyệt đối không được mutate.

Apply dùng verified staged artifact, tạo replacement tree ngoài live plugin directory, rồi atomic swap. Existing managed directory được giữ tạm làm rollback copy cho tới khi file/header/activation verifier pass. Vì Phase 6.3 chỉ repair cùng version, action không được chạy migration để đổi schema version như một upgrade flow.

### 4. Plugin activation / managed baseline

Nếu files exact managed nhưng plugin inactive hoặc baseline verifier fail ở phần có thể replay an toàn, repair có thể chạy lại activation/verifier đã có từ Phase 4.6/4.9. Replay phải giữ business tables/options và không reset onboarding/store settings.

Nếu verifier phát hiện schema/version state không tương thích với pinned same-version repair, action phải `blocked` thay vì thử downgrade database.

### 5. Credential repair

Credential repair luôn là explicit action và phải phân biệt authority của từng secret.

**CoffeePOS machine token** có ba case riêng. Khi active protected token còn đọc được **và endpoint vẫn chấp nhận**, repair có thể tái sử dụng primitive rotation active + pending của Phase 4.10. Khi có pending protected token từ transaction cũ và endpoint đã chấp nhận pending token, repair chỉ resume/promote transaction đó theo recovery semantics hiện có. Khi active token mất/unreadable/not accepted **và không có pending token đã được endpoint chấp nhận**, primitive rotation hiện tại **không đủ** vì nó yêu cầu chứng minh active token trước khi switch. Case này chỉ được `repairable` nếu Phase 6.3 bổ sung explicit independent-authority recovery primitive: managed CoffeePOS ownership phải hợp lệ; WordPress phải load được qua current verified DB credential; native phải snapshot exact server-side token hash vào protected repair state trước mutation; persist pending token trước; set hash mới bằng pinned local PHP; probe pending token; promote active secret khi pass. Failure/crash phải restore hoặc resume từ protected hash snapshot + pending state một cách deterministic. Nếu thiếu bất kỳ authority/snapshot/restore condition nào thì plan là `blocked`; không overwrite server hash chỉ vì Desktop sở hữu plugin path. Plaintext token không được trả qua IPC hoặc log.

**WordPress administrator credential** không thể “khôi phục plaintext” từ WordPress hash. Nếu protected admin secret mất/unreadable, Phase 6.3 chỉ được sửa bằng flow **Đặt lại mật khẩu quản trị** có người dùng nhập replacement password. Native tái sử dụng **protected pending-secret pattern** của Phase 5.2 nhưng gắn nó với repair journal riêng: persist pending secret trước, update đúng existing administrator account qua local managed bootstrap, verify password + identity, rồi mới promote active protected secret. Crash sau WordPress update nhưng trước promote phải giữ pending secret để relaunch resume/complete transaction; không được coi active secret cũ là thành công. Không tự sinh password mới khi mở app hoặc khi bấm Repair chung.

**WordPress database credential** bị mất, không giải mã được hoặc không còn khớp account trong MariaDB là `blocked` trong Phase 6.3. Sau provisioning, CoffeePOS đã retire bootstrap/root credential; runtime DB account hiện chỉ có authority phục vụ lifecycle như `SHUTDOWN`, không có quyền quản trị để `ALTER USER` WordPress một cách hợp lệ. Repair không được mở MariaDB bằng bypass-auth hoặc tự tạo một privileged recovery path chỉ để reset password.

**MariaDB runtime credential** bị mất hoặc không giải mã được cũng `blocked`. Phase 6.3 giữ nguyên datadir và yêu cầu restore matching protected credential hoặc một DB recovery procedure được thiết kế/nghiệm thu riêng trong tương lai; tuyệt đối không reinitialize database.

## Trường hợp bắt buộc từ chối auto-repair

Các trường hợp dưới đây không được có đường “Sửa tất cả”:

- `config/provisioning.json` unreadable/corrupt khiến ownership/journal không còn đáng tin;
- existing WordPress core/site không có CoffeePOS ownership evidence;
- `wp-config.php` tồn tại nhưng không có managed marker;
- WooCommerce/CoffeePOS directory tồn tại nhưng ownership metadata thiếu, invalid hoặc incompatible;
- plugin/core version khác pinned version hiện tại và cần upgrade/downgrade;
- `partial_wordpress_install` từ Phase 4.12 hoặc các `wp_*` tables không chứng minh được trạng thái cài đặt an toàn;
- WordPress/runtime DB credential mất, unreadable hoặc mismatch; current post-provisioning store không còn privileged bootstrap credential để rotate account an toàn;
- filesystem path resolve ra ngoài managed `data_root`, symlink/reparse target bất thường hoặc permission/ownership làm native không chắc target là managed path;
- verifier cho thấy schema database mới hơn/cũ hơn mức same-version repair có thể chứng minh an toàn.

`partial_wordpress_install` đặc biệt phải **preserve tables**. Phase 6.3 có thể giải thích blocker và thu thêm diagnostic evidence, nhưng không được drop/recreate `wp_*` tables để “thử lại”. Backup/restore thuộc Phase 7; adoption/import store ngoài ownership contract là scope riêng trong tương lai.

## Repair flow

Flow chuẩn có năm bước:

```text
Hệ thống → Sửa chữa
        │
        ├── 1. Kiểm tra
        │      provisioning inspect + ownership + manifests + health
        │
        ├── 2. Kế hoạch sửa chữa
        │      repairable / requires_input / blocked
        │
        ├── 3. Xác nhận
        │      impact + runtime restart + credential input nếu cần
        │
        ├── 4. Sửa chữa
        │      stop/drain → stage → atomic mutation → verifier
        │
        └── 5. Kiểm tra lại
               provisioning ready + runtime start + health snapshot
```

Inspect là read-only và có thể chạy nhiều lần. Apply phải dùng plan mới nhất; nếu filesystem/journal/version thay đổi từ lúc plan được tạo, native trả stale-plan error và yêu cầu inspect lại thay vì tiếp tục với assumption cũ.

**UX rule:** mở **Hệ thống → Sửa chữa** không tự gọi get_repair_plan và không tự chạy bất kỳ repair operation nào. Màn hình vào trạng thái **Chưa kiểm tra**; người dùng phải bấm **Kiểm tra** để tạo repair plan read-only. Sau khi đã inspect, nút đổi thành **Kiểm tra lại**. Nếu plan stale hoặc apply lỗi, app không tự inspect lại; người dùng chủ động bấm **Kiểm tra lại** trước khi có thể chạy **Sửa chữa** lần tiếp theo.

## Planned native contract

Tên type/command dưới đây là contract mục tiêu để implementation bám theo; có thể đổi tên trong code trước khi phase hoàn tất nếu semantics giữ nguyên và docs được cập nhật cùng commit.

```text
get_repair_plan() -> RepairPlan
apply_repair(plan_id, inputs?) -> RepairResult
```

`RepairPlan` tối thiểu gồm:

```text
plan_id
generated_at / generation token
store_state
runtime_was_running
items[]:
  id
  component
  target
  classification: repairable | requires_input | blocked
  action
  reason
  impact
  requires_runtime_stop
can_apply
```

`RepairResult` tối thiểu gồm:

```text
plan_id
status: repaired | partial | failed | stale
items[] result
provisioning_info
health_diagnostics?
last_error?
```

Structured `RuntimeErrorInfo { component, operation, message, recovery }` tiếp tục là error contract. Frontend không parse error string để quyết định repairability.

Các native invariants:

- `get_repair_plan` serialize với provisioning/lifecycle state đủ để không inspect giữa một mutation đang chạy;
- `apply_repair` dùng cùng exclusive maintenance/lifecycle guard với provisioning/start/stop/restart/diagnostics mutation;
- command có filesystem/process/DB I/O chạy ngoài Tauri UI thread như Phase 6.2;
- `plan_id` phải gắn với một fingerprint/generation của journal + target metadata để chặn stale apply;
- repair command không nhận arbitrary filesystem path, URL, executable hoặc plugin slug từ frontend;
- frontend chỉ gửi repair item/plan ID do native vừa phát hành và credential input cần thiết;
- plaintext secret không xuất hiện trong `RepairPlan`, `RepairResult`, logs hoặc frontend persisted state.

## Lifecycle khi apply

Mutation file/plugin/config không chạy trong lúc Caddy/PHP/cron đang phục vụ request. Apply luôn snapshot `runtime_was_running`; nếu runtime đang chạy thì phải drain/stop trước mutation, còn nếu đang stopped thì phải xác nhận không còn managed child trước khi chạm file/config/plugin:

1. UI phải báo trước rằng CoffeePOS sẽ tạm dừng trong lúc sửa;
2. native dùng Phase 6.2 graceful Caddy drain + stop PHP/cron/MariaDB theo lifecycle contract hiện có;
3. apply staged/atomic repair;
4. chạy verifier offline/local CLI cần thiết;
5. start runtime khi repair plan cần online verification; nếu runtime trước repair vốn đang chạy thì đây cũng là restore lifecycle state;
6. chạy `get_health_diagnostics()`/machine-health sau start và capture kết quả verification;
7. nếu runtime trước repair vốn **stopped**, stop lại sau verification để trả đúng trạng thái vận hành ban đầu;
8. chỉ báo **Đã sửa xong** khi target repair + provisioning state + verifier liên quan đều pass; health snapshot được báo từ lượt verify ngay cả khi runtime sau đó được trả về `stopped`.

Nếu stop không hoàn tất hoặc còn managed child, repair phải abort trước mutation. Nếu mutation đã bắt đầu nhưng verify fail, rollback file tree/config từ repair backup khi rollback đó không thể làm lệch schema/data; nếu rollback không còn an toàn, giữ target ở trạng thái failed, preserve cả evidence/backup và không tự chạy thêm mutation.

## Atomicity và repair journal

Phase 6.3 cần repair transaction/journal riêng hoặc extension backward-compatible của provisioning journal để relaunch biết repair đang ở bước nào. Tối thiểu phải phân biệt:

```text
planned
runtime_stopped
staged
swapped
verified
committed
```

Mỗi side effect phải hoàn tất trước khi journal advance. Relaunch sau crash phải inspect transaction và chọn một trong ba kết quả deterministic: resume verifier, rollback owned file swap, hoặc `blocked` cần người dùng xử lý. Không xóa repair backup chỉ vì process restart.

Repair staging/backup chỉ được tạo dưới managed `data_root`, dùng tên/path do native cố định và cleanup khi transaction committed. Không dùng user-provided path.

## UI trong Hệ thống

Repair là chức năng cấp hai của **Hệ thống**, cùng hierarchy với **Chẩn đoán**; không thêm top-level tab mới.

### Chưa kiểm tra

```text
Hệ thống > Sửa chữa

Sửa chữa hệ thống
Chưa kiểm tra
Nhấn Kiểm tra để CoffeePOS lập repair plan read-only.

[ Kiểm tra ]
```

### Có kế hoạch sửa chữa

```text
Hệ thống > Sửa chữa

Phát hiện 2 mục có thể sửa

WordPress router     Có thể sửa
  File managed bị thiếu. Sẽ tạo lại từ baseline CoffeePOS.

CoffeePOS plugin     Có thể sửa
  File plugin managed không khớp baseline 1.0.1.
  Dữ liệu cửa hàng trong database không bị xóa.

Trong lúc sửa, runtime có thể tạm dừng/khởi động để xác minh và sẽ được trả về trạng thái vận hành trước khi sửa.

[ Sửa chữa ]   [ Kiểm tra lại ]
```

### Cần input/xác nhận riêng

```text
Mật khẩu quản trị cần được đặt lại

CoffeePOS không thể khôi phục mật khẩu cũ từ WordPress.
Nhập mật khẩu mới để cập nhật đúng tài khoản quản trị hiện tại.

Mật khẩu mới        [••••••••••••]
Xác nhận mật khẩu   [••••••••••••]

[ Đặt lại và kiểm tra ]
```

Password không được persist ở localStorage/sessionStorage/config JSON; sau submit frontend phải clear field/state theo contract Phase 5.2.

### Blocked

```text
Không thể tự sửa an toàn

WordPress database có dấu hiệu cài đặt dang dở.
CoffeePOS sẽ giữ nguyên các bảng hiện có vì không thể xác định phần nào chứa dữ liệu cần giữ.

Không có thay đổi nào được thực hiện.
[ Kiểm tra lại ]
```

Blocked state không được đổi nhãn thành “Thử lại” nếu action kế tiếp vẫn là cùng mutation bị từ chối.

### Đang sửa

```text
Đang sửa chữa hệ thống…

✓ Đã dừng runtime an toàn
✓ Đã chuẩn bị artifact
• Đang xác minh CoffeePOS plugin
○ Khởi động lại runtime
○ Kiểm tra sức khỏe

[ action disabled ]
```

Double click, reload WebView hoặc chuyển navigation không được tạo repair operation thứ hai. Relaunch đọc native repair transaction thay vì frontend tự đoán progress.

### Thành công

```text
Sửa chữa hoàn tất

2 mục đã được khôi phục.
Database và dữ liệu cửa hàng được giữ nguyên.
Các kiểm tra liên quan đều đạt và runtime đã được trả về trạng thái trước khi sửa.

[ Mở Chẩn đoán ]   [ Mở bán hàng ]
```

## Acceptance

1. Healthy/ready store trả plan rỗng; inspect không mutate filesystem, DB, credential, runtime process count hoặc plugin state.
2. Missing managed router/uploads bridge trên store ownership hợp lệ được plan là repairable, restore deterministic và health/provisioning verify pass.
3. Existing conflicting router/config file không có managed marker bị preserve và plan `blocked`; không overwrite file.
4. Corrupt/missing WordPress core file trên exact managed version được restore từ verified pinned artifact mà giữ nguyên `wp-content`, `wp-config.php`, uploads và DB business sentinel.
5. WooCommerce/CoffeePOS exact managed ownership + corrupt file/header được same-version atomic repair; plugin khác và DB business sentinel giữ nguyên.
6. Plugin ownership metadata thiếu/invalid/incompatible hoặc version khác pinned baseline bị `blocked`; không adopt, upgrade hay downgrade ngầm.
7. Repair activation/verifier replay không reset WooCommerce/CoffeePOS store options, roles/capabilities ngoài managed baseline hoặc business rows.
8. Machine-token case active-valid dùng rotation hiện có; accepted-pending case resume transaction hiện có; active missing/unreadable/not accepted + no accepted pending chỉ repairable khi independent WordPress authority verify được và protected server-hash snapshot/restore path tồn tại. Thiếu điều kiện này phải `blocked`. Injected failure/crash phải resume/rollback đúng và không lộ plaintext; không được gọi rotation cũ rồi overwrite hash khi active authority đã mất.
9. Missing admin protected credential không tự sinh secret. Chỉ explicit password-reset flow mới thay WordPress password + protected secret và phải verify đúng existing admin identity.
10. WordPress/runtime DB credential missing/unreadable/mismatch đều `blocked` với implementation hiện tại; repair không dùng `--skip-grant-tables`, không tạo root recovery ngầm và datadir vẫn nguyên vẹn.
11. `partial_wordpress_install` giữ `wp_*` tables và `recovery_blocker`; Repair giải thích không thể tự sửa, không drop/reinstall database.
12. Runtime đang chạy: apply báo impact, graceful drain/stop trước mutation, restart + health sau repair. Runtime ban đầu stopped: online verifier có thể start tạm nhưng phải stop lại sau verify. Stop/cleanup failure ở cả hai đường abort trước mutation hoặc giữ transaction ở failed state phù hợp.
13. Crash/relaunch ở từng repair transaction boundary không tạo duplicate mutation; native resume/rollback/block deterministic và giữ repair backup khi cần.
14. Stale plan bị từ chối nếu journal/ownership/version/fingerprint đổi sau inspect.
15. UI giữ focus/accessibility/viewport hẹp; progress không spam live-region; action được disable trong in-flight apply và reload không nhân đôi request.
16. Text files trong `config/`, `logs/`, repair journal/backups metadata và frontend persisted state không chứa plaintext DB/admin/machine credential.

## Validation dự kiến

Theo preference hiện tại của dự án, automated validation của Phase 6.3 giữ gọn nhưng phải đánh trúng safety boundary:

```powershell
. .\scripts\use-local-rust.ps1
cargo fmt --manifest-path .\src-tauri\Cargo.toml --check
cargo test --manifest-path .\src-tauri\Cargo.toml --locked repair
npm run lint:ui
npm run build:ui
git diff --check
```

Ngoài focused unit/contract tests, cần ít nhất một disposable-store Windows smoke chạy store có real business sentinel qua các case: managed file repair, managed CoffeePOS plugin repair, machine-token repair failure/rollback, explicit admin-password reset và blocked partial WordPress. Không cần chạy full exhaustive provisioning matrix cho mọi chỉnh sửa nhỏ; user sẽ làm manual UI smoke sâu hơn sau implementation.

### Bằng chứng implementation hiện tại — 2026-09-19

- `get_repair_plan` giữ read-only: không start/stop runtime, không mutate DB/files/plugin để chỉ phục vụ inspect.
- `apply_repair` chạy DB-authority preflight **trước** `begin_repair()` và trước mọi file/plugin/config mutation. Runtime đang chạy dùng managed MariaDB port hiện tại và `SELECT 1` cho cả runtime user + WordPress user; runtime stopped chỉ start tạm MariaDB trong apply, xác minh hai account rồi luôn graceful shutdown hoặc terminate/reap trước khi tiếp tục/trả lỗi.
- DB secret missing/unreadable bị chặn ngay ở plan. DB credential mismatch/unverifiable khi apply trả `database_credentials = blocked`, không tạo repair journal và không chạy repair mutation. Việc này giữ `get_repair_plan` thuần read-only; với runtime stopped, native không thể chứng minh server đang chấp nhận plaintext DPAPI chỉ bằng filesystem inspection.
- Repair journal có `planned → runtime_stopped → staged → swapped → verified → committed`; crash/relaunch recovery rollback/commit theo owned evidence và plugin original-missing sentinel.
- Focused `cargo test ... repair`: 8/8 pass; `cargo fmt --check`, `npm run lint:ui`, `npm run build:ui`, `git diff --check` pass.
- Chưa ghi nhận manual disposable-store Windows acceptance cho repair UI/real store; không dùng automated checks để thay thế bước này.

## Thứ tự triển khai đề xuất

1. **6.3A — Repair inspection/plan:** native phân loại repairable/requires_input/blocked, chưa mutate.
2. **6.3B — Managed file repair:** router/uploads bridge/WordPress core + config policy, transaction/stale-plan guard.
3. **6.3C — Managed plugin repair:** WooCommerce/CoffeePOS same-version stage/swap/verify/rollback.
4. **6.3D — Credential repair:** machine-token rotation/pending recovery + independent-authority missing/mismatch recovery khi đủ điều kiện, explicit admin reset; DB credential loss/mismatch được phân loại blocked theo authority hiện tại.
5. **6.3E — UI + recovery acceptance:** Hệ thống → Sửa chữa, crash/relaunch boundaries, final health/provisioning verification.

Các nhãn A–E là implementation slices bên trong Phase 6.3, không thay numbering chính trong `ROADMAP.md`.

## Ngoài phạm vi

- Log viewer, redaction/export support bundle: Phase 6.4.
- Backup/restore dữ liệu store: Phase 7.
- Import/adopt một WordPress/WooCommerce site không có CoffeePOS ownership evidence.
- WordPress/WooCommerce/CoffeePOS version upgrade/downgrade hoặc database schema rollback.
- LAN repair/firewall/network-device remediation: Phase 8.x.
- Runtime bundle/installer/update repair trên máy phát hành: Phase 9.x.

## Definition of Done

Phase 6.3 chỉ được đánh dấu hoàn thành khi app có flow **Hệ thống → Sửa chữa** dùng native repair plan thật; managed file + same-version managed plugin repair chạy end-to-end trên Windows; machine-token active-valid rotation/pending recovery chạy đúng contract và missing/mismatch recovery chỉ chạy khi independent-authority + protected hash rollback đã được implement/verify, còn thiếu authority thì bị chặn; explicit admin-password reset có pending/recovery transaction; DB credential loss/mismatch bị chặn đúng authority hiện tại; unsafe ownership/partial-install cases bị từ chối; repair giữ database/uploads/business sentinel/secrets; crash/relaunch không để transaction mơ hồ; và final provisioning + health verification chứng minh store trở lại trạng thái hoạt động.
