# Phase 7.5 — Desktop localization (Tiếng Việt / English)

Phase 7.5 thêm localization cho **CoffeePOS Desktop shell** với hai locale đầu tiên: `vi` và `en`. Người dùng chọn ngôn ngữ trước khi bắt đầu fresh onboarding, toàn bộ Desktop UI sau đó dùng locale đã chọn, và preference có thể đổi lại trong **Cấu hình** mà không cần restart app.

> **Trạng thái:** đặc tả đã chốt cho implementation Phase 7.5. Windows-first. Scope này chỉ sở hữu ngôn ngữ của CoffeePOS Desktop; WordPress Admin, WooCommerce và CoffeePOS POS web app tiếp tục dùng locale riêng của chúng.

## Mục tiêu

1. Fresh profile cho người dùng chọn **Tiếng Việt** hoặc **English** trước Welcome/setup.
2. Lựa chọn được persist trong Desktop app config và được dùng lại ở lần mở app sau.
3. Installed store có thể đổi ngôn ngữ trong **Cấu hình** và UI đổi ngay, không restart runtime/app.
4. Toàn bộ user-facing Desktop shell text hiện có dùng translation keys thay vì hard-code Việt/Anh rải rác.
5. Navigation, onboarding, runtime states, diagnostics, repair, logs, backup/restore, dialogs, progress và actionable errors giữ cùng semantic ở cả hai locale.
6. Locale là preference của profile đích; portable backup/restore không ép ngôn ngữ từ máy nguồn sang máy đích.
7. Missing/invalid locale không làm app crash hoặc làm mất khả năng recovery.

## Locale được hỗ trợ

Phase 7.5 chỉ chốt hai mã ổn định:

~~~text
vi  → Tiếng Việt
en  → English
~~~

Tên ngôn ngữ trong picker luôn hiển thị theo chính ngôn ngữ đó:

~~~text
Tiếng Việt
English
~~~

Không persist label hiển thị như `"Tiếng Việt"` hoặc `"Vietnamese"`; config và IPC chỉ dùng mã `vi` / `en`.

Mapping format locale:

~~~text
vi → vi-VN
en → en-US
~~~

Mapping này dùng cho `Intl.DateTimeFormat`, `Intl.NumberFormat` và formatter tương tự trong Desktop shell. Nó không thay WordPress site locale.

## Ownership và ranh giới

### Desktop app language

Phase 7.5 sở hữu:

- shell navigation;
- onboarding/welcome;
- setup forms/review/progress/result;
- Tổng quan;
- Cấu hình;
- Hệ thống;
- Chẩn đoán;
- Sửa chữa;
- Nhật ký;
- Sao lưu và khôi phục;
- user-facing modal/dialog copy do Desktop sở hữu;
- system tray labels do Desktop sở hữu;
- accessible labels/ARIA text của Desktop;
- validation/recovery copy mà Desktop hiển thị.

### Không gộp với WordPress/POS locale

`app_language` không phải:

- WordPress `WPLANG`;
- locale của wp-admin;
- locale của WooCommerce;
- locale của CoffeePOS POS/KDS/customer display;
- nội dung do plugin/site tự render.

Ví dụ hợp lệ:

~~~text
CoffeePOS Desktop: English
WordPress Admin:   Tiếng Việt
CoffeePOS POS:     Tiếng Việt
~~~

Phase sau có thể thêm site/POS locale riêng. Phase 7.5 không tự sửa WordPress option chỉ vì user đổi Desktop language.

## Startup và effective locale

Desktop cần phân biệt **locale đã được user chọn** với **fallback locale**.

Conceptual startup:

~~~text
App launch
   ↓
Load config
   ↓
Resolve app_language
   ├── Some(vi/en) → dùng locale đã lưu
   └── None
        ├── brand-new profile → hiện language chooser
        └── existing/partial legacy profile → effective locale = vi
   ↓
Render Desktop UI
~~~

Không dùng OS display language để âm thầm đổi behavior ở Phase 7.5. Default/fallback ổn định là `vi`; user quyết định đổi sang English.

### Brand-new profile

Hiện language chooser khi đồng thời:

- `app_language` chưa được chọn;
- provisioning là fresh/NotInstalled;
- chưa có setup profile đã commit;
- không có provisioning/restore/recovery transaction đang cần tiếp tục.

