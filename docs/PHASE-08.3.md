# Phase 8.3 — LAN security

Phase 8.3 là security/release gate của toàn bộ LAN mode. 8.1 đã tạo listener có isolation và HTTPS; 8.2 đã cho người vận hành chọn network, copy URL và trust certificate. 8.3 xác minh toàn flow dưới góc nhìn một thiết bị LAN không đáng tin tuyệt đối: chỉ CoffeePOS surface cần thiết được reachable, auth/session vẫn do WordPress/CoffeePOS bảo vệ, private runtime/native control không lộ, Windows Public network fail closed và quay về local-only luôn hoạt động.

> **Trạng thái:** đặc tả sẵn sàng cho implementation sau 8.1–8.2. Chỉ sau khi 8.3 pass mới được coi LAN mode là khả năng có thể dùng trong vận hành.

## Mục tiêu

1. Xác định allow/deny matrix cụ thể cho LAN web surface.
2. Login credential, auth cookie và request có state-changing data chỉ đi qua HTTPS.
3. WordPress/CoffeePOS session hoạt động đúng trên canonical LAN origin mà không mở CORS rộng.
4. wp-admin, native health/control và các endpoint kỹ thuật không bị expose ngoài scope.
5. Windows firewall/network-profile behavior rõ ràng, không khuyến khích mở trên Public network.
6. Network transition Private → Public hoặc adapter trust thay đổi làm LAN fail closed.
7. Tắt LAN/recovery luôn quay về loopback mà không mất store data.
8. Multi-device sync chỉ được tuyên bố theo transport/plugin behavior đã test thật.

## Threat model

Phase 8 bảo vệ trước các tình huống thực tế sau:

- thiết bị khác cùng Wi-Fi dò port;
- client chưa đăng nhập gọi API trực tiếp;
- client gửi Host/Origin giả;
- credential bị sniff trên mạng nếu transport không mã hóa;
- browser gửi cross-origin request từ trang khác;
- network adapter đổi sang Public;
- user vô tình share trên VPN/adapter khác;
- remote client thử machine-health token endpoint, runtime readiness, Caddy admin, FastCGI hoặc database;
- remote client request file nhạy cảm;
- stale LAN URL/certificate sau DHCP/restart;
- LAN listener còn sống sau khi UI/config đã tắt hoặc runtime shutdown.

Không coi CORS là auth. Không coi “cùng Wi-Fi” là trusted identity.

## Security invariants

Các invariant sau phải đúng ở mọi state:

1. MariaDB bind loopback-only.
2. PHP FastCGI bind loopback-only.
3. Caddy admin bind loopback-only.
4. Tauri IPC/capability không có remote origin.
5. Machine-health secret không đi qua LAN.
6. LAN credential traffic dùng HTTPS với certificate đã trust.
7. LAN mode là explicit opt-in và chỉ trên approved adapter/profile.
8. Không UPnP/NAT/public tunnel.
9. Disable LAN đóng listener remote sau bounded drain.
10. Config/effective state không được diverge sau crash/recovery.

## LAN route allow/deny matrix

Trước implementation, audit exact CoffeePOS plugin routes dùng cho:

- POS;
- KDS nếu được support;
- customer display nếu được support;
- static assets/uploads;
- REST/AJAX endpoints thật sự cần cho các màn trên;
- login/logout/session refresh.

Sau audit, Caddy LAN listener dùng allowlist hoặc deny rules đủ hẹp để đạt behavior đó. Baseline:

