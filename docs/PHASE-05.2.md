# Phase 5.2 — Thiết lập cửa hàng và tài khoản

Phase 5.2 hoàn thiện onboarding lần đầu trên Windows x64. Flow người vận hành là **Chào mừng → Thông tin cửa hàng/tài khoản → Xác nhận → Cài đặt thật → Hoàn tất**. Provisioning database/WordPress/WooCommerce/CoffeePOS và recovery vẫn dùng đúng contract Phase 4.12; milestone này bổ sung input/persistence/credential contract và UX bao quanh flow đó.

## Contract dữ liệu

Fresh store yêu cầu bốn giá trị:

- tên cửa hàng: 1–80 ký tự, không có control character;
- username quản trị: 3–60 ASCII chữ/số/dấu `.`, `_`, `-`;
- email quản trị: tối đa 100 ký tự; local-part chỉ dùng chữ/số/dấu `.`, `+`, `_`, `-` để giá trị được WordPress `sanitize_email` giữ nguyên;
- password quản trị: 12–128 ký tự, không có control character.

`config/app.json` tiếp tục schema `1` và thêm hai field optional `setup_admin_username` / `setup_admin_email`. Tên cửa hàng, username và email là metadata setup/recovery không nhạy cảm. Các file Phase 4.x không có hai field này vẫn deserialize bình thường. Fresh store name được canonicalize sau trim và từ chối HTML bracket, repeated ASCII space và `%XX` byte escape để giá trị đã accept không bị `sanitize_text_field` của CoffeePOS đổi ngầm.

Password không được ghi vào JSON, URL, localStorage, sessionStorage hoặc log. Native lưu password trong `config/wordpress-admin.secret` bằng Windows DPAPI trước khi provisioning mutation bắt đầu. Password replace dùng `config/wordpress-admin.pending.secret` làm staged credential: config commit xong mới promote sang active; pending còn tồn tại luôn chặn provisioning. Frontend chỉ gửi plaintext trong IPC `save_setup_profile` tại thao tác submit; `SetupInfo` không bao giờ trả password về WebView.

## Vòng đời credential

Fresh store dùng password do người dùng đặt. Người dùng có thể quay lại sửa store/account trước khi provisioning bắt đầu. `save_setup_profile` chỉ được native chấp nhận khi `ProvisioningState::NotInstalled`; lifecycle mutex ngăn save/profile mutation chạy đồng thời với provision/start/stop/restart.

Khi provisioning đã bắt đầu, identity bị khóa. Retry/relaunch dùng lại metadata trong app config và cùng DPAPI secret. `ProvisioningJournal.admin_username` là guard bổ sung: nếu retry cố thay username sau journal commit, native từ chối.

Store Phase 4.11/4.12 cũ không có onboarding metadata tiếp tục dùng `coffeepos_admin`, `admin@coffeepos.local` và protected generated secret đã tồn tại. Store `ready` bỏ qua wizard; không reset user/password để ép store cũ đi qua onboarding mới.

Sau setup, WordPress user và CoffeePOS Settings/health là nguồn sự thật. App config chỉ là setup/recovery metadata. Action **Sao chép mật khẩu quản trị ban đầu** giải mã protected secret trực tiếp trong native layer và ghi vào Windows clipboard; plaintext không đi qua JavaScript. Nếu password sau này được đổi trong WordPress, protected initial secret không tự trở thành password mới.

## Store name source of truth

Fresh provisioning truyền tên cửa hàng vào `wp_install()` và verify `blogname` theo canonical value của chính WordPress (`sanitize_option('blogname', ...)`), vì WordPress lưu HTML-special characters như `&`/`"` ở dạng escaped. CoffeePOS vẫn giữ tên nghiệp vụ nguyên bản: khi activation baseline chạy lần đầu, native đặt current user là initial administrator và gọi plugin-owned `CoffeePOS\Infrastructure\Settings\Settings::update(Settings::OPTION_STORE_NAME, ...)`, sau đó verify `Settings::getStoreName()`.

Việc seed chỉ chạy trong activation baseline. Retry sau `CoffeePosActivated` không ghi đè `coffeepos_store_name`, vì người dùng có thể đã đổi tên nghiệp vụ bằng CoffeePOS. Machine-health `store.name` phải phản ánh value từ plugin Settings.

