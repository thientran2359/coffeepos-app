# CoffeePOS Desktop — Roadmap

Roadmap này là nguồn sự thật cho cách chia phase của CoffeePOS Desktop. Mỗi subphase phải tạo ra một thay đổi nhỏ có thể kiểm chứng độc lập; không gom nhiều mục tiêu lớn vào một phase rồi chỉ đánh dấu hoàn thành vì code compile.

## Quy tắc hoàn thành một phase

Một phase/subphase chỉ được đánh dấu **hoàn thành** khi đáp ứng đủ các điều kiện áp dụng cho scope đó:

1. Implementation của scope đã xong và không dựa vào fake-ready hoặc dependency ngoài hợp đồng runtime.
2. Test/lint/build phù hợp với thay đổi đều pass.
3. Có một flow thực tế chứng minh hành vi chính trên target đang hỗ trợ.
4. Retry/failure/cleanup được kiểm tra khi phase có lifecycle hoặc provisioning.
5. Tài liệu ghi rõ phần đã làm, phần chưa thuộc scope và bằng chứng validation.

Windows x64 là target được triển khai và nghiệm thu trước. macOS được bổ sung theo cùng contract ở các mốc sau; trạng thái Windows-first không đồng nghĩa đã chứng nhận cross-platform.

## Ranh giới và dependency gates

Giữ nguyên số và bằng chứng của Phase 1–4.12 đã hoàn thành. Ngày 2026-09-18 điều chỉnh Phase 5 thành các milestone UI/UX dưới đây; số 5.x cũ không còn dùng để lên kế hoạch. Phase 5.1–5.2 đã hoàn thành Windows-first với native Tauri/WebView acceptance và staged runtime regressions. Phase 5.3–5.6 đã triển khai và pass lightweight automated checks; manual Windows acceptance còn chờ người dùng smoke-test. Đặc tả trải nghiệm nằm tại [UI-UX.md](UI-UX.md). Các mốc artifact/install/activation là checkpoint kỹ thuật của cùng một setup flow; không yêu cầu người vận hành bấm từng bước.