### Existing hoặc interrupted legacy profile

Config cũ không có `app_language` vẫn hợp lệ. Nếu store đã installed hoặc đang có setup/recovery state từ phiên bản trước:

- không chèn language chooser vào giữa recovery;
- effective locale = `vi`;
- user có thể đổi trong **Cấu hình** sau khi vào shell;
- nếu flow recovery có màn hình trước shell, dùng fallback `vi`.

Điều này tránh việc update app làm một store đang recovery bị chặn bởi một preference mới.

## Fresh onboarding UX

Language chooser đứng trước Welcome và trước mọi input store/admin.

~~~text
CoffeePOS

Chọn ngôn ngữ
Choose your language

[ Tiếng Việt ]
[ English    ]
~~~

Đây là màn hình duy nhất cố ý dùng song ngữ trong cùng một view. Sau khi user chọn, UI chuyển hoàn toàn sang locale đó.

Flow:

~~~text
Language
   ↓
Welcome
   ↓
Store + administrator information
   ↓
Review
   ↓
Install
   ↓
Progress
   ↓
Complete
~~~

Ví dụ `vi`:

~~~text
Chào mừng đến CoffeePOS

Thiết lập cửa hàng cục bộ trên máy này.

[ Thiết lập cửa hàng mới ]
[ Khôi phục từ bản sao lưu ]
~~~

Ví dụ `en`:

~~~text
Welcome to CoffeePOS

Set up a local store on this computer.

[ Set up a new store ]
[ Restore from backup ]
~~~

Phase 7.4 fresh restore cũng đi sau language chooser, nên restore UI dùng đúng locale trước khi target store tồn tại.

## Cấu hình → Ngôn ngữ

Installed shell giữ IA:

~~~text
Tổng quan / Cấu hình / Hệ thống
Overview / Settings / System
~~~

Trong **Cấu hình**, thêm preference:

~~~text
Cấu hình

Chung

Ngôn ngữ
[ Tiếng Việt ▼ ]
~~~

English:

~~~text
Settings

General

Language
[ English ▼ ]
~~~

Options trong select luôn là:

~~~text
Tiếng Việt
English
~~~

### Hot-switch behavior

Khi user chọn locale khác:

1. validate mã locale;
2. persist native config atomically;
3. cập nhật locale state phía frontend;
4. re-render mọi text thuộc Desktop shell;
5. cập nhật `<html lang>`, title/ARIA labels và native Desktop-owned labels;
6. giữ nguyên route, form state không nhạy cảm và operation hiện tại.

Không reload WebView chỉ để đổi ngôn ngữ. Không restart Caddy/PHP/MariaDB. Không thay provisioning state.

Nếu persist lỗi:

- preference cũ vẫn là authority;
- UI quay lại locale đã persist;
- hiển thị lỗi lưu theo locale hiện tại;
- không ghi config dở dang.

## Native config contract

Current `config/app.json` dùng schema 1 và đã có pattern additive optional preferences. Phase 7.5 thêm optional field:

~~~rust
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppLanguage {
    Vi,
    En,
}

#[serde(default, skip_serializing_if = "Option::is_none")]
pub app_language: Option<AppLanguage>,
~~~

Sau khi user chọn:

~~~json
{
  "schema_version": 1,
  "store_name": "My Coffee",
  "bind_host": "127.0.0.1",
  "startup_view": "home",
  "app_language": "vi"
}
~~~

Config cũ không có field:

~~~json
{
  "schema_version": 1,
  "store_name": "Legacy Coffee",
  "bind_host": "127.0.0.1"
}
~~~

vẫn deserialize được với `app_language=None`.

Phase 7.5 không bump schema chỉ cho additive Desktop preference này, theo contract hiện tại của `startup_view` và setup metadata. Unknown/invalid locale value phải fail config validation rõ ràng; không tự đoán thành một locale khác.

## Native command contract

Conceptual:

~~~text
get_shell_info()
  -> config.app_language: "vi" | "en" | null

save_app_language(app_language)
  -> ShellInfo
~~~

`save_app_language`:

