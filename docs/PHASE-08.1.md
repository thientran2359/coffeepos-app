# Phase 8.1 — LAN bind

Phase 8.1 mở CoffeePOS cho thiết bị khác trong cùng mạng nội bộ bằng một network mode có kiểm soát. Đây là thay đổi runtime/security, không chỉ là thay giá trị host. Desktop phải giữ đường local hiện tại hoạt động, chỉ expose web traffic cần cho CoffeePOS và luôn có đường quay về local-only nếu bind, TLS hoặc network preflight thất bại.

> **Trạng thái:** đặc tả sẵn sàng cho implementation Windows-first. Phase 8.1–8.2 chỉ dùng cho development/controlled acceptance cho tới khi Phase 8.3 hoàn tất security acceptance. LAN mặc định luôn tắt.

## Mục tiêu

1. Installed store có thể bật/tắt truy cập LAN bằng hành động explicit trong Cấu hình.
2. Caddy phục vụ CoffeePOS trên một IPv4 LAN cụ thể đã chọn, đồng thời giữ một listener loopback riêng cho native health/lifecycle.
3. MariaDB, PHP FastCGI và Caddy admin không bao giờ bind ra LAN.
4. LAN origin dùng HTTPS để credential POS không đi qua mạng dưới dạng plaintext.
5. WordPress/CoffeePOS dùng đúng canonical origin đang effective, nên redirect, uploads và login/session không quay về 127.0.0.1 trên thiết bị khác.
6. Failure khi enable/rebind không làm mất local store; runtime phải quay về local-only hoặc giữ trạng thái local đã biết là healthy.
7. Native management/machine-health endpoint không trở thành endpoint LAN chỉ vì Caddy có listener mới.

## Entry gate đã chốt

Trước khi có LAN listener, implementation phải giữ các quyết định sau:

- local-only là default cho fresh profile, legacy profile chưa từng opt-in và fresh-profile restore;
- network preference thuộc target machine/profile, không phải portable store data;
- không dùng 0.0.0.0 làm URL hiển thị hoặc canonical WordPress origin;
- initial Windows LAN support là IPv4; IPv6/mDNS/discovery có thể bổ sung sau;
- canonical LAN origin là HTTPS trên một địa chỉ IPv4 của adapter được chọn;
- listener native/internal vẫn là HTTP loopback và không được quảng bá;
- machine-health token không đưa sang browser, URL, clipboard hoặc LAN client;
- không mở MariaDB, FastCGI, Caddy admin, Tauri IPC hoặc Desktop management command ra network;
- không tự tạo port-forward/UPnP/NAT/public tunnel;
- Windows network profile Public không được coi là mạng LAN hợp lệ để bật sharing;
- Phase 8 không tự đổi category của Windows network profile.

## Topology mục tiêu

Local-only:

~~~text
Desktop/native
   │
   ├── 127.0.0.1:<http_port> ── Caddy internal listener
   │                                │
   │                                └── 127.0.0.1:<fastcgi_port> ── PHP
   │
   ├── 127.0.0.1:<db_port> ───── MariaDB
   └── 127.0.0.1:<caddy_admin> ─ Caddy admin
~~~

LAN enabled:

~~~text
Desktop/native
   │
   ├── http://127.0.0.1:<internal_port> ─ Caddy internal listener
   │       └── native readiness + machine-health only
   │
   ├── https://<selected-lan-ip>:<lan-port> ─ Caddy LAN listener
   │       └── CoffeePOS web surface allowed for LAN clients
   │
   ├── 127.0.0.1:<fastcgi_port> ─ PHP FastCGI
   ├── 127.0.0.1:<db_port> ───── MariaDB
   └── 127.0.0.1:<caddy_admin> ─ Caddy admin
~~~

Không dựa vào firewall để biến một bind rộng thành an toàn. Caddy LAN listener phải bind đúng selected address hoặc dùng một cơ chế tương đương chứng minh chỉ expose interface đã chọn. Wildcard bind chỉ được dùng nếu implementation có route/interface isolation tương đương và test chứng minh không lộ trên adapter ngoài scope.

## Network preference và config migration

Config hiện tại có bind_host = 127.0.0.1 và schema 1. Phase 8 không nên biến field này thành arbitrary user input. Persist intent ở mức sản phẩm:

~~~json
{
  "network_mode": "local_only",
  "lan_adapter_id": null
}
~~~

Conceptual values:

~~~text
network_mode:
  local_only
  lan

lan_adapter_id:
  stable Windows adapter/interface identity
  null khi local-only hoặc chưa chọn
~~~

Exact schema migration có thể giữ legacy bind_host trong một version chuyển tiếp nếu implementation cần backward compatibility, nhưng các invariant bắt buộc là:

- legacy config hợp lệ với bind_host = 127.0.0.1 migrate thành local_only;
- arbitrary hostname/IP từ file cũ không được tin trực tiếp;
- effective IP luôn derive lại từ adapter hiện tại;
- portable backup/restore không mang network_mode, adapter identity, LAN certificate/private key hoặc firewall state sang profile đích;
- restore vào existing profile giữ network preference/TLS identity của chính target khi preference vẫn hợp lệ; restore staging luôn loopback-only và LAN chỉ được resume sau committed restore + final active health;
- fresh-profile restore dùng local_only và target-local TLS identity chỉ được tạo/dùng nếu user bật LAN sau restore;
- save vẫn atomic và giữ file cũ nếu validation/migration fail.

## Chọn adapter và địa chỉ

Windows-first dùng API hệ điều hành trực tiếp, không parse output của ipconfig/PowerShell.

Candidate address phải:

- thuộc adapter đang Up;
- là IPv4 unicast;
- không phải loopback;
- không phải unspecified/multicast;
- không phải APIPA 169.254.0.0/16;
- có network profile hợp lệ cho LAN mode;
- có stable adapter identity để phát hiện DHCP address change trên cùng adapter.

Nếu có nhiều candidate, 8.1 có thể chọn recommended candidate theo default route/network profile để chạy development acceptance. Phase 8.2 bổ sung UI chọn/đổi network rõ ràng. Không silently chuyển sang một adapter khác khi adapter đã chọn biến mất.

## Canonical origin

Current runtime dùng COFFEEPOS_SITE_URL để set WP_HOME/WP_SITEURL và upload URL. Phase 8 mở rộng contract này:

~~~text
local_only
  canonical_origin = http://127.0.0.1:<internal-port>

lan
  canonical_origin = https://<selected-lan-ip>:<lan-port>
~~~

Canonical origin phải là một structured native value, không ghép từ input frontend.

Khi LAN enabled:

- WP_HOME/WP_SITEURL nhận LAN HTTPS origin;
- uploads bridge dùng cùng origin;
- redirect/login/static asset URL phải giữ đúng LAN origin;
- open_pos và URL người dùng copy phải lấy từ current effective canonical origin;
- native health có thể connect qua loopback nhưng phải gửi request theo host/origin contract phù hợp, không bắt WordPress quay lại loopback chỉ để native probe chạy được.

Đổi canonical host/protocol có thể làm browser session cũ không còn áp dụng. UI ở 8.2 phải báo trước rằng thiết bị có thể cần đăng nhập lại sau khi đổi network/address.

## HTTPS và LAN certificate

LAN mode không gửi POS credential qua plain HTTP. Phase 8 dùng local TLS identity của target profile.

Contract:

- LAN listener chỉ quảng bá URL https://;
- certificate có SAN khớp effective LAN IP/hostname được hỗ trợ;
- private CA/server key là target-local secret/material và không đi vào portable backup;
- public root certificate có thể export cho thiết bị LAN ở Phase 8.2;
- Desktop hiển thị fingerprint của public root/certificate để người dùng đối chiếu;
- private key không qua IPC, clipboard, log hoặc support bundle;
- nếu certificate preparation fail thì LAN enable fail trước khi listener được quảng bá;
- local-only vẫn chạy được dù LAN certificate hỏng/mất.

Caddy internal PKI hoặc một native-generated local CA đều có thể dùng nếu đáp ứng contract trên. Exact primitive phải được pin/test trước implementation; không dựa vào browser warning và nút “continue anyway” như flow sản phẩm.

## Caddy routing boundary

LAN listener không phải mirror nguyên xi internal listener.

Internal listener được dùng cho:

- runtime nonce/readiness;
- WordPress/native health;
- authenticated CoffeePOS machine-health;
- lifecycle/drain primitives cần HTTP nội bộ.

LAN listener chỉ expose web surface cần cho POS/KDS/customer display và plugin transport đã được xác minh. Ngay từ 8.1:

- /wp-json/coffeepos/v1/system/status không được trả machine-health payload cho remote client;
- Caddy admin endpoint không nằm trên LAN listener;
- runtime nonce/readiness path không nằm trên LAN listener;
- file nhạy cảm hiện đang block vẫn giữ block;
- unexpected Host không được dùng làm canonical origin.

Phase 8.3 sẽ harden allow/deny matrix đầy đủ, wp-admin/login exposure, auth/session, origin và firewall.

## Runtime apply transaction

Bật/tắt LAN làm thay đổi listener và canonical WordPress origin nên phải serialize như lifecycle operation.

~~~text
User changes network mode
        ↓
Validate config + selected adapter/profile
        ↓
Prepare TLS identity if enabling LAN
        ↓
Acquire lifecycle/admission gate
        ↓
Drain current Caddy requests
        ↓
Apply/restart managed web layer with new origin
        ↓