## Wireframe

### Chào mừng

```text
CoffeePOS

Thiết lập cửa hàng CoffeePOS
Cài một lần trên máy này.

[ Thiết lập cửa hàng ]
```

### Thông tin cửa hàng và tài khoản

```text
Bước 1 / 2

Tên cửa hàng             [________________]
Tên đăng nhập quản trị   [________________]
Email quản trị           [________________]
Mật khẩu                 [••••••••••••••••]
Nhập lại mật khẩu        [••••••••••••••••]

[ Quay lại ]                         [ Tiếp tục ]
```

Validation field-level đưa focus tới field lỗi. Password chỉ ở memory của page trong lúc người dùng đang nhập; sau save thành công hai ô password được clear.

### Xác nhận

```text
Bước 2 / 2

Cửa hàng       My Coffee
Tài khoản      owner
Email          owner@example.com
Mật khẩu       Đã lưu bảo mật

[ Quay lại sửa ]                    [ Cài đặt ]
```

Quay lại sửa trước mutation không provision/start runtime. Double click/Enter không được tạo hai provisioning operation; frontend busy guard và native lifecycle/provisioning mutex cùng bảo vệ.

### Progress / recovery

Progress dùng `ProvisioningInfo` thật. Không giả phần trăm. Interruption/relaunch dùng journal Phase 4.12: retryable hiện **Tiếp tục thiết lập**; blocker non-retryable chỉ **Kiểm tra lại trạng thái**, không thêm Repair giả.

### Hoàn tất

```text
Cửa hàng đã được cài đặt
Tài khoản quản trị: owner

[ Sao chép mật khẩu quản trị ban đầu ]
[ Vào Trang chính ]
```

Không tự mở POS ở Phase 5.2; open/login product flow thuộc Phase 5.4. Acceptance 5.2 chỉ dùng login thật để chứng minh credential tạo ra sử dụng được.

## Native commands

- `get_setup_info`: trả store/account metadata, `password_configured`, `editable`; không trả secret.
- `save_setup_profile`: validate/persist metadata và optional user-set password vào DPAPI, chỉ khi store còn `NotInstalled`.
- `provision_wordpress`: với fresh store yêu cầu setup profile + protected password đã có; configure Provisioner với identity đã khóa rồi chạy Phase 4.12 orchestration.
- `copy_admin_password`: state-scoped command cho main shell, decrypt protected initial password và đưa thẳng vào Windows clipboard.

Legacy `save_store_name` IPC bị bỏ khỏi command manifest/capability để không có đường thay store identity sau khi setup đã bắt đầu.

## Implementation

- `AppConfig` giữ schema `1` và thêm optional `setup_admin_username` / `setup_admin_email`; validator native chốt cùng giới hạn với form.
- `save_setup_profile` chạy dưới lifecycle mutex, chỉ khi inspection là `not_installed`; password optional chỉ khi protected secret đã tồn tại từ một lần save trước. Password replacement dùng pending-secret commit protocol để failure/crash không ghép metadata mới với secret cũ.
- `Provisioner::configure_initial_admin()` thay hard-coded identity cho fresh flow nhưng giữ default legacy cho store Phase 4.x. Khi journal đã tồn tại, username khác journal bị từ chối.
- WordPress bootstrap phải verify initial administrator login/email/administrator role/password + canonical `blogname` cho tới khi journal đã commit `WordPressInstalled`, kể cả retry sau khi `wp_install()` đã tạo tables nhưng native bị gián đoạn trước journal commit. Sau journal đó, rerun không ép password/email cũ để không phá thay đổi hợp lệ của store đã cài.
- CoffeePOS activation baseline seed tên cửa hàng qua `Settings::update()` dưới initial administrator và verify read-back. Retry sau baseline không seed lại.
- `copy_admin_password` dùng Windows clipboard API trong native layer. IPC chỉ trả `()` success/error; frontend không nhận secret.
- Debug build hỗ trợ `COFFEEPOS_DESKTOP_DATA_ROOT=<absolute-path>` để nghiệm thu Tauri bằng disposable store. Override không tồn tại trong release behavior; production vẫn resolve OS app-local-data.
- Frontend có năm state/panel: welcome, details, review, progress/recovery, complete. Password field được clear sau save thành công; installed store vẫn dùng shell Phase 5.1.