- chỉ nhận enum/mã allowlisted `vi|en`;
- persist cùng atomic config path hiện có;
- serialize với store/config lock hiện có;
- không start/stop runtime;
- không acquire backup/restore maintenance;
- không mutate store/admin identity;
- không ghi locale value vào log nếu log không cần dữ liệu đó;
- trả `ShellInfo` mới sau khi persist thành công.

Settings có thể gọi command này trực tiếp khi select đổi. Không buộc onboarding phải gửi `startup_view` chỉ để lưu language.

Frontend không tự ghi `config/app.json`.

## Frontend i18n structure

Không rải conditional:

~~~ts
language === "vi" ? "Sao lưu dữ liệu" : "Back up data"
~~~

trong từng handler/component.

Target structure:

~~~text
src/
  i18n/
    index.ts
    types.ts
    vi.ts
    en.ts
~~~

Conceptual API:

~~~ts
t("backup.create")
t("common.retry")
t("system.diagnostics.title")
~~~

Translation catalogs dùng cùng key shape. `en` không có key riêng tùy ý mà `vi` thiếu, và ngược lại.

Key groups nên theo domain/surface:

~~~text
common.*
language.*
navigation.*
onboarding.*
overview.*
settings.*
system.*
diagnostics.*
repair.*
logs.*
backup.*
restore.*
errors.*
tray.*
accessibility.*
~~~

Không dùng full Vietnamese sentence làm key.

## Static HTML và bootstrap

Current shell có nhiều text nằm trực tiếp trong `index.html`. Phase 7.5 phải chuyển các text user-facing đó sang một trong hai cách:

- node có stable translation key để i18n runtime cập nhật; hoặc
- node được tạo/render từ TypeScript bằng translation catalog.

Không giữ hai bản copy song song trong HTML rồi hide/show bằng CSS.

Trước khi locale được resolve, initial shell chỉ nên hiển thị branding/loading trung tính hoặc language chooser. English preference đã lưu không được thấy một flash đáng kể của full Vietnamese UI rồi mới đổi sang English.

## Translation rules

1. Translation value là plain text mặc định.
2. Không đưa translation/interpolation chưa tin cậy vào `innerHTML`.
3. Dynamic store name, username, path summary hoặc error detail được gắn qua `textContent`/DOM node an toàn.
4. Placeholder có tên, ví dụ `{storeName}`, không phụ thuộc thứ tự từ.
5. Không ghép câu bằng nhiều fragment dịch rời nếu trật tự từ có thể khác giữa locale.
6. Date/time/number dùng formatter theo locale, không tự nối `dd/mm/yyyy` bằng string.
7. Product names, usernames, file names, version strings và technical ids không dịch.
8. Stable internal values như `home/settings/diagnostics`, error code, stage code và command name không đổi theo locale.

Fallback cho translation key:

- development/test: missing key hoặc catalog shape lệch phải fail validation;
- production: nếu key của locale hiện tại bất ngờ thiếu, thử cùng key ở `vi` để giữ UI usable và ghi diagnostic chỉ chứa key id;
- nếu key cũng thiếu ở `vi`, hiển thị generic localized UI error thay vì throw làm blank screen;
- normal accepted flows không được dựa vào fallback này; key parity phải đầy đủ nên không có mixed-language copy.

Ví dụ:

~~~ts
t("backup.completed_at", {
  time: formatDateTime(createdAt, locale),
})
~~~

## User-facing errors

Phase 7.5 không nên tiếp tục phụ thuộc vào exact English/Vietnamese Rust sentence để quyết định UI behavior.

Đối với command/error được Desktop shell hiển thị cho user, target contract là stable error code + safe structured context:

~~~json
{
  "code": "destination_not_writable",
  "context": {},
  "technical_detail": "..."
}
~~~

Frontend map:

~~~text
errors.destination_not_writable
  vi → Không thể ghi vào thư mục đã chọn.
  en → The selected folder is not writable.
~~~

`technical_detail`:

- không chứa secret;
- không dùng làm translation key;
- chỉ hiển thị trong **Chi tiết kỹ thuật** khi phù hợp;
- có thể giữ ngôn ngữ kỹ thuật gốc nếu chưa có semantic translation.

Không bắt Phase 7.5 dịch raw third-party output từng dòng. User-facing summary/recovery action phải localized.

### Migration từ Result<T, String>