| Surface | LAN policy |
| --- | --- |
| POS route + assets cần thiết | Allow |
| CoffeePOS API cần cho POS/KDS/display | Allow theo exact route contract |
| Upload/static public content cần render | Allow |
| POS login/logout do plugin sở hữu | Allow qua HTTPS |
| wp-admin | Deny mặc định; chỉ mở nếu product requirement riêng được chốt |
| wp-login.php | Deny nếu POS login flow không cần; current Phase 5.4 dùng login form tại /pos/ |
| machine-health system/status | Deny remote |
| runtime nonce/readiness | Deny remote |
| Caddy admin | Không có LAN listener |
| FastCGI | Không có LAN listener |
| MariaDB | Không có LAN listener |
| wp-config/.env/composer/.htaccess | Deny như baseline hiện tại |
| XML-RPC | Deny trừ khi có requirement và acceptance riêng |
| arbitrary PHP/debug/test path | Deny |

Không block theo filename pattern rồi coi phần còn lại an toàn. Route policy phải được kiểm với actual CoffeePOS requests.

## Host header và canonical origin

LAN listener chỉ chấp nhận host/origin đã commit cho current LAN state.

Yêu cầu:

- unexpected Host trả 400/404 hoặc equivalent deny;
- redirect không reflect arbitrary Host;
- password reset/login redirect không lấy attacker-supplied host làm canonical URL;
- uploads/assets không sinh URL từ Host header ngoài contract;
- native internal listener có host policy riêng và không biến LAN host thành quyền management.

Tests phải bao gồm Host khác canonical IP, localhost, attacker.example và stale old LAN IP.

## Authentication và authorization

Desktop không tạo LAN password riêng nếu CoffeePOS/WordPress auth hiện tại đã đủ cho product surface.

Rules:

- unauthenticated client chỉ thấy/login vào surface mà plugin chủ động cho phép;
- API yêu cầu session/nonce/capability tiếp tục enforce server-side;
- Desktop không auto-login remote device;
- admin password không nằm trong share URL/QR;
- machine token không dùng như user/session token;
- role/capability decisions thuộc CoffeePOS/WordPress, không duplicate trong Caddy/Rust.

Nếu audit phát hiện endpoint CoffeePOS state-changing hiện dựa vào “local network” thay vì authenticated user/capability, 8.3 bị blocked cho tới khi plugin contract được sửa.

## Session, cookie và CSRF

Canonical LAN origin dùng HTTPS nên acceptance phải kiểm browser thật:

- valid POS login tạo session và redirect trong canonical HTTPS origin;
- auth cookie có Secure khi chạy HTTPS nếu WordPress/plugin contract yêu cầu;
- HttpOnly/SameSite behavior không bị Desktop/Caddy làm yếu đi;
- logout invalidates session theo plugin;
- session expiry quay lại login đúng;
- state-changing POST/API vẫn yêu cầu nonce/CSRF primitive hiện có;
- Origin/Referer validation nếu plugin dùng phải chấp nhận exact canonical origin;
- không thêm Access-Control-Allow-Origin: * để “sửa” browser error;
- không thêm Access-Control-Allow-Credentials rộng cho arbitrary origin.

Nếu POS/KDS/customer display cùng origin thì ưu tiên same-origin. Cross-origin chỉ được thêm khi có requirement cụ thể và allowlist exact origin.

## TLS hardening

LAN HTTPS acceptance:

- protocol/cipher do current supported Caddy/OS baseline quản lý; disable plaintext LAN redirect nếu nó cho phép credential đi qua HTTP;
- HTTP request tới LAN port không được serve login form plaintext;
- certificate SAN khớp exact effective address/hostname;
- public trust cert export không chứa private key;
- private CA/server key nằm trong protected target-local data, permission hạn chế;
- support bundle/log không chứa private key;
- certificate/key không nằm trong portable backup;
- restore sang profile/máy khác dùng TLS identity của target, không mang identity của source.

Rotation/recreate:

- nếu CA/private key mất/corrupt, LAN bị disabled hoặc tls_needs_attention;
- explicit recreate tạo trust identity mới và cảnh báo client phải trust lại;
- không silently rotate mỗi restart vì sẽ phá client trust.

## Windows network profile policy

Windows-first chỉ hỗ trợ LAN sharing trên network profile Private trong Phase 8.

Public:

- không enable listener LAN;
- nếu current selected network chuyển Private → Public khi app đang chạy, LAN phải fail closed;
- local loopback runtime tiếp tục nếu có thể;
- UI báo “Mạng đang là Công cộng” và hướng dẫn user thay đổi network setting ngoài CoffeePOS nếu họ thật sự tin network;
- Desktop không tự sửa Windows network category.

Domain/enterprise profile:

- ngoài initial support trừ khi acceptance riêng được chốt;
- không suy Domain = trusted café LAN.

## Firewall policy

Phase 8 không âm thầm tắt Windows Firewall và không thêm broad allow-all rule.

Initial supported flow:

- LAN bind chỉ trên selected Private adapter/address;
- UI giải thích Windows Firewall có thể chặn inbound;
- dùng standard Windows firewall permission/rule flow cho exact CoffeePOS web server/port khi cần;
- nếu implementation có thể đọc firewall state qua stable Windows API, hiển thị diagnostic guidance nhưng không coi read failure là lý do mở rộng rule;
- không yêu cầu mở database/FastCGI/admin port;
- không hướng dẫn tắt firewall toàn bộ;
- không dùng shell command tùy ý từ frontend.

Nếu project quyết định tự quản firewall rule trong implementation, phải có spec bổ sung trước code:

- explicit user confirmation/elevation;
- dedicated stable rule identity;
- Private profile only;
- exact TCP LAN port/local address;
- remove/update on disable/address change;
- rollback nếu rule mutation fail;
- upgrade/uninstall ownership;
- không để stale broad rule sau app removal.

Không bắt buộc automatic firewall mutation để Phase 8.3 pass nếu Windows standard allow flow + guidance đã chứng minh usable và deterministic trên target acceptance.

## Network change fail-closed

Khi selected adapter:

- mất kết nối;
- chuyển sang Public;
- bị thay bằng adapter identity khác;
- address không còn thuộc adapter;

Desktop không tiếp tục quảng bá old lan_ready.

Required result:

~~~text
LAN listener disabled/unreachable
        ↓
local listener preserved/recovered
        ↓
network state marked needs attention
        ↓
user explicitly selects/applies trusted network again
~~~

Không auto-bind sang Wi-Fi/VPN mới chỉ vì nó có default route.

## Device-to-device / multi-client behavior

“Thiết bị truy cập được trang” không đủ để tuyên bố LAN usable.

Acceptance phải chạy ít nhất:

- Desktop/local POS + một remote POS/browser;
- hoặc POS + KDS/customer display nếu plugin hiện support;
- simultaneous catalog/API load;
- login/logout độc lập;
- tạo một test order trên client được hỗ trợ;
- client khác quan sát thay đổi qua đúng transport plugin đang dùng;
- reconnect sau brief network loss;
- request dài + runtime shutdown/drain theo Phase 6.2.

Nếu plugin dùng polling, REST, SSE, WebSocket hoặc transport khác, tài liệu validation phải ghi đúng cái đã test. Không ghi “real-time sync” nếu implementation thực tế chỉ polling eventual update.

## Abuse/negative tests

Từ một máy khác trong LAN:

1. Port scan chỉ thấy intended LAN HTTPS listener trong phạm vi CoffeePOS.
2. Connect DB port fail.
3. Connect FastCGI/Caddy admin fail.
4. GET machine-health route không trả schema/store/version payload.
5. GET internal readiness/probe path fail.
6. GET wp-config.php/.env/composer files fail.
7. Unexpected Host bị reject.
8. HTTP plaintext không nhận login form/credential POST.
9. Unauthenticated state-changing CoffeePOS API bị reject.
10. Cross-origin request từ arbitrary origin không được CORS wildcard + credentials chấp nhận.
11. Stale old IP sau rebind không còn serve store.
12. Disable LAN đóng remote listener nhưng local runtime vẫn usable.

## Recovery và rollback

### User tắt LAN

~~~text
Confirm
  ↓