- `provisioning.ready` nghĩa là phần cài đặt đã hoàn tất, không chứng minh service đang chạy. Runtime readiness và application health là các kết quả riêng; chỉ hiển thị POS sẵn sàng sau health tương ứng.
- Idempotency, retry an toàn, cleanup, lỗi có hướng phục hồi và bảo vệ secrets là yêu cầu ngay tại phase tạo hành vi đó. Phase 4.11–4.12 kiểm chứng tích hợp toàn stack; Phase 6 bổ sung diagnostics, runtime performance và repair/log tooling, không trì hoãn xử lý lỗi cơ bản tới đó.
- Phase 4.1–4.3 phải pass trước WooCommerce. Trước khi pin CoffeePOS ở 4.7, chốt contract health/POS URL/machine authentication với plugin; implementation và nghiệm thu endpoint vẫn ở 4.10.
- UI/UX là deliverable của từng phase: màn hình, hành động chính, loading/error/retry và nghiệm thu thao tác thật. 5.1 sở hữu khung điều hướng; 5.2 setup/tài khoản; 5.3 trang chính/cài đặt; 5.4 mở POS/login; 5.5 auto-start; 5.6 shutdown. Không trì hoãn toàn bộ UI đến khi backend hoàn tất.
- Kiểm chứng PHP self-request/background jobs từ 4.6; concurrency với POS từ 5.4. Requirement thay thế `php -S` đã được ghi nhận và Phase 6.2 sở hữu quyết định/implementation serving stack concurrent trước LAN hoặc release; xem [ARCHITECTURE.md](../ARCHITECTURE.md#php-built-in-server-requirement-thay-thế-đã-được-chứng-minh). Không coi benchmark pass là chứng nhận production cho `php -S`.
- Chốt thiết kế bảo mật LAN trước bind 8.1; 8.3 là nghiệm thu/hardening toàn flow. Phase 8.1–8.2 chỉ thử nghiệm có kiểm soát cho đến khi 8.3 pass.
- Phase 7 dùng development artifact đã pin cho dump/restore; không phụ thuộc production bundling 9.1. Phase 9 chịu trách nhiệm bundle chính các công cụ đã nghiệm thu.
- Development và acceptance dùng store/profile riêng trước khi thử dữ liệu vận hành. Không kiểm thử failure/restore/upgrade trên store thật.

## Trạng thái hiện tại

| Phase | Trạng thái | Slice đã chứng minh |
| --- | --- | --- |
| 1 — Desktop Shell | ✅ Hoàn thành Windows-first | Tauri shell, config, app-local-data, native build/run |
| 2 — Runtime Manager | ✅ Hoàn thành Windows-first | PHP + MariaDB start/stop/restart/readiness/cleanup |
| 3 — WordPress Provisioning | ✅ Hoàn thành Windows-first | Fresh WordPress install, retry/idempotency, runtime E2E |
| 4.1 — Provisioning UI | ✅ Hoàn thành Windows-first | Fresh app → Install → Installing → Ready → restart vẫn Ready |
| 4.2 — WordPress runtime UX | ✅ Hoàn thành Windows-first | Runtime lifecycle + WordPress health + retry/process-death handling |
| 4.3 — Open WordPress test | ✅ Hoàn thành Windows-first | Managed dynamic URL, occupied-port fallback, redirect/static assets và system-browser open đã pass |
| 4.4 — WooCommerce artifact | ✅ Hoàn thành Windows-first | WooCommerce 11.1.0 exact archive + SHA256 + compatibility metadata + deterministic staging |
| 4.5 — WooCommerce provisioning | ✅ Hoàn thành Windows-first | Ownership-safe atomic install, journal stage, retry preserves plugin/data, unmanaged conflict refusal |
| 4.6 — WooCommerce activation | ✅ Hoàn thành Windows-first | Active 11.1.0, DB schema 11.1.0-1, setup/onboarding verified, managed CLI cron/background worker |
| 4.7 — CoffeePOS artifact | ✅ Hoàn thành Windows-first | CoffeePOS 1.0.0 immutable checked-in ZIP + SHA256 + source provenance + deterministic staging |
| 4.8 — CoffeePOS provisioning | ✅ Hoàn thành Windows-first | Ownership-safe install 1.0.0, dependency preflight, journal stage, retry/data preservation |
| 4.9 — CoffeePOS activation | ✅ Hoàn thành Windows-first | Exact 1.0.0 active, Woo 11.1.0 preflight, fresh-process plugin baseline verification, restart/retry persistence |
| 4.10 — CoffeePOS health endpoint | ✅ Hoàn thành Windows-first | CoffeePOS 1.0.1 machine-health schema 1, DPAPI token auth, native classification, rotation/recovery, real E2E |
| 4.11 — Full install idempotency | ✅ Hoàn thành Windows-first | Second full DB → WordPress → WooCommerce → CoffeePOS provisioning preserves credentials, uploads, business data, plugin ownership/state and machine token |
| 4.12 — First-run recovery | ✅ Hoàn thành Windows-first | Journal-boundary interruption recovery, credential preservation, persistent partial-WordPress repair blocker, real staged E2E |
| 5.1 — UI shell and navigation | ✅ Hoàn thành Windows-first | Setup tách khỏi installed shell; Home/Settings/Diagnostics; native mouse/keyboard/resize; navigation/reload không respawn runtime; recovery 4.12 regressions pass |
| 5.2 — Store and account onboarding | ✅ Hoàn thành Windows-first | Fresh wizard + user-set admin credential trong DPAPI; WordPress/CoffeePOS store identity; real POS login; clipboard copy; retry/relaunch + legacy credential preservation |
| 5.3 — Home and app settings | 🟡 Đã triển khai, chờ manual acceptance | Home Start/Retry theo native state; stale health không giữ qua lifecycle; Desktop startup-view setting persist atomically; lightweight checks pass |
| 5.4 — Open POS and login | 🟡 Đã triển khai, chờ manual acceptance | Home Mở bán hàng khi machine health healthy; system browser dùng verified dynamic origin + plugin pos_path; auth/session giữ ở WordPress/CoffeePOS; lightweight checks pass |
| 5.5 — Daily startup | 🟡 Đã triển khai, chờ manual acceptance | Installed ready store auto-start khi runtime stopped; reload running không start lại; setup/recovery không auto-start; không tự mở POS; lightweight checks pass |
| 5.6 — Minimize, exit and shutdown | 🟡 Đã triển khai, chờ manual acceptance | Minimize giữ runtime; Close/Alt+F4 native confirm; bounded PHP request drain + MariaDB shutdown; Windows Job Object crash containment; lightweight checks pass |
| 6.1 — Health diagnostics | 🟡 Đã triển khai, chờ manual acceptance | Live authenticated Database/PHP probes; WordPress readiness + CoffeePOS machine-health mapping cho đủ 5 component; recovery action rõ ràng; lightweight checks pass |
| 6.2 — Runtime performance and responsiveness | 🟡 Đã triển khai, chờ manual acceptance | Caddy 2.11.4 + PHP FastCGI 4 workers + OPcache; async lifecycle/status-health split; dynamic p50 134 ms, 4 parallel 144 ms vs 537 ms sequential; staged lifecycle/concurrency smoke pass |
| 6.3 — Repair flow | 🟡 Đã triển khai, chờ manual acceptance | Hệ thống → Sửa chữa; read-only plan + stale guard; managed file/core/plugin atomic repair + crash journal; admin reset/pending machine-token recovery; DB mismatch preflight blocked trước mutation; lightweight checks pass; xem [PHASE-06.3](PHASE-06.3.md) |
| 6.4 — Log viewer/export | ✅ Hoàn thành Windows-first | Hệ thống → Nhật ký; native 10-source allowlist + bounded paging; viewer/export fail-closed redaction; support bundle schema 1 + Windows Save As; 11 focused Rust tests + UI checks pass; xem [PHASE-06.4](PHASE-06.4.md) |
| 7.1 — Backup format | ✅ Hoàn thành Windows-first | Encrypted portable container + strict manifest/inventory/checksum/path/compatibility validation + native inspect/validate; focused tests pass; xem [PHASE-07.1](PHASE-07.1.md) |
| 7.2 — Database backup | ✅ Hoàn thành Windows-first | Managed mariadb-dump + maintenance snapshot + disposable import/fingerprint verification + crash-safe child cleanup; focused tests pass; xem [PHASE-07.2](PHASE-07.2.md) |
| 7.3 — Uploads/config backup | ✅ Hoàn thành Windows-first | Complete encrypted backup + native Save As/progress/cancel + same-snapshot uploads/config/admin secret + crash recovery/final validation; real disposable-store smoke pass; xem [PHASE-07.3](PHASE-07.3.md) |
| 7.4 — Restore | 🟡 Đã triển khai, chờ manual acceptance | Inspect + recovery snapshot + isolated staging + target secrets + transaction-owned cutover/rollback/recovery; focused restore checks pass; xem [PHASE-07.4](PHASE-07.4.md) |
| 7.5 — Desktop localization | 🟡 Đã triển khai, chờ manual acceptance | Tiếng Việt/English; first-run language chooser + persisted Settings hot-switch + full Desktop-shell i18n + native tray/close locale + stable user-facing error codes; UI/Rust checks pass; xem [PHASE-07.5](PHASE-07.5.md) |

## Phase 4 — Setup WordPress, WooCommerce và CoffeePOS

### Phase 4.1 — Provisioning UI

**Mục tiêu:** nối shell UI với `get_provisioning_info` và `provision_wordpress` đã có ở native layer.

**Scope:** trạng thái Not installed / Installing / Ready / Error; nút Install gọi provisioning thật; disable action hợp lý khi operation đang chạy.

**Definition of Done:** mở app development trên store mới → bấm Install → native Phase 3 provisioning chạy thành công → UI báo Ready. Lỗi native phải hiển thị message/recovery có thể hành động.

### Phase 4.2 — WordPress runtime UX

**Mục tiêu:** người dùng thấy rõ lifecycle WordPress sau khi đã provision.

**Scope:** đồng bộ provisioning state với runtime state, hiển thị database/PHP/WordPress readiness, retry sau lỗi.

Phân biệt rõ `đã cài + stopped`, `starting`, `runtime running + WordPress chưa healthy`, và `WordPress healthy`. Probe PHP hiện tại không thay WordPress health; kiểm WordPress sau mỗi start/restart với timeout và lỗi có component. Chặn action xung đột ở native layer, không chỉ disable nút. Refresh/reload UI chỉ đọc trạng thái; không tự reinstall hoặc spawn thêm process. Auto-start khi mở app thuộc 5.5; progress từng bước chỉ bổ sung nếu cần cho UX này.

**Definition of Done:** app restart trên store đã cài nhận đúng trạng thái, không yêu cầu cài lại; start/stop/retry cập nhật UI đúng với native state.

Acceptance phải có stopped → start → healthy → stop → restart, lỗi startup → retry và process chết khi UI đang mở; không hiển thị health cũ sau stop/failure. WordPress health hỏng không được tự động coi là cần cài lại.

### Phase 4.3 — Open WordPress test

**Mục tiêu:** xác nhận site WordPress cục bộ có thể được mở từ app bằng URL runtime thực tế.

**Scope:** dùng `http://127.0.0.1:<dynamic-port>` từ runtime info; không hard-code port.

Action development mở trang WordPress bằng trình duyệt hệ thống, chỉ enabled sau WordPress health. Native chỉ mở URL của runtime đang quản lý, không nhận URL tùy ý từ frontend. Không nạp WordPress vào shell có management IPC; POS host được chốt riêng ở 5.4. Chưa yêu cầu đăng nhập admin ở mốc này.

**Definition of Done:** sau provisioning có action mở WordPress local thành công và reload/restart vẫn dùng đúng port hiện tại.

Diễn tập port cũ bị chiếm: URL mới, redirect và static assets vẫn đúng origin; dừng runtime thì action mở site bị disable.

**Hoàn thành Windows-first 2026-09-18:** `RuntimeManager::wordpress_url()` chỉ trả managed loopback URL khi runtime `running` và WordPress `healthy`; `open_wordpress` không nhận URL từ frontend và mở bằng system browser trên Windows; UI chỉ enable action khi provisioning ready + runtime running + health healthy. Acceptance đã pass cho occupied old port → new port, redirect/static assets trên origin mới, system-browser open bằng app thật và stop → action disabled. Xem `docs/PHASE-04.3.md`.

### Phase 4.4 — WooCommerce artifact

**Mục tiêu:** pin một WooCommerce release cụ thể tương thích với WordPress/PHP/MariaDB đã chọn.

**Scope:** exact version, official source, checksum, license/readme metadata, deterministic staging script và ignored development target.

**Definition of Done:** staging từ clean target tái tạo đúng artifact và checksum; không tải `latest` lúc app chạy.

**Hoàn thành Windows-first 2026-09-18:** pin WooCommerce `11.1.0` từ exact WordPress.org archive, SHA256 `6bae9bf74d722b6deb15f049687c311cfafc26e3a5d8fa55ac6ea4b9a3a8df19`, license/readme/plugin metadata và compatibility baseline. `scripts/stage-woocommerce-development.ps1` stage vào ignored development target và verify checksum + version/requirements trước/sau extract. Repeated staging tạo cùng tree 5,862 files với cùng fingerprint. Xem `docs/PHASE-04.4.md`.

### Phase 4.5 — WooCommerce provisioning

**Mục tiêu:** đưa WooCommerce đã pin vào site được CoffeePOS Desktop quản lý.

**Scope:** copy/install plugin bằng ensure semantics; không ghi đè plugin/site không thuộc ownership contract.

**Definition of Done:** fresh site có plugin WooCommerce đúng version; chạy provisioning lần hai không phá plugin hoặc dữ liệu hiện có.

**Hoàn thành Windows-first 2026-09-18:** native provisioning consume exact staged WooCommerce 11.1.0 artifact từ Phase 4.4, copy qua owned `woocommerce.provisioning` rồi atomic rename vào `wp-content/plugins/woocommerce`, ghi ownership metadata và journal `woocommerce_provisioned`. Existing unmanaged WooCommerce bị preserve/refuse; retry cùng managed version là no-op. Real E2E pass fresh install + second provisioning, giữ sentinel trong WooCommerce và unrelated plugin; existing development store migrate `needs_repair` → `ready`. Xem `docs/PHASE-04.5.md`.

### Phase 4.6 — WooCommerce activation

**Mục tiêu:** activate WooCommerce và kiểm dependency/version trước khi tiếp tục.

**Scope:** activation thật trong WordPress, bounded failure, actionable error.

**Definition of Done:** WooCommerce active sau fresh setup và vẫn active sau restart/retry; activation failure không làm hỏng WordPress install.

Kiểm schema/setup bắt buộc và background jobs/loopback cần cho baseline đã chọn; plugin active chưa đủ chứng minh usable. Ghi rõ xử lý onboarding, WP-Cron và tác vụ nền khi app đóng; không yêu cầu người vận hành tự vào wp-admin để hoàn tất prerequisite. Nếu PHP self-request bị kẹt, giải quyết hoặc ghi blocker trước khi tiếp tục dependency đó.

**Hoàn thành Windows-first 2026-09-18:** exact managed WooCommerce 11.1.0 được activate qua bounded pinned-PHP CLI bootstrap; baseline verify internal DB schema `11.1.0-1`, required tables/pages/role, Action Scheduler và onboarding không cần operator. Managed `wp-config.php` disable web-triggered WP-Cron; Runtime Manager chạy `wp-cron.php` bằng managed PHP CLI ngay khi healthy và định kỳ khi app/runtime chạy, tránh nested self-request của PHP built-in server. Activation failure giữ journal ở `woo_commerce_provisioned`; retry development store đã recover thành `woo_commerce_activated`. Real staged E2E pass activation/restart/retry + Store API HTTP 200. Xem `docs/PHASE-04.6.md`.

### Phase 4.7 — CoffeePOS artifact

**Mục tiêu:** xác định và pin nguồn CoffeePOS plugin dùng cho desktop distribution.

**Scope:** version/source/layout/hash hoặc reproducible source snapshot; giữ plugin CoffeePOS là sản phẩm WordPress độc lập.

**Definition of Done:** development staging tạo đúng plugin tree/version mà không copy ngẫu nhiên từ một LocalWP install đang chạy.

Schema/auth/version/POS-route của machine-health phải được chốt trước khi pin artifact 4.7. Artifact 4.7 chưa bắt buộc implement endpoint mới vì implementation/nghiệm thu thuộc 4.10; nếu 4.10 sửa CoffeePOS code thì phải bump plugin version + artifact hash, repin manifest và chạy lại acceptance 4.8–4.10. Không sửa âm thầm plugin bên trong staged artifact cũ.

**Hoàn thành Windows-first 2026-09-18:** pin CoffeePOS `1.0.0` release snapshot build từ clean Git commit `9473867c65f1409dbeeaa03a98daef564f620b8e`, checked-in ZIP SHA256 `ee9f241a516e7c6ddddc6e84ad26515d0a9cd9ccd5d6fd101d078a738e598f2a`. Development staging verify archive/path safety, plugin metadata/dependency/vendor autoload và repeated tree fingerprint. Machine-health contract schema/token/version/POS path được chốt trong `docs/PROVISIONING.md`; implementation endpoint vẫn thuộc 4.10 và phải repin artifact nếu code plugin thay đổi. Xem `docs/PHASE-04.7.md`.

### Phase 4.8 — CoffeePOS provisioning

**Mục tiêu:** cài CoffeePOS vào site đã có WooCommerce.

**Scope:** ensure plugin files, dependency preflight, không chuyển business logic sang Rust.

**Definition of Done:** fresh site có CoffeePOS đúng version; retry giữ nguyên site/plugin data.

**Hoàn thành Windows-first 2026-09-18:** native provisioning consume exact staged CoffeePOS 1.0.0, validate target/layout/header/dependency + full WordPress/PHP/MariaDB/WooCommerce compatibility baseline, ensure vào wp-content/plugins/coffeepos qua owned coffeepos.provisioning + atomic rename và .coffeepos-managed.json. Existing unmanaged/mismatched/corrupt destination được preserve/refuse; exact managed retry là no-op. Journal append coffee_pos_provisioned + optional coffeepos_version, nên Phase 4.6 store deserialize an toàn và chuyển needs_repair cho tới retry. E2E xác nhận Woo active nhưng CoffeePOS chưa active, fresh/retry giữ WordPress/Woo/CoffeePOS/unrelated sentinels. Acceptance cũng phát hiện Woo-active WordPress first request có thể vượt 10s; runtime tách WordPress readiness 45s khỏi PHP HTTP readiness 10s. Xem docs/PHASE-04.8.md.

### Phase 4.9 — CoffeePOS activation

**Mục tiêu:** activate CoffeePOS sau khi WooCommerce đã sẵn sàng.

**Definition of Done:** CoffeePOS active, dependency lỗi được báo rõ, restart runtime không làm mất trạng thái activation.

Xác minh migrations, roles/capabilities, settings và route cần thiết bằng cơ chế của plugin. Activation không thay nghiệm thu health/POS, và Desktop không tự tạo lại schema hoặc business defaults của CoffeePOS.

**Hoàn thành Windows-first 2026-09-18:** existing provisioning orchestration activate exact managed CoffeePOS 1.0.0 qua bounded pinned-PHP CLI sau explicit exact WooCommerce 11.1.0 active/header/runtime preflight. Activation và verification tách thành hai PHP process: process đầu chỉ chạy plugin activation/lifecycle; process thứ hai load WordPress mới hoàn toàn rồi verify active version/autoload, plugin-owned Migrator schema + physical tables, Settings options, roles/capabilities, rewrite version/rules và actual REST registry (bao gồm toàn bộ route mà RouteRegistrar::registeredRoutes() công bố). Chỉ sau fresh-process verifier pass mới persist journal coffee_pos_activated và báo Ready/coffeepos_active=true. Failure giữ journal ở coffee_pos_provisioned để retry replay plugin lifecycle mà không reset site/database/plugin data. Real staged E2E pass fresh activation, REST permission contract, runtime restart/process-death/port recovery và second provisioning. Xem docs/PHASE-04.9.md.

### Phase 4.10 — CoffeePOS health endpoint

**Mục tiêu:** chốt và dùng application-level health contract từ plugin.

**Scope:** endpoint `/wp-json/coffeepos/v1/system/status` hoặc contract cuối cùng tương đương; WordPress/WooCommerce/CoffeePOS/database/store state; machine auth không lộ secret.

Chốt schema/auth/POS route trước artifact 4.7; 4.10 triển khai và nghiệm thu trên artifact đã pin. Phải kiểm token thiếu/sai, timeout, schema không tương thích, dependency lỗi và plugin inactive. Endpoint mất không đủ bằng chứng để kết luận dependency cụ thể; diagnostics giữ trạng thái unknown/unavailable khi không xác minh được.

**Definition of Done:** Desktop phân biệt được healthy, dependency failure và transport/bootstrap failure từ endpoint thật; không tái tạo CoffeePOS domain checks trong Rust.

**Hoàn thành Windows-first 2026-09-18:** CoffeePOS `1.0.1` implement `/wp-json/coffeepos/v1/system/status` schema 1 với `X-CoffeePOS-Machine-Token`, chỉ lưu SHA-256 server-side và trả 200 healthy / 503 degraded / 401 auth theo state plugin-owned. Desktop tạo token 32 random bytes lowercase hex trong Windows DPAPI, bootstrap qua bounded pinned-PHP stdin, probe endpoint thật sau WordPress health, validate strict response/POS path và phân loại authentication/contract/transport-bootstrap riêng. Managed 1.0.0 được upgrade atomically lên exact 1.0.1 artifact; token không reset khi retry/restart. Rotation dùng protected active+pending credential, verify pending trước promote và rollback old hash khi update/probe fail. Acceptance bao phủ missing/wrong/correct token, incompatible schema parser, dependency degraded, plugin inactive/missing route, runtime restart/process death/port move, credential rotation và second provisioning. Xem `docs/PHASE-04.10.md`.

### Phase 4.11 — Full install idempotency

**Mục tiêu:** chứng minh toàn bộ DB → WordPress → WooCommerce → CoffeePOS có thể chạy lại an toàn.

**Definition of Done:** provisioning lần hai giữ nguyên sentinel và dữ liệu nghiệp vụ test; không reset credential, database, uploads hoặc plugin state.

**Hoàn thành Windows-first 2026-09-18:** full staged E2E chạy lại toàn bộ `prepare → runtime start → WordPress/WooCommerce/CoffeePOS install/activation → machine health` trên cùng completed store và giữ nguyên DB runtime/WordPress/admin/machine credentials, WordPress admin password hash, managed wp-config/salts, external uploads, real CoffeePOS suspended-cart row + store option, plugin ownership metadata, activation state, journal và unrelated files. Native readiness cũng yêu cầu protected DB/admin credentials đọc được; installed WordPress không tự sinh replacement admin secret khi credential bị mất mà chuyển sang `needs_repair`. Xem `docs/PHASE-04.11.md`.

### Phase 4.12 — First-run recovery

**Mục tiêu:** retry được khi setup thất bại giữa chừng.

**Definition of Done:** inject/diễn tập failure ở các ranh giới DB, WordPress, WooCommerce và CoffeePOS; lần Retry tiếp tục hoặc trả repair error rõ ràng mà không tự xóa store.

Nghiệm thu trực tiếp từ app: fresh store → setup toàn stack → application healthy; đóng/mở giữa setup → đọc journal → tiếp tục an toàn. Một flow UI điều phối các bước, không yêu cầu người dùng chạy staging/CLI hoặc vào wp-admin. Artifact development được chuẩn bị trước; production/offline payload thuộc Phase 9. Lỗi không retryable phải chỉ rõ hướng xử lý, không hiện nút Repair như thể engine 6.3 đã tồn tại.

**Hoàn thành Windows-first 2026-09-18:** native provisioning có deterministic test-only interruption checkpoint sau từng side effect và trước journal commit. Real staged E2E recreate `Provisioner` + `RuntimeManager` trên cùng `data_root` qua các boundary database/site/WordPress/WooCommerce/CoffeePOS/machine health rồi Retry tới `ready`, giữ nguyên protected DB/admin/machine credentials. Interruption giữa `wp_install()` được bảo vệ riêng: bootstrap exit 6/7 persist optional `recovery_blocker=partial_wordpress_install`; relaunch trả `needs_repair`, `can_retry=false`, giữ nguyên tables/site và normal provisioning không bypass blocker. UI dùng cùng một Install/Retry flow và khi non-retryable chỉ cho kiểm tra lại trạng thái. Xem `docs/PHASE-04.12.md`.

## Phase 5 — Trải nghiệm ứng dụng và mở bán hàng

Phase 5.1–5.2 đã hoàn thành Windows-first. Phase 5.3–5.6 và Phase 6.1–6.4 đã triển khai code + focused/lightweight validation, còn manual Windows acceptance theo tài liệu từng phase. Sau snapshot 6.1, product IA được chốt lại: installed-store top-level navigation là **Tổng quan / Cấu hình / Hệ thống**; **Chẩn đoán**, **Sửa chữa** và **Nhật ký** là chức năng bên trong Hệ thống. Đây là target cho mọi UI change tiếp theo; acceptance lịch sử của 5.1 vẫn ghi đúng shell đã chạy lúc đó. Trước code mỗi milestone, bổ sung spec phase với wireframe success/loading/error, contract native cần dùng và acceptance theo [UI-UX.md](UI-UX.md). Tái sử dụng runtime/provisioning hiện có; không viết lại backend chỉ để đổi giao diện.

### Phase 5.1 — Khung giao diện và điều hướng

**Scope:** tách setup khỏi khu vực cửa hàng đã cài; navigation Trang chính / Cài đặt / Chẩn đoán; thống nhất layout, typography, form/button/error states. Chuyển technical controls đang có vào Chẩn đoán; giữ flow 4.12 hoạt động.

**Done khi:** điều hướng app thật bằng chuột/bàn phím, reload/relaunch chọn đúng màn hình theo installation; runtime không bị spawn lại do đổi trang. Resize/DPI không che action; không có trang chức năng tương lai rỗng. Native success/error hiện có vẫn hiển thị và thao tác được. Chưa cần auto-start, POS host hoặc repair engine.

**Hoàn thành Windows-first 2026-09-18:** shell frontend tách setup/recovery khỏi installed-store navigation, chuyển controls kỹ thuật sang Chẩn đoán và giữ `store_name` ở setup để không phá Phase 4.12. Tauri acceptance chứng minh ready+stopped → Home; navigation/reload giữ nguyên long-lived runtime PID và không tăng `runtime start requested`; Diagnostics start/restart/stop vẫn đạt health đúng; native Windows mouse + keyboard Enter/Space đổi view và focus đúng heading; cửa sổ native resize tới WebView `460×560` vẫn cuộn/không overflow. Host chỉ có monitor 100%, nên 150% được kiểm bằng WebView2 `deviceScaleFactor=1.5` trong app Tauri thật thay vì đổi setting hệ thống. Ba staged provisioning/recovery regressions đều pass. Xem [PHASE-05.1.md](PHASE-05.1.md).

**IA revision sau Phase 6.1 — đã áp dụng frontend 2026-09-18:** giữ invariant navigation không respawn runtime, focus/accessibility và responsive acceptance của 5.1, nhưng đổi product labels/hierarchy thành **Tổng quan / Cấu hình / Hệ thống**. Internal view/config keys `home/settings/diagnostics` được giữ tương thích; `diagnostics` dẫn vào Hệ thống tại mục Chẩn đoán. Header + top-level navigation giữ vị trí ổn định; content phía dưới cuộn độc lập và dùng chung max-width/padding/spacing. Segmented control cũ đã được thay bằng navigation ngang với active underline; Cấu hình dùng grid desktop và Hệ thống gom health/runtime theo hierarchy mới. UI typecheck/build và DOM-reference checks pass; manual Windows visual smoke-test vẫn thuộc acceptance của người dùng.

### Phase 5.2 — Thiết lập cửa hàng và tài khoản

**Scope:** Chào mừng → thông tin cửa hàng/tài khoản → tiến trình → hoàn tất, reuse recovery 4.12. Chốt input/persistence/credential contract trước code; WordPress/plugin giữ nguồn sự thật cho store/account. Cách đặt/nhận credential phải dùng được với cả fresh store và store cũ có generated secret, không reset tài khoản khi retry.

**Done khi:** fresh setup từ form thật tạo đúng store/account; validation, quay lại sửa, double-submit và interruption/relaunch được nghiệm thu; tài khoản đăng nhập được qua auth hiện có của WordPress/CoffeePOS trên trình duyệt test. Store đã cài bỏ qua fresh wizard, dữ liệu cũ được giữ; secret không lọt vào logs/URL/draft storage. Tích hợp nút mở bán hàng thuộc 5.4.

**Hoàn thành Windows-first 2026-09-18:** fresh Tauri wizard chấp nhận tên Unicode/HTML-special hợp lệ, validation focus đúng field, save/edit clear password khỏi form và không dùng URL/localStorage/sessionStorage. Native dùng pending-secret transaction + Windows DPAPI, khóa profile khi provisioning bắt đầu và copy initial password thẳng vào Windows clipboard mà IPC không trả plaintext. Real staged E2E tạo custom administrator, verify WordPress canonical blogname, CoffeePOS raw semantic store name/health và đăng nhập POS thật; journal-boundary recovery, partial-WordPress blocker và legacy full-idempotency đều pass. Native Tauri acceptance chứng minh double-submit chỉ tạo một provisioning prepare request, Complete → Home hiển thị đúng store, clipboard khớp credential test, DPAPI blob không đổi qua provision/relaunch và relaunch không auto-start runtime. Runtime stop/restart cũng được harden để reap child sau terminate timeout và không bỏ sót cron. Xem [PHASE-05.2.md](PHASE-05.2.md).

### Phase 5.3 — Trang chính và cài đặt ứng dụng

**Scope:** tên cửa hàng, trạng thái dễ hiểu, hành động theo native state; cài đặt thuộc Desktop với save/error feedback. Tách thông tin kỹ thuật khỏi trang chính. Khi chưa có opener 5.4, chỉ hiển thị hệ thống sẵn sàng và chức năng thực sự có.

**Done khi:** nghiệm thu installed/stopped/starting/healthy/error/stopping; health stale bị loại sau failure; Start/Retry có kết quả thật và không reinstall. Người dùng tìm được **Cấu hình** và **Hệ thống → Chẩn đoán** mà không phải hiểu PHP/database. Không nhân bản dashboard hoặc settings nghiệp vụ POS.

**Đã triển khai 2026-09-18, chờ manual acceptance:** Home có primary action theo native runtime/health state, transition busy chặn submit trùng và health Retry chỉ recheck WordPress/CoffeePOS mà không gọi provisioning. Settings thêm preference Desktop-only `startup_view` (`home`/`settings`/`diagnostics`) với atomic config persistence và default tương thích config schema 1 cũ. `git diff --check`, UI typecheck/build, Rust fmt check và 9 focused config tests đều pass; staged E2E/native interaction dài được bỏ theo yêu cầu kiểm thử gọn và để người dùng smoke-test. Xem [PHASE-05.3.md](PHASE-05.3.md).

### Phase 5.4 — Mở POS và đăng nhập

**Scope:** nút **Mở bán hàng**, URL từ plugin router/health và native origin đã kiểm chứng, login/session bằng auth hiện có. Chốt host trong spec trước code: browser là hướng đề xuất MVP; POS WebView là lựa chọn riêng, không phải prerequisite của shell và không bắt buộc triển khai cả hai.

**Done khi:** app → Mở bán hàng → login → POS → đơn test → logout/login lại; session hết hạn, runtime restart/đổi port và open failure có đường phục hồi. Redirect login không được coi là authenticated POS. Browser không cung cấp tín hiệu tab/login cho shell; WebView nếu chọn phải cách ly management IPC và kiểm external navigation. Kiểm jobs/self-request/concurrency bằng POS thật. Không thêm nghiệp vụ bán hàng vào Desktop.

**Đã triển khai 2026-09-18, chờ manual acceptance:** MVP chốt system browser. Home chỉ hiện **Mở bán hàng** khi runtime đang running, WordPress healthy và CoffeePOS machine-health healthy. Native `open_pos` không nhận URL từ frontend, refresh process state dưới lifecycle guard, lấy `pos_path` đã validate từ machine-health và ghép với loopback origin/dynamic port hiện tại; browser-open failure trả lỗi actionable về Home và không gọi provisioning. Frontend chặn double-submit, không cache POS URL và chỉ báo “Đã yêu cầu mở trình duyệt”, không suy đoán login/tab state. UI lint/build, Rust fmt và focused URL tests đều pass; login → order → logout/login, session expiry, restart/port move và browser interaction thật để người dùng manual smoke-test. Xem [PHASE-05.4.md](PHASE-05.4.md).

### Phase 5.5 — Khởi động hằng ngày

**Scope:** mở app trên store đã cài tự start runtime, báo tiến trình/lỗi trên trang chính; healthy thì cho Mở bán hàng. Không tự bật app cùng OS trong milestone này.

**Done khi:** relaunch đi đúng luồng không reinstall, không sinh process hoặc tab trùng do polling/reload; failure có Retry; setup dở được dẫn về recovery. Không làm mất session/data bằng startup automation.

**Đã triển khai 2026-09-18, chờ manual acceptance:** bootstrap chỉ auto-start sau khi provisioning native xác nhận ready và get_runtime_info trả stopped; runtime đã running hoặc đang transition chỉ render state hiện tại. Auto-start tái sử dụng start_runtime + lifecycle mutex hiện có, polling vẫn chỉ refresh và không gọi open_pos, nên reload/navigation không tự sinh tab POS. Startup failure rơi về Home state Phase 5.3 với **Thử lại** gọi start thật; not_installed/needs_repair giữ setup/recovery. Bootstrap có busy guard để retry không tạo hai startup request. Lượt này cũng đóng gap Phase 5.4 bằng cách thêm open_pos vào Tauri app manifest/capability; focused native compile đã generate permission allow-open-pos. UI lint/build, Rust fmt, git diff --check và focused runtime test đều pass. Manual smoke-test còn cần xác nhận PID/log start-count qua relaunch/reload, startup failure/retry và không auto-open browser. Xem [PHASE-05.5.md](PHASE-05.5.md).

### Phase 5.6 — Thu nhỏ, thoát và shutdown

**Scope:** Minimize ẩn shell xuống system tray và giữ runtime. Close/Alt+F4 mở shutdown confirmation như trước; tray mở lại shell hoặc thực hiện **Thoát hoàn toàn**. Close/Alt+F4 và tray exit dùng chung bounded shutdown contract. Đóng tab trình duyệt không dừng server.

**Done khi:** close/cancel/minimize/relaunch và crash không để process mồ côi; bounded drain/timeout được kiểm với request/đơn test đang chạy; kiểm recovery không tạo đơn trùng. Không hứa graceful shutdown khi mất điện/kill; không coi đóng app là chốt ca hoặc tự thay payment state.

**Đã triển khai 2026-09-19, chờ manual acceptance:** Tauri tạo tray icon từ icon ứng dụng; Minimize phát hiện trạng thái minimized rồi hide cửa sổ và reset minimized state trong lúc ẩn, nên runtime/POS tiếp tục chạy nền. Close/Alt+F4 prevent-close rồi gọi cùng `request_full_exit` với tray **Thoát hoàn toàn**: lifecycle mutex + confirmation hiện có → bounded drain → graceful MariaDB SHUTDOWN + forced-cleanup fallback; app chỉ exit khi không còn managed child, còn cancel/busy/failure giữ hoặc mở lại shell. Tray có **Mở CoffeePOS** và double-click trái để restore. Windows kill-on-close Job Object tiếp tục bảo đảm child không mồ côi khi desktop crash/kill, nhưng không được mô tả là graceful. Cargo check/diff/fmt pass; native Win32 smoke xác nhận Minimize hide-to-tray giữ process alive. Close/Alt+F4 dialog, tray menu và in-flight request để user manual smoke-test trước khi chốt acceptance. Xem [PHASE-05.6.md](PHASE-05.6.md).

**Gate cuối Phase 5:** người thử hoàn tất setup → login/bán hàng → đóng/mở → dùng store cũ → xử lý lỗi thông thường mà không dùng terminal/wp-admin. Ghi rõ người thử, mức hỗ trợ, bằng chứng và giới hạn; developer acceptance không tự thay user usability test.

## Phase 6 — Hệ thống: Diagnostics, hiệu năng và Repair

Phase 6 mở rộng phần runtime/hệ thống sau khi POS thật đã hoạt động. Chẩn đoán, tối ưu runtime, Repair và Log viewer/export cùng giữ ranh giới kỹ thuật của Desktop; không đưa business logic POS sang native layer và không thêm công cụ kỹ thuật thành các tab ngang cấp với **Tổng quan** và **Cấu hình**.

### Phase 6.1 — Health diagnostics

Hiển thị component health cho Database / PHP / WordPress / WooCommerce / CoffeePOS. **Done khi** failure được phân loại đúng component với recovery action rõ ràng.

**Đã triển khai 2026-09-18, chờ manual acceptance:** snapshot ban đầu dùng top-level Chẩn đoán và có native diagnostics được serialize bằng lifecycle lock, kiểm trực tiếp MariaDB bằng authenticated `SELECT 1`, PHP bằng nonce HTTP probe, WordPress bằng readiness response, sau đó dùng authenticated CoffeePOS machine-health schema 1 để phân loại Database/WordPress/WooCommerce/CoffeePOS ở application layer. Payload `degraded` chỉ gán lỗi cho boolean component `false`; machine-health auth/transport/contract fail không suy đoán WooCommerce hỏng khi chưa có payload. UI hiển thị 5 component, recovery action, **Kiểm tra lại**, và gom port/version/path/error kỹ thuật vào phần chi tiết. Product IA mới giữ nguyên contract này nhưng đặt màn hình tại **Hệ thống → Chẩn đoán**. Full diagnostics chỉ chạy khi mở Chẩn đoán hoặc người dùng yêu cầu; Phase 6.2 sẽ tách fast process status khỏi background application-health scheduling để poll shell không tranh request với POS. Repair và log export lùi thành 6.3/6.4. Lightweight TypeScript/Rust checks pass; native degraded/failure interaction để manual smoke-test. Xem [PHASE-06.1.md](PHASE-06.1.md).

### Phase 6.2 — Hiệu năng runtime và độ mượt

Đưa lifecycle/readiness/health I/O khỏi Tauri UI thread; tách fast runtime status khỏi application-health probe; bật OPcache; thay serving path POS single-process `php -S` bằng local web-serving/FastCGI worker model có concurrency và được Desktop pin/quản lý. Giữ loopback/dynamic port, machine-health, provisioning, Phase 5.6 admission/drain và Windows Job Object semantics. **Done khi** app vẫn phản hồi trong start/restart, status poll không tự tạo WordPress request, OPcache active, benchmark cùng máy cải thiện rõ ràng và 4 request động concurrent không còn serialize gần tuyến tính. Xem [PHASE-06.2.md](PHASE-06.2.md).

**Đã triển khai 2026-09-19, chờ manual acceptance:** runtime Windows development pin Caddy 2.11.4 và dùng một PHP 8.4.25 NTS `php-cgi` master với 4 FastCGI workers; Caddy serve static/uploads, proxy PHP trên private loopback và graceful-drain qua admin loopback động. OPcache được bật và readiness HTTP kiểm trực tiếp web SAPI. `get_runtime_info` trở thành fast process/cached-health snapshot; cron + application health chuyển sang maintenance riêng; lifecycle/diagnostics/close shutdown blocking work chạy ngoài Tauri UI thread. Disposable-store lifecycle smoke pass, không orphan. Các final warm benchmark đạt p50 138–144 ms/p95 151–314 ms; 4 WordPress request tuần tự 562–738 ms vs 4 song song 145–189 ms; probe PHP cố định 250 ms/request đạt 1.03–1.07 s tuần tự vs 258–269 ms song song; static p50 0–1 ms/p95 1 ms. Automated checks pass; resize/navigation/repaint trong lúc auto-start/restart để người dùng manual smoke-test. Xem [PHASE-06.2.md](PHASE-06.2.md).

### Phase 6.3 — Repair flow

Trong **Hệ thống**, repair các file/config/plugin managed bị thiếu hoặc hỏng trong phạm vi có thể khôi phục an toàn. Flow inspect trước, lập repair plan có `repairable / requires_input / blocked`, rồi mới apply dưới lifecycle guard. Same-version managed file/plugin repair dùng pinned artifact + staging/atomic swap/verify; credential repair chỉ chạy khi có authority/transaction rõ; partial WordPress hoặc ownership/compatibility không đủ phải preserve và từ chối tự động sửa. **Done khi** repair không xóa dữ liệu store, giữ uploads/business data/secrets, recover được sau interruption và final provisioning + health verify pass. Xem [PHASE-06.3.md](PHASE-06.3.md).

### Phase 6.4 — Log viewer/export

Trong **Hệ thống → Nhật ký**, cho phép xem diagnostic logs theo native allowlist + bounded paging và export support bundle schema-versioned. Viewer chỉ nhận text đã redact; export chạy redaction lần nữa, normalize path, giới hạn kích thước và chỉ lấy safe diagnostic projections + allowlisted logs. Không copy database/uploads/site/config/protected secrets và không start/stop runtime chỉ để export. **Done khi** fixture secret canary không xuất hiện trong viewer/bundle, support bundle đủ evidence cho runtime/provisioning/repair failure phổ biến và manual Windows smoke pass cho running/stopped/cancel/error. Xem [PHASE-06.4.md](PHASE-06.4.md).

**Hoàn thành Windows-first 2026-09-19:** native có catalog/read/export commands với 10 log source cố định, canonical containment, paging 200 dòng + byte/scan bounds, stale/mid-line cursor rejection và loại trailing line đang ghi dở. Viewer/export đều exact-redact protected DB/admin/machine/pending secrets và fail closed nếu secret file tồn tại nhưng không giải mã được; export normalize data-root/USERPROFILE case-insensitive trên Windows. Support bundle schema 1 chỉ lấy safe runtime/health/provisioning/repair projections + allowlisted log tails, giới hạn 2 MiB/source và 20 MiB uncompressed, staged temp + atomic finalize, native Save As và duplicate-export guard. UI **Hệ thống → Nhật ký** có source selector, empty/error states, refresh/load-older/export gating. Rust logs tests 11/11, cargo check/fmt và UI lint/build pass. User đã xác nhận Phase 6.4 hoàn thành trước khi chuyển sang lập đặc tả Phase 7.

## Phase 7 — Backup / Restore và Desktop localization

### Phase 7.1 — Backup format

Chốt encrypted archive schema, component/schema versions, checksums, path rules, portable-secret policy và compatibility rules. **Done khi** backup có thể decrypt/inspect/validate độc lập trước restore và corrupt/incompatible archive fail closed. Xem [PHASE-07.1.md](PHASE-07.1.md).

### Phase 7.2 — Database backup

Logical dump MariaDB bằng exact tool từ development artifact đã pin/verify; production bundle được hoàn thiện ở 9.1. Runtime vào maintenance, giữ Caddy/PHP/cron stopped và dùng database-only process cho dump. **Done khi** dump/import trên disposable database tái tạo dữ liệu mong đợi, credential không lộ và failure/cancel không để orphan. Xem [PHASE-07.2.md](PHASE-07.2.md).

### Phase 7.3 — Uploads và CoffeePOS config backup

Đưa uploads, portable store config và administrator secret vào encrypted backup, không bundle runtime/core binaries/raw DPAPI config. **Done khi** backup hoàn chỉnh giữ database + uploads/config trong cùng maintenance snapshot, final archive validate pass và runtime quay về previous state. Xem [PHASE-07.3.md](PHASE-07.3.md).

Nghiệm thu database + uploads/config cùng một mốc dữ liệu: chặn writer/request/jobs trong thời gian snapshot hoặc dùng cơ chế nhất quán đã chứng minh. Backup không được báo thành công nếu chỉ hoàn tất một phần. Chốt encryption/key recovery và cách tái tạo secrets cho profile/máy đích; copy blob DPAPI của máy cũ không đủ cho portable restore.

### Phase 7.4 — Restore

Inspect/validate → pre-restore recovery backup → isolated staging → target-local secrets → migrations → staging health → atomic cutover → active health. **Done khi** restore E2E sang Windows profile khác pass và failure/crash vẫn giữ hoặc rollback được trạng thái trước đó. Xem [PHASE-07.4.md](PHASE-07.4.md).

Trước thay dữ liệu active phải có backup current state đã validate. Target khôi phục phải có đúng artifact tương thích và kiểm archive trước extract. Chạy health staging ở môi trường cách ly; chỉ chuyển active khi đạt. Test restore sang profile Windows khác, failure ở từng bước thay thế và giữ bản gốc đến khi xác nhận thành công; Windows ↔ macOS chỉ được công bố sau acceptance trên cả hai target.

### Phase 7.5 — Desktop localization

Thêm hai locale Desktop `vi` / `en`. Fresh profile chọn language trước Welcome/setup; installed store đổi trong **Cấu hình** và hot-switch UI không restart runtime. Locale được persist như preference của target profile, không copy qua portable backup/restore và không tự đổi WordPress/POS locale. Sweep toàn bộ user-facing Desktop shell hiện có sang translation keys, gồm onboarding, Tổng quan/Cấu hình/Hệ thống, diagnostics/repair/logs, backup/restore, tray/close và actionable error copy. **Done khi** fresh + installed Windows flows cho cả hai locale pass, legacy config/recovery vẫn mở an toàn và target locale được giữ qua cross-profile restore. Xem [PHASE-07.5.md](PHASE-07.5.md).

## Phase 8 — LAN Mode

Entry gate: chốt canonical URL/redirect, auth/session, transport bảo vệ credential, firewall, health-token scope và rollback về loopback trước 8.1. Kiểm web server theo gate kiến trúc, bao gồm POS + KDS/customer display/request dài. `0.0.0.0` là bind address, không phải URL hiển thị. Khả năng sync của các thiết bị phải kiểm qua transport thực tế của plugin; không suy luận sync nhiều máy từ việc mở được trang.

### Phase 8.1 — LAN bind

Opt-in bind HTTP trên interface LAN phù hợp; database vẫn loopback-only. **Done khi** thiết bị khác trong LAN truy cập POS được sau khi bật LAN mode.

### Phase 8.2 — LAN address UI

Hiển thị reachable local URL/address để người dùng không phải tự tìm IP. **Done khi** URL được cập nhật đúng khi network thay đổi trong các case hỗ trợ.

### Phase 8.3 — LAN security

Chốt auth/access control, firewall guidance, cookie/origin behavior và không expose management endpoints. **Done khi** LAN acceptance test chứng minh POS reachable nhưng DB/native control không reachable từ LAN.

Nghiệm thu login/session, đổi địa chỉ mạng, quay về local-only, health endpoint không lộ thông tin khi thiếu auth và sync liên thiết bị theo phạm vi plugin hỗ trợ. Không bật LAN cho vận hành trước khi toàn bộ 8.1–8.3 pass.

## Phase 9 — Distribution / Packaging

### Phase 9.1 — Runtime bundle

Đưa PHP, MariaDB, WordPress baseline và plugin artifacts vào app resources với manifest/version/hash. **Done khi** release build không phụ thuộc `runtime/development`.

Bao gồm tools backup/restore đã nghiệm thu, dependent DLLs và license notices. Tách profile development/test khỏi dữ liệu production; chốt identifier/data path và verify integrity của payload. Không lấy runtime/tool từ PATH để bù file thiếu.

### Phase 9.2 — Windows installer

Tạo installer Windows gồm runtime/prerequisites cần thiết. **Done khi** cài/uninstall trên máy test sạch hoạt động và store data nằm ngoài installation directory.

### Phase 9.3 — Fresh-machine test

Acceptance trên Windows sạch không Node, Rust, PHP, MariaDB hoặc LocalWP. **Done khi** install → first run → setup → POS chạy hoàn chỉnh.

Kiểm offline install/first run với prerequisite payload theo cam kết sản phẩm; login và đơn test thật; restart/backup/restore. Ghi kết quả trên VM/máy sạch, không dùng smoke test trên máy developer thay cho fresh-machine evidence. Chính sách ký release và integrity phải rõ trước phát hành bên ngoài.

### Phase 9.4 — Upgrade safety

Desktop/runtime update phải giữ `site/`, `database/`, `uploads/`, backups và schema tương thích. **Done khi** upgrade E2E giữ store data và rollback/recovery path đã được kiểm chứng cho failure được hỗ trợ.

Phân biệt desktop/runtime update với plugin/schema update. Bản plugin/core do Desktop quản lý cần policy cập nhật rõ ràng, tránh updater trong WordPress làm trôi baseline đã pin. Chốt backup trước migration, version compatibility và recovery khi schema không thể downgrade; không rollback riêng executable rồi giả định database cũ vẫn tương thích.

## Phạm vi release và macOS

Phase 4–5 tạo development preview local; chưa phải bản vận hành cho cửa hàng thật. Release Windows local-only cần Phase 6–7, các gate runtime/security và 9.1–9.4. LAN là khả năng opt-in riêng: có thể để tắt trong release local-only, nhưng không được bỏ gate 8.1–8.3 nếu phân phối tính năng LAN. Đây là dependency release, không phải chỉ dẫn bỏ qua thứ tự triển khai hiện tại.

macOS vẫn chưa có acceptance. Sau baseline Windows, lập kế hoạch target riêng trước implementation: artifact arm64/x64, secrets Keychain, process cleanup/parent crash, WKWebView login/POS, backup/restore và signed/notarized package trên máy sạch. Mỗi target phải chạy lại các gate liên quan; không tự đánh dấu complete từ Windows hoặc CI compile. Auto-updater, service discovery và cloud sync chưa được cam kết trong roadmap này.

## Thứ tự thực hiện ngay tiếp theo

Phase 4.1–4.12 và **Phase 5.1–5.2** đã pass Windows-first. **Phase 5.3–5.6** và **Phase 6.1–6.3** đã triển khai theo trạng thái ghi ở bảng trên; **Phase 6.4** đã được user xác nhận hoàn thành Windows-first. **Phase 7.1–7.3** đã hoàn thành Windows-first; **Phase 7.4–7.5** đã triển khai và đang chờ manual acceptance theo trạng thái ở bảng trên.