Code hiện tại còn command trả raw `String`. Phase 7.5 cần ưu tiên các string thực sự xuất hiện trong UI:

- startup/config;
- onboarding/provisioning;
- runtime/health;
- repair;
- logs;
- backup;
- restore.

Các path này phải có stable classification/code trước khi được coi là localized hoàn chỉnh. Internal developer-only panic/test messages không thuộc sweep này.

## Native-owned UI surfaces

Một số string không render trong WebView, ví dụ:

- system tray menu;
- Close/Alt+F4 confirmation;
- native Desktop-owned dialog title/action khi implementation kiểm soát được label.

Các surface này phải đọc effective `app_language` và dùng cùng semantic translation.

Rust có thể giữ một catalog nhỏ dành riêng cho native-owned strings:

~~~text
tray.open
tray.quit
shutdown.title
shutdown.confirm
shutdown.cancel
~~~

Không duplicate toàn bộ frontend catalog sang Rust.

Nếu OS-owned file dialog có text do Windows tự render, Phase 7.5 không ép override locale của OS.

## Navigation và state khi đổi locale

Đổi language chỉ thay presentation.

Phải giữ:

- current top-level route;
- current Hệ thống subview;
- backup/restore operation id;
- progress state;
- diagnostics/repair/log result đang hiển thị;
- non-secret form draft;
- focus ở control hợp lý.

Không:

- gọi provisioning lại;
- restart runtime;
- tạo backup/restore operation mới;
- reset error/recovery state;
- clear store/admin identity;
- thay `startup_view`.

Nếu app/WebView reload sau khi user đổi language, `get_shell_info` phải khôi phục locale đã persist trước khi render shell hoàn chỉnh.

## Onboarding secret handling

Đổi locale trong onboarding không được làm password đi qua translation layer hoặc log.

Nếu language chỉ được chọn ở màn đầu rồi onboarding bắt đầu, password field chưa tồn tại và không có vấn đề.

Nếu future UX cho đổi locale giữa form:

- password DOM value không serialize vào translation state;
- re-render không copy password vào debug/log;
- không đưa password vào translation interpolation;
- nếu implementation không chứng minh giữ secret an toàn, password field có thể được clear với notice localized.

Phase 7.5 baseline chỉ yêu cầu language picker trước form và Settings sau install.

## Backup/restore integration

`app_language` là **target profile preference**, không phải store business data.

Giữ contract Phase 7.1:

- portable `config/store.json` không chứa `app_language`;
- raw `config/app.json` không được backup;
- restore vào profile hiện có giữ locale của target;
- fresh restore dùng locale user đã chọn trước khi bắt đầu restore;
- source backup không đổi ngôn ngữ của target.

Ví dụ:

~~~text
Máy A: Desktop = English
      ↓ backup
Máy B: chọn Tiếng Việt
      ↓ restore backup của A
Máy B sau restore: Desktop = Tiếng Việt
~~~

Backup manifest/warning/error codes giữ stable machine values; chỉ UI label/message được dịch.

## Accessibility và layout

Localization acceptance không chỉ kiểm đúng câu chữ.

Phải verify:

- `<html lang="vi">` hoặc `<html lang="en">` cập nhật đúng;
- aria-label/aria-description/status message đổi cùng locale;
- focus không mất khi hot-switch;
- button/select không bị cắt ở 100% và 150% DPI;
- layout vẫn cuộn được khi English copy dài hơn;
- error/progress không chỉ dựa vào màu;
- screen-reader live region không phát cả bản cũ và bản mới sau switch;
- keyboard Tab/Enter/Space vẫn thao tác language chooser/settings.

Không dùng fixed width chỉ vừa text tiếng Việt hiện tại.

## Functional acceptance

### Fresh profile

1. Xóa/chuẩn bị disposable fresh profile không có setup/recovery state.
2. Launch app → màn đầu là language chooser song ngữ.
3. Chọn `English` → Welcome và toàn onboarding chuyển sang English.
4. Relaunch trước khi setup → language vẫn English.
5. Hoàn tất setup → installed shell vẫn English.
6. Fresh profile khác chọn `Tiếng Việt` → toàn flow dùng tiếng Việt.

### Existing/legacy profile

