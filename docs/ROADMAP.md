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

Giữ nguyên số phase. Các mốc artifact/install/activation là checkpoint kỹ thuật nhỏ của cùng một flow setup; không yêu cầu người vận hành bấm từng bước. Mỗi mốc phải có cách chạy/kiểm chứng rõ ràng, và Phase 4.12 phải nối toàn bộ flow từ UI.

- `provisioning.ready` nghĩa là phần cài đặt đã hoàn tất, không chứng minh service đang chạy. Runtime readiness và application health là các kết quả riêng; chỉ hiển thị POS sẵn sàng sau health tương ứng.
- Idempotency, retry an toàn, cleanup, lỗi có hướng phục hồi và bảo vệ secrets là yêu cầu ngay tại phase tạo hành vi đó. Phase 4.11–4.12 kiểm chứng tích hợp toàn stack; Phase 6 bổ sung diagnostics/repair UI, không trì hoãn xử lý lỗi cơ bản tới đó.
- Phase 4.1–4.3 phải pass trước WooCommerce. Trước khi pin CoffeePOS ở 4.7, chốt contract health/POS URL/machine authentication với plugin; implementation và nghiệm thu endpoint vẫn ở 4.10.
- Phase 5.2 phải có đăng nhập thực tế; Phase 5.5 mở rộng shutdown đã có từ Phase 2 cho POS đang hoạt động. Phase 5.3 sở hữu điều hướng; 5.4 sở hữu tự khởi động runtime.
- Kiểm chứng PHP self-request/background jobs từ 4.6; concurrency với POS từ 5.2. Chốt quyết định web server trước LAN hoặc release, theo [ARCHITECTURE.md](../ARCHITECTURE.md#php-built-in-server-quyết-định-có-điều-kiện). Không coi benchmark pass là chứng nhận production cho `php -S`.
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
| 4.3+ | ⏳ Chưa bắt đầu | Mốc tiếp theo: Open WordPress test |

## Phase 4 — Setup WordPress, WooCommerce và CoffeePOS

### Phase 4.1 — Provisioning UI

**Mục tiêu:** nối shell UI với `get_provisioning_info` và `provision_wordpress` đã có ở native layer.

**Scope:** trạng thái Not installed / Installing / Ready / Error; nút Install gọi provisioning thật; disable action hợp lý khi operation đang chạy.

**Definition of Done:** mở app development trên store mới → bấm Install → native Phase 3 provisioning chạy thành công → UI báo Ready. Lỗi native phải hiển thị message/recovery có thể hành động.

### Phase 4.2 — WordPress runtime UX

**Mục tiêu:** người dùng thấy rõ lifecycle WordPress sau khi đã provision.

**Scope:** đồng bộ provisioning state với runtime state, hiển thị database/PHP/WordPress readiness, retry sau lỗi.

Phân biệt rõ `đã cài + stopped`, `starting`, `runtime running + WordPress chưa healthy`, và `WordPress healthy`. Probe PHP hiện tại không thay WordPress health; kiểm WordPress sau mỗi start/restart với timeout và lỗi có component. Chặn action xung đột ở native layer, không chỉ disable nút. Refresh/reload UI chỉ đọc trạng thái; không tự reinstall hoặc spawn thêm process. Auto-start khi mở app thuộc 5.4; progress từng bước chỉ bổ sung nếu cần cho UX này.

**Definition of Done:** app restart trên store đã cài nhận đúng trạng thái, không yêu cầu cài lại; start/stop/retry cập nhật UI đúng với native state.

Acceptance phải có stopped → start → healthy → stop → restart, lỗi startup → retry và process chết khi UI đang mở; không hiển thị health cũ sau stop/failure. WordPress health hỏng không được tự động coi là cần cài lại.

### Phase 4.3 — Open WordPress test

**Mục tiêu:** xác nhận site WordPress cục bộ có thể được mở từ app bằng URL runtime thực tế.

**Scope:** dùng `http://127.0.0.1:<dynamic-port>` từ runtime info; không hard-code port.

Action development mở trang WordPress bằng trình duyệt hệ thống, chỉ enabled sau WordPress health. Native chỉ mở URL của runtime đang quản lý, không nhận URL tùy ý từ frontend. Không nạp WordPress vào shell có management IPC; POS WebView riêng thuộc 5.2. Chưa yêu cầu đăng nhập admin ở mốc này.

**Definition of Done:** sau provisioning có action mở WordPress local thành công và reload/restart vẫn dùng đúng port hiện tại.

Diễn tập port cũ bị chiếm: URL mới, redirect và static assets vẫn đúng origin; dừng runtime thì action mở site bị disable.

### Phase 4.4 — WooCommerce artifact

**Mục tiêu:** pin một WooCommerce release cụ thể tương thích với WordPress/PHP/MariaDB đã chọn.

**Scope:** exact version, official source, checksum, license/readme metadata, deterministic staging script và ignored development target.

**Definition of Done:** staging từ clean target tái tạo đúng artifact và checksum; không tải `latest` lúc app chạy.

### Phase 4.5 — WooCommerce provisioning

**Mục tiêu:** đưa WooCommerce đã pin vào site được CoffeePOS Desktop quản lý.

**Scope:** copy/install plugin bằng ensure semantics; không ghi đè plugin/site không thuộc ownership contract.

**Definition of Done:** fresh site có plugin WooCommerce đúng version; chạy provisioning lần hai không phá plugin hoặc dữ liệu hiện có.

### Phase 4.6 — WooCommerce activation

**Mục tiêu:** activate WooCommerce và kiểm dependency/version trước khi tiếp tục.

**Scope:** activation thật trong WordPress, bounded failure, actionable error.

**Definition of Done:** WooCommerce active sau fresh setup và vẫn active sau restart/retry; activation failure không làm hỏng WordPress install.

Kiểm schema/setup bắt buộc và background jobs/loopback cần cho baseline đã chọn; plugin active chưa đủ chứng minh usable. Ghi rõ xử lý onboarding, WP-Cron và tác vụ nền khi app đóng; không yêu cầu người vận hành tự vào wp-admin để hoàn tất prerequisite. Nếu PHP self-request bị kẹt, giải quyết hoặc ghi blocker trước khi tiếp tục dependency đó.

### Phase 4.7 — CoffeePOS artifact

**Mục tiêu:** xác định và pin nguồn CoffeePOS plugin dùng cho desktop distribution.

**Scope:** version/source/layout/hash hoặc reproducible source snapshot; giữ plugin CoffeePOS là sản phẩm WordPress độc lập.

**Definition of Done:** development staging tạo đúng plugin tree/version mà không copy ngẫu nhiên từ một LocalWP install đang chạy.

Artifact được chọn phải khớp contract health đã thống nhất trước mốc này. Nếu 4.10 cần sửa plugin thì phải tạo artifact mới có version/hash mới và chạy lại acceptance 4.8–4.10; không sửa âm thầm plugin bên trong staged artifact cũ.

### Phase 4.8 — CoffeePOS provisioning

**Mục tiêu:** cài CoffeePOS vào site đã có WooCommerce.

**Scope:** ensure plugin files, dependency preflight, không chuyển business logic sang Rust.

**Definition of Done:** fresh site có CoffeePOS đúng version; retry giữ nguyên site/plugin data.

### Phase 4.9 — CoffeePOS activation

**Mục tiêu:** activate CoffeePOS sau khi WooCommerce đã sẵn sàng.

**Definition of Done:** CoffeePOS active, dependency lỗi được báo rõ, restart runtime không làm mất trạng thái activation.

Xác minh migrations, roles/capabilities, settings và route cần thiết bằng cơ chế của plugin. Activation không thay nghiệm thu health/POS, và Desktop không tự tạo lại schema hoặc business defaults của CoffeePOS.

### Phase 4.10 — CoffeePOS health endpoint

**Mục tiêu:** chốt và dùng application-level health contract từ plugin.

**Scope:** endpoint `/wp-json/coffeepos/v1/system/status` hoặc contract cuối cùng tương đương; WordPress/WooCommerce/CoffeePOS/database/store state; machine auth không lộ secret.

Chốt schema/auth/POS route trước artifact 4.7; 4.10 triển khai và nghiệm thu trên artifact đã pin. Phải kiểm token thiếu/sai, timeout, schema không tương thích, dependency lỗi và plugin inactive. Endpoint mất không đủ bằng chứng để kết luận dependency cụ thể; diagnostics giữ trạng thái unknown/unavailable khi không xác minh được.

**Definition of Done:** Desktop phân biệt được healthy, dependency failure và transport/bootstrap failure từ endpoint thật; không tái tạo CoffeePOS domain checks trong Rust.

### Phase 4.11 — Full install idempotency

**Mục tiêu:** chứng minh toàn bộ DB → WordPress → WooCommerce → CoffeePOS có thể chạy lại an toàn.

**Definition of Done:** provisioning lần hai giữ nguyên sentinel và dữ liệu nghiệp vụ test; không reset credential, database, uploads hoặc plugin state.

### Phase 4.12 — First-run recovery

**Mục tiêu:** retry được khi setup thất bại giữa chừng.

**Definition of Done:** inject/diễn tập failure ở các ranh giới DB, WordPress, WooCommerce và CoffeePOS; lần Retry tiếp tục hoặc trả repair error rõ ràng mà không tự xóa store.

Nghiệm thu trực tiếp từ app: fresh store → setup toàn stack → application healthy; đóng/mở giữa setup → đọc journal → tiếp tục an toàn. Một flow UI điều phối các bước, không yêu cầu người dùng chạy staging/CLI hoặc vào wp-admin. Artifact development được chuẩn bị trước; production/offline payload thuộc Phase 9. Lỗi không retryable phải chỉ rõ hướng xử lý, không hiện nút Repair như thể engine 6.2 đã tồn tại.

## Phase 5 — POS trong Desktop

### Phase 5.1 — POS URL

Xác định route POS thật từ CoffeePOS/plugin health contract và kiểm readiness. **Done khi** runtime trả một URL POS hợp lệ và request thật thành công.

Route lấy từ plugin router, resolve trong origin runtime được xác minh; chặn URL ngoài origin. Redirect tới login có thể đúng với người chưa đăng nhập, nhưng không được báo là authenticated POS đã sẵn sàng.

### Phase 5.2 — Desktop POS WebView

Load POS local trong WebView riêng với origin đã xác minh và không cấp management IPC cho WordPress content. **Done khi** POS render và thao tác cơ bản chạy trong app.

Trước implementation, chốt cách người dùng nhận/đặt credential ban đầu và đăng nhập bằng auth WordPress/CoffeePOS hiện có; credential được bảo vệ trong native storage chưa tự tạo thành luồng đăng nhập dùng được. Không dùng health token làm staff session. Acceptance gồm login → POS → tạo đơn test → logout → login lại, session hết hạn/relaunch, external navigation và kiểm POS không gọi được management IPC. Kiểm self-request, jobs và request đồng thời thực tế; lưu số đo/giới hạn của baseline.

### Phase 5.3 — Navigation shell

Xây flow Setup → Starting → Login/POS → Error và đường quay lại shell. **Done khi** điều hướng theo native state đúng, có thể rời POS để xem lỗi/start lại và quay về POS mà không tạo WebView/runtime trùng. Mốc này vẫn cho phép Start thủ công; auto-start thuộc 5.4. Repair engine thuộc 6.2, trước đó chỉ hiện recovery action thực sự hỗ trợ.

### Phase 5.4 — Startup automation

Store đã provision thì app tự detect, start runtime và điều hướng tới trạng thái phù hợp. **Done khi** relaunch trên store có sẵn mở đúng flow mà không reinstall.

### Phase 5.5 — Shutdown lifecycle

Đóng app phải stop PHP và MariaDB sạch. **Done khi** close/crash test không để process runtime mồ côi và dữ liệu store vẫn nhất quán.

Mở rộng lifecycle Phase 2 cho request/checkout đang chạy, multiple windows và pending setup. Chốt bounded drain/timeout khi đóng bình thường; không hứa graceful shutdown khi process bị kill hoặc mất điện. Crash acceptance phải relaunch và kiểm database recovery, trạng thái đơn test và không tạo đơn trùng; trạng thái thanh toán vẫn do plugin/WooCommerce quyết định.

## Phase 6 — Diagnostics và Repair

### Phase 6.1 — Health diagnostics

Hiển thị component health cho Database / PHP / WordPress / WooCommerce / CoffeePOS. **Done khi** failure được phân loại đúng component với recovery action rõ ràng.

### Phase 6.2 — Repair flow

Repair các file/config/plugin managed bị thiếu hoặc hỏng trong phạm vi có thể khôi phục an toàn. **Done khi** repair không xóa dữ liệu store và từ chối tự động sửa trường hợp ownership/compatibility không rõ.

### Phase 6.3 — Log viewer/export

Cho phép xem/export diagnostic logs đã redact secret. **Done khi** một support bundle đủ để chẩn đoán runtime/provisioning failure mà không chứa credential.

## Phase 7 — Backup / Restore

### Phase 7.1 — Backup format

Chốt archive schema, component/schema versions, checksums và compatibility rules. **Done khi** backup có thể validate độc lập trước restore.

### Phase 7.2 — Database backup

Logical dump MariaDB bằng tool từ development artifact đã pin/verify; production bundle được hoàn thiện ở 9.1. **Done khi** dump/restore database test tái tạo dữ liệu mong đợi và chọn được chính sách snapshot nhất quán với uploads/config.

### Phase 7.3 — Uploads và CoffeePOS config backup

Đưa uploads và cấu hình cần thiết vào backup, không bundle runtime/core binaries. **Done khi** backup chứa đủ store data ngoài database.

Nghiệm thu database + uploads/config cùng một mốc dữ liệu: chặn writer/request/jobs trong thời gian snapshot hoặc dùng cơ chế nhất quán đã chứng minh. Backup không được báo thành công nếu chỉ hoàn tất một phần. Chốt encryption/key recovery và cách tái tạo secrets cho profile/máy đích; copy blob DPAPI của máy cũ không đủ cho portable restore.

### Phase 7.4 — Restore

Validate → stop runtime → stage restore → migrations → health → activate. **Done khi** restore E2E pass và restore failure vẫn giữ được trạng thái trước đó.

Trước thay dữ liệu active phải có backup current state đã validate. Target khôi phục phải có đúng artifact tương thích và kiểm archive trước extract. Chạy health staging ở môi trường cách ly; chỉ chuyển active khi đạt. Test restore sang profile Windows khác, failure ở từng bước thay thế và giữ bản gốc đến khi xác nhận thành công; Windows ↔ macOS chỉ được công bố sau acceptance trên cả hai target.

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

Không bắt đầu WooCommerce trước khi Phase 4.1–4.3 pass. Phase 4.2 đã hoàn thành Windows-first; mốc tiếp theo là **Phase 4.3 — Open WordPress test** để mở đúng dynamic runtime URL chỉ sau khi WordPress health đạt.
