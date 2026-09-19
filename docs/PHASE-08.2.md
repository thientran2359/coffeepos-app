# Phase 8.2 — LAN address UI

Phase 8.2 biến native LAN foundation của 8.1 thành một flow người vận hành có thể hiểu và dùng mà không phải tự chạy ipconfig, đoán IP hoặc copy port từ Diagnostics. UI phải luôn phản ánh effective listener hiện tại, hỗ trợ chọn network khi máy có nhiều adapter, hướng dẫn trust certificate và xử lý address change mà không âm thầm expose sang network khác.

> **Trạng thái:** đặc tả sẵn sàng cho implementation sau khi 8.1 pass. LAN vẫn chỉ dùng cho controlled acceptance cho tới khi 8.3 hoàn tất.

## Mục tiêu

1. Người dùng biết LAN đang Tắt / Đang bật / Cần xử lý.
2. Khi bật, UI hiển thị exact reachable HTTPS URL của listener đã verify.
3. Có thể copy URL bằng native clipboard mà không tự ghép host/port ở frontend.
4. Máy nhiều adapter cho phép chọn đúng network bằng tên adapter + IPv4 rõ ràng.
5. Khi DHCP/network thay đổi, UI phát hiện stale address và không hiển thị URL cũ như còn reachable.
6. Thiết bị mới có flow lấy public trust certificate + fingerprint mà không lộ private key.
7. Apply address/network mới giải thích impact tới session và rollback local-only nếu operation fail.

## Information architecture

Không thêm top-level tab. LAN nằm trong:

~~~text
Cấu hình
  └── Mạng nội bộ
~~~

Diagnostics vẫn có thể hiển thị technical network detail dưới Hệ thống, nhưng enable/disable/select/share URL là preference/action của Cấu hình.

## State model

Frontend nhận structured network state từ native:

| State | Ý nghĩa | UI chính |
| --- | --- | --- |
| local_only | LAN chưa bật | Bật truy cập LAN |
| enabling | Đang apply listener/TLS/origin | Progress, disable duplicate action |
| lan_ready | Listener + health đã verify | URL, Copy, network, certificate |
| address_changed | Adapter cũ còn nhưng IP effective đã đổi | Áp dụng địa chỉ mới |
| adapter_unavailable | Adapter đã chọn mất/down | Chọn mạng khác hoặc Tắt LAN |
| public_network | Network profile không đủ điều kiện | Hướng dẫn chuyển sang Private, không bind |
| tls_needs_attention | Certificate state cần repair/refresh | Xem chứng chỉ / thử lại |
| failed_local_safe | LAN apply fail, local-only đã verify | Lỗi + Thử lại |
| failed_needs_system_check | Rollback/local health chưa verify | Mở Hệ thống |

UI không tự suy state từ bind_host, http_port hoặc string lỗi.

## Wireframe — local-only

~~~text
Cấu hình

Mạng nội bộ

Truy cập từ thiết bị khác                         Tắt
CoffeePOS hiện chỉ dùng được trên máy này.

[ Bật truy cập LAN ]
~~~

English:

~~~text
Settings

Local network

Access from other devices                         Off
CoffeePOS is currently available on this computer only.

[ Enable LAN access ]
~~~

## Wireframe — chọn network

Nếu chỉ có một candidate hợp lệ, vẫn hiển thị network sẽ dùng trước confirm.

~~~text
Bật truy cập LAN

Chọn mạng dùng cho CoffeePOS

(•) Wi-Fi — 192.168.1.24
( ) Ethernet — 10.10.0.15

Chỉ các thiết bị trong mạng đã chọn mới nên được dùng
để truy cập CoffeePOS.

[ Hủy ] [ Tiếp tục ]
~~~

Không chỉ hiển thị “Wi-Fi” nếu có nhiều adapter cùng tên. Native trả stable adapter id để frontend giữ selection đúng; UI có thể thêm suffix dễ đọc khi cần.

## Wireframe — LAN ready

~~~text
Mạng nội bộ

Truy cập từ thiết bị khác                         Đang bật
Wi-Fi · 192.168.1.24

Địa chỉ CoffeePOS
https://192.168.1.24:8443
[ Sao chép địa chỉ ]