7. Config schema 1 cũ không có `app_language` vẫn mở được.
8. Existing store cũ không bị đưa ngược về language chooser.
9. Existing/partial recovery dùng fallback `vi` và vẫn tiếp tục recovery được.
10. Vào Cấu hình → đổi sang English → UI đổi ngay, runtime không restart.
11. Reload/relaunch → English vẫn được giữ.
12. Đổi lại Tiếng Việt → UI đổi ngay và persist.

### Coverage

13. Top-level navigation và Hệ thống sub-navigation có đủ hai locale.
14. Onboarding validation/loading/success/error có đủ hai locale.
15. Runtime start/stop/health states có đủ hai locale.
16. Diagnostics/repair/log viewer/export có đủ hai locale.
17. Backup create/progress/cancel/success/failure có đủ hai locale khi Phase 7.3 surface tồn tại.
18. Restore inspect/review/progress/rollback/recovery có đủ hai locale khi Phase 7.4 surface tồn tại.
19. Tray + close confirmation đổi theo saved locale.
20. Không có visible mixed-language string do hard-code sót trong normal supported flows, ngoại trừ product/technical identifiers hoặc OS-owned UI.

### Failure/fallback

21. Missing translation key trong development/test bị phát hiện; production không crash/blank screen.
22. Invalid persisted locale bị phân loại config error rõ ràng, không silently chọn ngôn ngữ khác.
23. Persist language failure giữ preference cũ và không corrupt `config/app.json`.
24. Double-click/repeated selection không tạo race làm config hỏng.
25. Đổi language trong lúc runtime đang running không thay process lifecycle.
26. Đổi language trong lúc read-only diagnostic/log view mở giữ nguyên result/data.

### Backup/restore preference

27. Backup tạo từ Desktop English restore vào target Tiếng Việt → target vẫn Tiếng Việt.
28. Backup tạo từ Desktop Tiếng Việt restore vào target English → target vẫn English.
29. Fresh restore chọn locale trước khi inspect backup; archive source không override locale đó.

## Validation dự kiến

Giữ validation gọn, tập trung vào contract mới:

~~~powershell
. .\scripts\use-local-rust.ps1
cargo fmt --manifest-path .\src-tauri\Cargo.toml -- --check
cargo test --manifest-path .\src-tauri\Cargo.toml --locked config
cargo check --manifest-path .\src-tauri\Cargo.toml --locked
npm run lint:ui
npm run build:ui
git diff --check
~~~

Focused frontend validation cần chứng minh:

- `vi` và `en` có cùng translation key set;
- interpolation thiếu placeholder bắt được trong development/test;
- locale switch update visible text + `html.lang`;
- unsupported locale bị reject;
- no raw password/secret tham gia translation state.

Manual Windows smoke:

1. fresh → English → setup/relaunch;
2. fresh → Tiếng Việt → setup/relaunch;
3. installed → đổi vi ↔ en nhiều lần;
4. kiểm Tổng quan/Cấu hình/Hệ thống + tray/close dialog;
5. mở diagnostics/repair/log/backup và đổi locale;
6. resize + 150% DPI + keyboard;
7. restore cross-profile để xác nhận target locale không bị archive override.

## Ngoài phạm vi

- Tự động chọn locale theo Windows display language.
- Thêm locale ngoài `vi` và `en`.
- Dịch WordPress Admin.
- Dịch WooCommerce Admin.
- Dịch CoffeePOS POS/KDS/customer display web app.
- Dịch nội dung store do user nhập.
- Dịch raw PHP/MariaDB/Caddy/Windows error output từng dòng.
- Cloud/account language sync giữa nhiều máy.
- Đưa locale preference vào portable backup.
- RTL layout.

## Definition of Done

Phase 7.5 chỉ hoàn thành khi fresh Windows profile cho chọn **Tiếng Việt / English** trước onboarding; selection persist và được dùng lại sau relaunch; installed store đổi language trong **Cấu hình** và shell đổi ngay không restart runtime; toàn bộ user-facing Desktop surfaces hiện có dùng translation catalog thay cho hard-coded copy; user-facing operational errors có stable classification để map sang hai locale; tray/close/accessibility text theo effective locale; legacy config/recovery không bị chặn bởi field mới; backup/restore giữ locale của target profile; UI build, focused config/i18n checks và manual Windows smoke cho cả hai locale pass.