Internal readiness
        ↓
WordPress health
        ↓
CoffeePOS machine-health
        ↓
LAN listener probe when enabled
        ↓
Persist effective network preference
        ↓
Release gate
~~~

Nếu bất kỳ bước nào sau mutation fail:

1. stop candidate LAN listener;
2. restore last known-good local-only/internal config;
3. restart/verify local runtime;
4. chỉ persist LAN preference khi final effective state đã verify;
5. nếu local rollback cũng fail, dùng structured runtime error + existing Diagnostics/Repair path, không reset store.

Không được để config nói lan trong khi runtime thực tế chỉ local, hoặc ngược lại.

## Operation gating

Không apply network mode đồng thời với:

- provisioning;
- start/stop/restart đang chạy;
- backup maintenance;
- restore transaction;
- repair apply;
- shutdown drain.

Read-only network inspection có thể chạy song song nếu không giữ lifecycle lock lâu. Network apply phải chạy ngoài Tauri UI thread theo async + blocking worker pattern của Phase 6.2.

## UI tối thiểu của 8.1

Phase 8.1 có control thật trong Cấu hình để flow có thể nghiệm thu từ app:

~~~text
Cấu hình

Mạng nội bộ

Truy cập từ thiết bị khác        [ Tắt ]
Chỉ bật trên mạng riêng mà bạn tin cậy.

[ Bật truy cập LAN ]
~~~

Khi bật lần đầu và có candidate hợp lệ:

~~~text
Bật truy cập mạng nội bộ?

CoffeePOS sẽ khởi động lại phần web để thiết bị trong
mạng đã chọn có thể kết nối.

Thiết bị có thể cần cài chứng chỉ tin cậy trước khi đăng nhập.

[ Hủy ] [ Bật ]
~~~

8.1 chưa cần UI address/copy hoàn chỉnh; phần đó thuộc 8.2. Nhưng loading/error/rollback phải phản ánh đúng operation thật.

## Structured runtime state

Runtime/network snapshot cần đủ dữ liệu để frontend không tự suy luận:

~~~text
configured_mode
effective_mode
adapter_id
adapter_name
lan_address
internal_origin
canonical_origin
lan_listener_state
tls_state
network_profile
last_error
~~~

Không trả private key, machine token hoặc hidden internal management URL không cần cho UI.

## Failure cases bắt buộc

1. Không có LAN adapter hợp lệ.
2. Adapter đang Public.
3. Selected adapter biến mất trước apply.
4. IP đổi giữa preflight và bind.
5. LAN port bị chiếm.
6. TLS identity/certificate không tạo hoặc không đọc được.
7. Caddy LAN listener bind fail nhưng internal listener vẫn có thể chạy.
8. WordPress health fail sau canonical origin change.
9. CoffeePOS machine-health fail sau rebind.
10. App bị đóng/crash giữa apply; relaunch phải đọc persisted last-known-good state và không tự expose LAN nếu commit chưa hoàn tất.

## Acceptance

Phase 8.1 chỉ Done khi các test sau pass trên disposable Windows store:

1. Fresh/legacy/fresh-restore profile đều khởi động local-only; existing target restore không nhận network state từ archive, giữ target preference và chỉ resume LAN sau restore commit + final health.
2. Bật LAN trên Windows Private network qua app, runtime reconfigure/restart không freeze UI.
3. Một thiết bị khác trong cùng LAN truy cập canonical HTTPS POS URL sau khi trust certificate.
4. POS login form và static/uploads URL không redirect về 127.0.0.1.
5. MariaDB port không connect được từ LAN; FastCGI và Caddy admin không reachable từ LAN.
6. Remote request tới machine-health/internal readiness không nhận protected payload.
7. Stop/start/relaunch giữ explicit LAN preference chỉ khi previous enable đã commit thành công.
8. Tắt LAN đưa app về loopback, remote URL ngừng reachable, local POS vẫn chạy sau health verification.
9. Inject bind/TLS/health failure: app rollback về local-only, không đổi store data và không để Caddy/PHP/MariaDB orphan.
10. Đổi HTTP/LAN port do conflict vẫn tạo đúng canonical origin; frontend không cache URL cũ.
11. Focused Rust/TypeScript tests, cargo fmt/check phù hợp và git diff --check pass.

## Ngoài scope

- Address chooser/copy/trust-certificate UX hoàn chỉnh: Phase 8.2.
- Firewall/security matrix và production LAN acceptance: Phase 8.3.
- mDNS/coffee.local/service discovery.
- Public internet, cloud relay, port forwarding hoặc remote administration.
- IPv6 LAN.
- Staff/device provisioning mới bên ngoài auth/session hiện có của CoffeePOS.
- Distribution/installer firewall automation: chỉ bổ sung nếu Phase 8.3/9.x chốt cần thiết.