Thiết bị mới
Để trình duyệt tin cậy kết nối này, cài chứng chỉ CoffeePOS
trên thiết bị trước khi đăng nhập.
[ Xuất chứng chỉ tin cậy ]  [ Xem dấu vân tay ]

[ Đổi mạng ]  [ Tắt truy cập LAN ]
~~~

URL chỉ xuất hiện sau khi listener effective + TLS + application health đã verify. Không render URL candidate trong success card trước commit.

## Copy address

Frontend không tự ghép URL từ IP/port. Native trả canonical_origin hoặc có command copy current share URL.

Yêu cầu:

- chỉ copy lan_ready canonical HTTPS URL;
- không copy internal loopback URL;
- không append POS path nếu UI label là “Địa chỉ CoffeePOS”; nếu có action riêng “Sao chép địa chỉ POS” thì native lấy validated pos_path từ machine-health;
- clipboard success/failure có localized status;
- URL không chứa credential, machine token hoặc query secret.

## Trust certificate UX

Phase 8.1 đã chốt LAN dùng HTTPS với target-local CA/certificate. 8.2 cung cấp onboarding cho client.

### Export

Action “Xuất chứng chỉ tin cậy”:

- native Save As;
- chỉ export public root/intermediate certificate cần trust;
- không export private key;
- tên file an toàn, ví dụ CoffeePOS-LAN-Trust.cer;
- fingerprint SHA-256 lấy từ exact certificate bytes được export;
- export không cần dừng runtime.

### Fingerprint

~~~text
Chứng chỉ CoffeePOS

SHA-256
AB:CD:EF:...

Đối chiếu dấu vân tay này trên thiết bị trước khi tin cậy chứng chỉ.

[ Sao chép dấu vân tay ]
~~~

Không log fingerprint cùng bất kỳ secret nào. Fingerprint/public cert không phải secret, nhưng UI vẫn phải tránh tạo cảm giác đây là mã đăng nhập.

### Hướng dẫn

Initial Windows-first spec chỉ cần copy ngắn, không nhúng hướng dẫn OS dài vào màn hình chính. Có thể mở phần “Cách kết nối thiết bị” giải thích:

1. đưa public trust certificate sang thiết bị bằng kênh người dùng kiểm soát;
2. kiểm fingerprint;
3. cài/trust certificate theo OS;
4. mở HTTPS URL;
5. đăng nhập CoffeePOS bằng tài khoản hợp lệ.

Không hướng dẫn bỏ qua cảnh báo TLS.

## Network changes

Native phân biệt adapter identity với current address.

### DHCP đổi IP trên cùng adapter

Khi detected address khác effective address:

~~~text
Địa chỉ mạng đã thay đổi

Wi-Fi hiện dùng 192.168.1.51.
CoffeePOS vẫn đang dùng địa chỉ cũ và không còn được chia sẻ ổn định.

Áp dụng địa chỉ mới sẽ khởi động lại phần web.
Các thiết bị có thể cần đăng nhập lại.

[ Áp dụng địa chỉ mới ]
~~~

Không update URL displayed thành địa chỉ mới trước khi listener/canonical origin mới đã commit.

### Adapter mất/down

Giữ local path dùng được và hiện:

~~~text
Mạng đã chọn không còn khả dụng.
CoffeePOS trên máy này vẫn có thể tiếp tục chạy.

[ Chọn mạng khác ] [ Tắt truy cập LAN ]
~~~

Không tự chọn adapter khác vì hành động đó có thể expose store sang một network mà user chưa approve.

### Network chuyển sang Public

Phase 8.3 sẽ enforce fail-closed đầy đủ. UI 8.2 đã phải hiển thị state rõ:

~~~text
Mạng này đang được Windows đánh dấu là Công cộng.
CoffeePOS không chia sẻ trên mạng Công cộng.

[ Mở hướng dẫn mạng Windows ]
~~~

Desktop không tự đổi network profile sang Private.

## Apply/confirm UX

Các action Bật / Đổi mạng / Áp dụng địa chỉ mới / Tắt đều dùng operation state chung:

~~~text
Đang cập nhật mạng nội bộ…

✓ Đã kiểm tra network
✓ Đã chuẩn bị kết nối bảo mật
• Đang cập nhật CoffeePOS
○ Kiểm tra hệ thống
○ Hoàn tất
~~~