Drain LAN listener
  ↓
Return canonical origin to loopback
  ↓
Restart/reload managed web layer
  ↓
Internal health + CoffeePOS health
  ↓
Persist local_only
~~~

Browser/device LAN cũ có thể mất session/reachability; đây là expected.

### Security precondition fail khi startup

Ví dụ saved LAN preference nhưng adapter nay Public:

- không restore LAN listener automatically;
- start local-only;
- keep configured preference as needing attention hoặc downgrade configured mode only through explicit contract;
- UI giải thích LAN chưa bật vì network không còn trusted;
- không block operator khỏi local POS nếu local health pass.

### Crash giữa LAN apply

Persisted network apply state phải đủ để phân biệt candidate/committed state. Relaunch không được expose candidate listener chỉ vì config partially written. Last-known-good local state là recovery baseline.

## Logging và diagnostics

Allowed:

- network mode state;
- adapter display name/id dạng không nhạy cảm cần cho debug;
- IP/port trong technical diagnostics/support bundle nếu current redaction policy cho phép;
- TLS state/fingerprint public;
- firewall/profile classification;
- structured error code.

Never log:

- admin password;
- auth cookie;
- WordPress nonce nếu có thể replay;
- machine-health token;
- TLS private key;
- exported secret path chứa user credential.

## Acceptance matrix

Phase 8.3 Done khi Windows disposable-store matrix pass:

| Scenario | Expected |
| --- | --- |
| Fresh app/default | LAN off, only loopback |
| Enable on Private Wi-Fi | HTTPS LAN ready after explicit opt-in |
| Enable on Public Wi-Fi | Blocked/fail closed |
| Remote POS login | Works through canonical HTTPS |
| Invalid login | Rejected without native side effect |
| Two clients | Sessions independent; plugin behavior consistent |
| Remote machine-health | Protected/internal data unavailable |
| DB/FastCGI/admin scan | Not reachable |
| Host spoof | Rejected/no attacker-host redirect |
| Cross-origin state change | Rejected unless exact supported origin contract |
| DHCP address change | Old URL stale; explicit apply creates verified new URL |
| Selected adapter disconnect | LAN unavailable, local path preserved |
| Private → Public transition | LAN disabled/fail closed |
| Disable LAN | Remote closes; local-only health pass |
| Restart/relaunch LAN | Only previously committed trusted state restored |
| Fresh restore to another profile | Target stays local-only; source network/TLS state is not imported |
| Restore into existing profile | Preserve valid target network preference/TLS identity; staging stays loopback-only and LAN resumes only after committed restore + final health |
| Shutdown with active LAN request | Bounded drain, no orphan listener/process |

Manual acceptance phải có ít nhất hai physical devices hoặc một physical remote device + isolated VM/network client đủ để chứng minh traffic đi qua LAN, không dùng request từ chính 127.0.0.1 làm bằng chứng remote.

## Definition of Done

Phase 8 chỉ được coi hoàn tất sau 8.3 khi có đủ:

- opt-in LAN flow từ app;
- canonical HTTPS URL + trust onboarding;
- supported network/address change flow;
- POS login/session thật trên remote device;
- exact plugin multi-client behavior đã test;
- Public network fail-closed;
- DB/FastCGI/Caddy admin/native control vẫn private;
- machine-health/internal endpoints không lộ qua LAN;
- disable/rollback local-only pass;
- lifecycle/shutdown không orphan listener/process;
- vi/en UI và diagnostics đúng;
- focused automated checks + manual Windows LAN acceptance được ghi lại.

## Ngoài scope

- Public internet access.
- Cloud relay/VPN management.
- Automatic router configuration/UPnP.
- mDNS/coffee.local.
- IPv6.
- Remote wp-admin unless later product requirement explicitly adds it.
- Enterprise/domain network certification.
- macOS LAN certification; macOS phải implement cùng security contract ở phase sau trước khi công bố support.