## Acceptance

1. Fresh app hiển thị Welcome; installed ready store bỏ qua wizard.
2. Store/account input valid được persist, password chỉ nằm trong protected storage; invalid input không tạo provisioning side effect.
3. Back/edit trước install giữ draft không nhạy cảm và cho đổi profile; password không đi vào browser storage/URL.
4. Double-submit không tạo hai native lifecycle/provisioning operation.
5. Fresh install tạo đúng WordPress username/email/admin role/password và WordPress `blogname`.
6. CoffeePOS `coffeepos_store_name` + machine-health `store.name` bằng tên onboarding; retry không overwrite setting plugin đã tồn tại sau baseline.
7. Login thật qua CoffeePOS `/pos/` với username/password onboarding thành công và tạo WordPress authenticated cookie/session.
8. Relaunch/interruption tiếp tục bằng cùng identity/DPAPI secret; missing protected secret sau WordPress install vẫn `needs_repair`, không regenerate.
9. Existing Phase 4.x generated credential/password hash không đổi khi chạy trên build Phase 5.2.
10. Password sentinel không xuất hiện trong app config, provisioning journal, runtime/provisioning/plugin logs, site text files, URL hoặc WebView storage.
11. Completion/Settings copy initial password hoạt động qua native Windows clipboard mà không trả secret qua IPC response.
12. Lint/build/unit tests + staged fresh onboarding E2E + Phase 4.11 idempotency + Phase 4.12 recovery/blocker regressions pass.

## Kết quả nghiệm thu Windows-first — 2026-09-18

- Static/unit gates trên snapshot cuối: build UI, lint, Rust unit tests và native release build đều pass; Rust unit suite có 50 passed, 0 failed, 5 ignored.
- Fresh staged E2E Phase 5.2: pass 1/1, test body 130.34s, wall 131.44s. Case dùng tên Coffee & "Co" Café 5.2, verify WordPress canonical escaped blogname, CoffeePOS raw semantic store name + health, DPAPI hygiene và login /pos/ thật.
- Legacy/idempotency staged E2E: pass 1/1, 185.01s test body / 186.11s wall; existing admin secret/password hash được giữ. Một run trước đó lộ terminate-timeout recovery gap trong Runtime Manager; fix cuối reap child ở Stopping, tính cron vào state và refresh trước restart.
- Recovery journal-boundary staged E2E: pass 1/1, 177.82s / 178.9s. Partial-WordPress blocker staged E2E: pass 1/1, 85.07s / 86.12s.
- Native Tauri chạy trên disposable data root: fresh Welcome; invalid store name focus đúng field; profile đặc biệt Coffee & "Co" Café Tauri 5.2 save được; password fields clear về rỗng, local/session storage rỗng, URL/body không chứa sentinel; quay lại edit metadata với password rỗng giữ protected credential.
- Acceptance phát hiện và sửa một frontend blocker: sau first save nút **Cài đặt** từng giữ trạng thái disabled từ bootstrap. applySetupInfo giờ cập nhật enablement theo password_configured; rerun chứng minh nút enabled, double-click chuyển đúng Progress và native log có đúng một provisioning prepare request.
- Provisioning Tauri thật đạt ready/Complete với owner_tauri52; native clipboard copy được đối chiếu bằng boolean CLIPBOARD_MATCH=True mà không in plaintext. Complete → Home hiển thị đúng semantic store name Coffee & "Co" Café Tauri 5.2 Final.
- SHA-256 của protected admin-secret blob không đổi trước/sau provisioning và sau relaunch. Relaunch cùng disposable root không để PHP/MariaDB child chạy tự động, phù hợp boundary Phase 5.5; installed-store bypass tiếp tục dùng cùng bootstrap contract đã được Phase 5.1 nghiệm thu.

## Ngoài scope

- Đổi store/account sau setup: dùng WordPress/CoffeePOS chức năng tương ứng; product settings UX mở rộng ở 5.3.
- Mở POS từ Desktop và session/open failure UX: Phase 5.4.
- Auto-start runtime: Phase 5.5.
- Repair engine/log export: Phase 6.x.
- Password manager/sync hoặc reset password flow trong Desktop không thuộc Phase 5.2.