Frontend không giả progress từ timer. Native phải trả stage hoặc snapshot đủ để map từng bước.

Nếu operation fail nhưng local rollback pass:

~~~text
Không thể bật truy cập LAN

CoffeePOS đã quay về chế độ chỉ dùng trên máy này.
Dữ liệu cửa hàng không bị thay đổi.

[ Thử lại ]
~~~

Nếu local rollback không verify:

~~~text
Kết nối cần kiểm tra

Không thể xác minh CoffeePOS sau khi cập nhật mạng.

[ Mở Hệ thống ]
~~~

## Session impact

Đổi từ loopback sang LAN HTTPS hoặc từ LAN address A sang B tạo origin mới. UI phải nói rõ trước apply:

- tab/browser ở origin cũ có thể không còn truy cập được;
- cookie host cũ không được Desktop copy sang origin mới;
- người dùng có thể phải đăng nhập lại;
- Desktop không inject admin password hoặc session vào thiết bị LAN;
- order/cart state durability thuộc CoffeePOS/plugin, không suy đoán từ Desktop.

## Runtime/diagnostics presentation

Technical detail trong Hệ thống có thể hiển thị:

~~~text
Internal HTTP     127.0.0.1:<port>
LAN HTTPS         192.168.1.24:<port>
Network           Wi-Fi
Profile           Private
TLS               Ready
Canonical origin  https://192.168.1.24:<port>
~~~

Database vẫn hiển thị loopback. Không hiển thị machine token, certificate private-key path hoặc Caddy admin URL.

## Accessibility và localization

Toàn bộ string mới có key vi/en theo Phase 7.5.

Acceptance UI:

- keyboard Tab/Enter dùng được với toggle, radio/select, Copy, Export;
- focus trở về heading/action phù hợp sau modal/apply;
- status không chỉ dựa vào màu;
- network/address string không làm vỡ layout ở DPI/zoom cao;
- URL/fingerprint dùng selectable/copy-friendly presentation;
- live region chỉ announce state change, không lặp mỗi poll;
- đang apply thì chặn double click và route gây operation trùng.

## Native contract đề xuất

Tên command có thể điều chỉnh theo code style hiện tại, nhưng semantic cần tách read/apply:

~~~text
get_lan_settings()
  read-only candidates + configured/effective state

apply_lan_settings(mode, adapter_id?)
  validate + lifecycle transaction + rollback

copy_lan_url()
  native clipboard, current committed URL only

export_lan_trust_certificate()
  native Save As, public cert only

copy_lan_certificate_fingerprint()
  native clipboard, fingerprint only
~~~

Frontend không truyền raw bind address, arbitrary URL, certificate path hoặc firewall command.

## Acceptance

Phase 8.2 Done khi:

1. Local-only UI không cần người dùng biết IP/port.
2. Máy có một adapter: enable confirm hiển thị đúng network/address và sau commit hiển thị exact HTTPS URL.
3. Máy có nhiều adapter: chọn adapter A chỉ bind/share trên A; UI giữ đúng stable selection qua refresh/relaunch.
4. Copy address mở đúng CoffeePOS trên một thiết bị LAN đã trust certificate.
5. Export public trust cert + fingerprint khớp exact CA/certificate đang phục vụ; không có private key trong export.
6. DHCP đổi IP: UI phát hiện stale state, không quảng bá URL mới trước apply; apply mới rebind, health pass rồi đổi URL.
7. Adapter disconnect: local POS còn đường chạy, LAN card báo unavailable; app không auto-switch sang adapter khác.
8. Network Public: enable/apply bị chặn theo policy, copy giải thích được action tiếp theo.
9. Đổi LAN origin cảnh báo session re-login; browser cũ không được Desktop coi là session mới.
10. Failure khi apply hiển thị rollback-to-local result chính xác.
11. vi/en hot-switch render toàn bộ LAN UI không restart runtime.
12. Keyboard/resize/DPI + focused native/UI checks + git diff --check pass.

## Ngoài scope

- Full firewall automation/hardening/security matrix: Phase 8.3.
- QR code/device discovery/mDNS.
- Public-domain certificate hoặc cloud hostname.
- Auto-install certificate trên thiết bị khác.
- Remote Desktop administration.
- Business device registry nếu CoffeePOS plugin chưa có contract riêng.

