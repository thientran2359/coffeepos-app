# CoffeePOS Desktop — Đặc tả UI/UX

Ngày cập nhật: 2026-09-19. Đây là đặc tả trải nghiệm; bằng chứng triển khai nằm trong tài liệu từng phase. Phase 1–4.12 giữ nguyên kết quả kỹ thuật đã nghiệm thu; Phase 5.1 shell/navigation và Phase 5.2 store/account onboarding đã hoàn thành Windows-first. Phase 5.3–5.6 và Phase 6.1–6.4 đã triển khai và pass focused/lightweight automated checks; manual Windows acceptance còn chờ người dùng thực hiện. Sau snapshot Phase 6.1, installed-store shell đã được refactor theo kiến trúc thông tin **Tổng quan / Cấu hình / Hệ thống**, thay thế cách trình bày ba mục ngang **Trang chính / Cài đặt / Chẩn đoán**. **Chẩn đoán / Sửa chữa / Nhật ký** hiện là các chức năng cấp hai trong Hệ thống. Scope, thứ tự và trạng thái phase do [ROADMAP.md](ROADMAP.md) quản lý.

## 1. Mục tiêu

Người vận hành biết mình cần làm gì tiếp theo mà không phải hiểu WordPress, PHP, database, port hoặc health token. Desktop quản lý môi trường; giao diện bán hàng và nghiệp vụ vẫn do CoffeePOS cung cấp. Không xây dashboard doanh thu, giỏ hàng hoặc quản lý nhân viên lần nữa trong shell.

Giao diện hiện tại là công cụ development. Việc gom setup, runtime controls và cấu hình trên một trang không phải thiết kế sản phẩm cuối cùng. UI/UX phải được triển khai cùng các luồng thật, không để đến cuối dự án mới trang trí.

## 2. Cấu trúc màn hình

| Màn hình | Nội dung và hành động chính | Mốc triển khai |
| --- | --- | --- |
| Chọn ngôn ngữ | Fresh profile chọn **Tiếng Việt / English** trước Welcome; selection persist cho Desktop shell | 7.5 |
| Chào mừng | Giải thích ngắn: cài một lần, dữ liệu nằm trên máy; nút **Thiết lập cửa hàng** | 4.12: bắt đầu setup tối thiểu; 5.1–5.2: hoàn thiện |
| Thiết lập cửa hàng | Tên cửa hàng, tài khoản ban đầu, validation; nút **Cài đặt** | 5.2 |
| Tiến trình thiết lập | Các bước thật đã xong/đang chạy; lỗi và tiếp tục khi an toàn | 4.12, tích hợp form ở 5.2 |
| Tổng quan | Tên cửa hàng, trạng thái sử dụng, cảnh báo ngắn và một hành động chính phù hợp trạng thái | 5.3; shell IA refactor sau 6.1 |
| Cấu hình | Cấu hình thuộc Desktop, trạng thái lưu và lỗi; không nhân bản settings nghiệp vụ | 5.3; LAN bổ sung ở 8.x |
| Hệ thống | Sức khỏe runtime và các chức năng quản trị kỹ thuật. **Chẩn đoán** là màn hình/con mục bên trong Hệ thống, không còn là navigation cấp cao nhất | Khung 5.1; health 6.1; runtime performance 6.2; repair/log 6.3–6.4 |
| Hệ thống → Sao lưu và khôi phục | Tạo backup, thời điểm/kết quả, chọn và kiểm bản phục hồi khi chức năng 7.x có thật | 7.x |

Trước setup, dùng luồng theo bước, không đưa người dùng vào dashboard chưa có cửa hàng. Sau setup, navigation cấp cao chỉ gồm **Tổng quan / Cấu hình / Hệ thống**. **Chẩn đoán**, **Nhật ký**, **Repair**, **Sao lưu/Khôi phục** và các chức năng kỹ thuật tương lai nằm trong **Hệ thống** khi chúng có implementation thật. Không tạo trang rỗng hoặc nút hoạt động giả cho phase tương lai.

### 2.1. App shell và navigation sau setup

Ba mục cấp cao dùng cùng một loại khái niệm và cùng một cấp độ thông tin:

- **Tổng quan**: nơi vận hành hằng ngày. Hiển thị cửa hàng có sẵn sàng hay không, cảnh báo ngắn và action thường dùng như **Mở bán hàng**.
- **Cấu hình**: các preference và thiết lập thuộc CoffeePOS Desktop như màn hình mở đầu, ngôn ngữ Desktop, hành vi ứng dụng và cấu hình local/LAN khi phase tương ứng tồn tại.
- **Hệ thống**: trạng thái runtime và công cụ kỹ thuật. Landing của Hệ thống ưu tiên health summary; **Chẩn đoán** là chức năng bên trong khu vực này. Phase 6.2 tối ưu runtime phía sau UI; Phase 6.3 Repair và 6.4 Log viewer/export mở rộng Hệ thống thay vì tạo thêm top-level tab.

Shell dùng một cấu trúc ổn định trên cả ba khu vực:

```text
┌────────────────────────────────────────────────────┐
│ CoffeePOS                              ● Trạng thái │
├────────────────────────────────────────────────────┤
│ Tổng quan        Cấu hình        Hệ thống          │
├────────────────────────────────────────────────────┤
│ <vùng nội dung cuộn của màn hình hiện tại>         │
└────────────────────────────────────────────────────┘
```

Header và navigation giữ cùng vị trí khi đổi khu vực; phần nội dung phía dưới là vùng cuộn. Các màn hình dùng chung max-width, padding ngang, khoảng cách header → navigation → page heading → content và cùng hierarchy typography. Navigation cấp cao không dùng segmented-control/capsule nhiều lớp; active state dùng một cue rõ như underline hoặc nền nhẹ và vẫn có `aria-current`.

Internal config key hiện có được giữ tương thích trong lúc refactor: `startup_view=home` mở **Tổng quan**, `settings` mở **Cấu hình**, và `diagnostics` mở **Hệ thống** tại mục **Chẩn đoán**. Không đổi schema/config chỉ để đổi nhãn hiển thị.

## 3. Luồng chính

### Lần đầu

```text
Chọn ngôn ngữ → Chào mừng → Thông tin cửa hàng/tài khoản → Cài đặt
              → Tiến trình thật → Hoàn tất → Tổng quan → Mở bán hàng
```

Phase 4.12 hoàn thiện tiến trình/recovery trên input setup cũ; Phase 5.2 đã bổ sung form tài khoản, review/complete, protected user-set credential và nghiệm thu fresh flow đầy đủ. Không kéo các hành vi Phase 5.4 như mở POS vào onboarding.

Phase 7.5 thêm language chooser **chỉ cho brand-new profile** trước Welcome. Existing/partial legacy profile không bị chèn một gate mới giữa recovery; khi chưa có preference thì Desktop dùng fallback `vi`. Sau khi chọn, onboarding dùng một locale nhất quán. `app_language` là preference của Desktop profile và không tự đổi WordPress/POS locale; xem [PHASE-07.5.md](PHASE-07.5.md).

Màn hình tiến trình lấy trạng thái từ native. Nếu chưa có sự kiện từng bước, dùng trạng thái tổng quát có thật; không chạy thanh phần trăm theo timer. Sau thành công, nói rõ cửa hàng đã cài xong; chỉ nói hệ thống sẵn sàng khi live application health đạt.

### Lần sau

```text
Mở app → Đọc installation → Tổng quan: Đang khởi động
       → Hệ thống sẵn sàng → Mở bán hàng → Đăng nhập/POS
```

Auto-start chỉ có từ 5.5. Trước đó màn hình vận hành cho phép Khởi động thủ công. Reload UI không tự tạo provisioning, reset credentials hoặc spawn trùng. Khi cửa hàng đã cài thì không quay lại form fresh setup chỉ vì runtime đang dừng.

### Khi lỗi hoặc bị gián đoạn

```text
Setup lỗi/đóng giữa chừng → Mở lại → Native xác định checkpoint
  → có thể tiếp tục: thông báo + Tiếp tục thiết lập
  → không thể retry: giữ dữ liệu + hướng xử lý + Xem chi tiết
```

Không bắt đầu lại từ đầu hoặc xóa store để che lỗi. Lỗi input hiện tại field; lỗi setup ở bước đang làm; lỗi runtime ở **Tổng quan** kèm liên kết tới **Hệ thống → Chẩn đoán**. Một lỗi không tự biến thành màn hình trắng toàn app. Repair chỉ xuất hiện khi operation đó đã được triển khai.

## 4. Tổng quan và trạng thái

| Trạng thái đã xác minh | Nội dung chính | Hành động |
| --- | --- | --- |
| Chưa cài | Cần thiết lập cửa hàng | Thiết lập cửa hàng |
| Setup gián đoạn, retryable | Thiết lập chưa hoàn tất | Tiếp tục thiết lập |
| Đã cài, stopped | Hệ thống đang dừng | Khởi động |
| Starting | Đang khởi động cửa hàng | Hiển thị chờ, chặn submit trùng |
| Runtime chạy, health đang kiểm | Đang kiểm tra cửa hàng | Chưa mở bán hàng |
| Application healthy | Hệ thống sẵn sàng | Mở bán hàng, từ 5.4 |
| Health/runtime lỗi | Không thể mở bán hàng + lý do ngắn | Thử lại nếu native cho phép; Xem chi tiết |
| Stopping | Đang dừng hệ thống | Chặn start/open xung đột |

Trước 5.4, trạng thái healthy chỉ thông báo kết quả, không có nút Mở bán hàng giả. Application healthy không đồng nghĩa đã đăng nhập, mở ca hoặc thanh toán thành công. Shell không suy đoán các trạng thái nghiệp vụ này.

Tổng quan không hiển thị thường trực PHP version, database port, manifest hash hay đường dẫn nội bộ. Thông tin đó ở **Hệ thống → Chẩn đoán/Chi tiết kỹ thuật**. Start/stop/restart là thao tác vận hành có chủ đích trong Hệ thống, không phải ba nút nổi bật ngang hàng với **Mở bán hàng**.

## 5. Thông tin cửa hàng và tài khoản

Phase 5.2 phải viết contract input/native/persistence trước code. Dùng tên cửa hàng và tài khoản quản trị ban đầu với tối thiểu field mà WordPress/plugin thực sự cần. Thông báo validation rõ ràng; có thể quay lại sửa trước khi bắt đầu mutation. Không yêu cầu người dùng cấu hình DB hoặc vào wp-admin để hoàn tất setup.

Chọn cách user đặt credential ban đầu hoặc nhận credential được sinh an toàn; contract phải giải quyết cả store mới và store 4.11 đã có generated secret. Không reset password của store cũ để đưa vào wizard mới. Trạng thái tạo/nhận credential phải phục hồi được khi app đóng giữa chừng. Không lưu password ở localStorage, draft JSON, URL hoặc logs; dữ liệu nhạy cảm chỉ qua cơ chế native được giới hạn phù hợp. Không tự dùng machine token để đăng nhập nhân viên.

WordPress/plugin là nguồn sự thật cho thông tin cửa hàng sau provisioning. `store_name` trong app config không trở thành một bản tên cửa hàng độc lập bị lệch với WordPress. Đổi thông tin nghiệp vụ sau setup đi qua chức năng của plugin.

## 6. Mở POS: quyết định riêng với thiết kế shell

Shell phải dùng được dù POS mở bằng trình duyệt hay WebView. Hướng đề xuất cho MVP là trình duyệt hệ thống; chưa coi lựa chọn host là đã chốt chỉ vì cập nhật docs này. Trước code 5.4 phải ghi quyết định host, hành vi đóng cửa sổ, session và acceptance trong spec phase. POS WebView không còn là prerequisite mặc định của thiết kế màn hình.

Nếu dùng trình duyệt: nút Mở bán hàng lấy route thật từ plugin, native resolve URL theo runtime origin đã kiểm chứng. Không nhận URL tùy ý. Đóng tab không dừng server; Desktop không tự biết người dùng đã đóng tab hay đăng nhập. Không báo “đã mở POS” chỉ từ việc giao lệnh mở URL; dùng thông báo “Đã yêu cầu mở trình duyệt” và cho phép thử lại nếu cần.

Nếu dùng WebView: POS ở WebView riêng, không có management IPC; kiểm navigation, external links, cookie/session và quay về shell. Không nạp WordPress vào WebView shell có quyền native. Không cần triển khai cả hai host trong cùng milestone.

Cả hai lựa chọn đều phải nghiệm thu login → POS → đơn test → logout → login lại, session hết hạn, restart/đổi port và lỗi runtime. Không làm lại form login nghiệp vụ trong shell khi auth WordPress/CoffeePOS đã có.

## 7. Khởi động, thu nhỏ và thoát

Phase 5.5 tự khởi động runtime khi mở app trên store đã cài; không tự bật app cùng Windows trong scope này. Không tự mở thêm tab POS mỗi lần refresh/health poll.

Baseline 5.6 Windows-first dùng system tray cho Minimize: bấm **—** ẩn shell khỏi taskbar nhưng giữ runtime hoạt động. Close/Alt+F4 giữ shutdown UX cũ và dùng cùng path với tray **Thoát hoàn toàn**: khi runtime active, Windows confirmation xuất hiện; chọn thoát mới bật admission gate, bounded-drain request đã được nhận, stop runtime rồi authorize app exit. Tray vẫn có **Mở CoffeePOS** và **Thoát hoàn toàn**. Lifecycle đang bận hoặc forced cleanup còn child thì shell được giữ/mở lại để người dùng xử lý.

Đóng app không thay chốt ca. Khi dùng browser, không khẳng định mọi phiên bán hàng đã kết thúc vì shell không quan sát được tab. Bounded shutdown, request đang chạy và crash recovery thuộc native/plugin contracts; UI phản ánh thật, không hứa chống mất điện hoàn toàn.

## 8. Quy tắc trình bày và tương tác

Trong **Hệ thống → Chẩn đoán**, Phase 6.1 hiển thị Database/PHP/WordPress/WooCommerce/CoffeePOS thành các dòng health độc lập. Chỉ gán **Có lỗi** khi native probe hoặc authenticated machine-health có bằng chứng cho đúng component; dependency chưa xác minh giữ **Chưa xác minh**. **Kiểm tra lại** chạy snapshot health thật, còn restart runtime là action riêng. Phase 6.2 không tạo thêm màn hình top-level; nó làm status/lifecycle/health phản hồi mượt hơn ở phía runtime. Port, version, path và structured error chi tiết được gom dưới **Chi tiết kỹ thuật**; repair và log export không xuất hiện trước Phase 6.3–6.4.

Trong **Hệ thống → Sửa chữa**, mở tab chỉ hiển thị trạng thái **Chưa kiểm tra**. Không tự chạy get_repair_plan khi vào tab, đổi tab, bootstrap hoặc route vào needs_repair; người dùng phải bấm **Kiểm tra** trước. Chỉ khi có repair plan mới nhất và can_apply=true thì nút **Sửa chữa** mới được bật. Plan stale hoặc apply lỗi yêu cầu người dùng bấm **Kiểm tra lại**; app không tự inspect/apply lại.

- Một hành động chính cho mỗi bước/trạng thái; nhãn dùng từ của người vận hành: Cài đặt, Tiếp tục thiết lập, Mở bán hàng, Xem chi tiết.
- Layout có tiêu đề, mô tả ngắn, vùng nội dung và action rõ ràng. Shell dùng cùng typography, spacing, max-width, padding và button/input/error styles; không thêm UI framework chỉ để chia màn hình.
- Navigation cấp cao sau setup chỉ dùng **Tổng quan / Cấu hình / Hệ thống**. Chẩn đoán và công cụ kỹ thuật dùng navigation cấp hai trong Hệ thống. Wizard có thứ tự bước trước setup. Lỗi/chờ là state của màn hình, không bắt buộc tạo route riêng cho mọi state.
- Không lặp hierarchy kiểu eyebrow → page title → card title khi các nhãn cùng nghĩa. Mỗi màn hình có một page heading chính; card/section heading chỉ dùng khi thực sự chia nội dung.
- Có focus bàn phím rõ; Tab/Enter hoạt động; chuyển màn hình đưa focus tới tiêu đề phù hợp, lỗi form tới field liên quan. Progress dùng thông báo accessible, không đọc lặp mỗi poll.
- Không chỉ dùng màu để báo lỗi/thành công. Text phải nhất quán theo locale Desktop đã chọn (`vi` hoặc `en`), không cắt nội dung quan trọng; resize và DPI/zoom phải giữ được nút chính, cho cuộn khi cần.
- Operation đang chạy không bị nhân đôi bởi double click, Back hoặc reload. Draft không nhạy cảm được giữ khi điều hướng hợp lệ; secret phải có quy tắc vòng đời riêng.

Trước triển khai mỗi màn hình, spec phase phải có wireframe cho trạng thái chính/lỗi/chờ và danh sách action thật. Dùng thiết kế đó để review bố cục trước code; tài liệu này chưa chốt palette/font hay mockup hình ảnh cuối cùng.

## 9. UX đi cùng tính năng tương lai

| Nhóm | Phần UX phải nghiệm thu cùng backend |
| --- | --- |
| 6.x Hệ thống / Diagnostics / Repair / Logs | Hệ thống là khu vực cấp cao; Chẩn đoán tóm tắt dễ hiểu, chi tiết kỹ thuật mở khi cần; repair giải thích phạm vi/kết quả; Nhật ký có source selector + bounded viewer, export có trạng thái, native Save As và redaction |
| 7.x Hệ thống / Backup / Restore | Địa điểm lưu, tiến trình, thành công/lỗi; validate bản restore và giải thích dữ liệu sẽ thay trước xác nhận; lỗi giữ đường phục hồi |
| 7.5 Desktop localization | Fresh profile chọn Tiếng Việt/English trước Welcome; Cấu hình cho đổi locale và hot-switch shell; navigation, states, action/error copy, tray/close và accessibility text theo locale; target profile giữ language riêng qua backup/restore |
| 8.x LAN | Mặc định tắt; Cấu hình → Mạng nội bộ cho bật/tắt, chọn adapter, exact HTTPS URL có thể copy, export public trust certificate/fingerprint, lỗi network/firewall, cảnh báo session khi đổi canonical address và fail-closed khi network không còn trusted. Xem [8.1](PHASE-08.1.md), [8.2](PHASE-08.2.md), [8.3](PHASE-08.3.md) |
| 9.x Distribution/Update | Setup trên máy sạch, thông báo prerequisite; update tiến trình/lỗi và dữ liệu được giữ; không bắt người dùng chạy lệnh |

## 10. Nghiệm thu UX

Mỗi milestone ghi riêng implementation, test/build, native acceptance và giới hạn. Ảnh/mockup không thay việc bấm app thật. Với UI thay đổi, lưu bằng chứng màn hình và kết quả flow cho success/loading/error/retry, bàn phím, resize/DPI và reload/relaunch phù hợp scope.

Cuối Phase 5, một lượt nghiệm thu theo kịch bản người vận hành phải hoàn thành: thiết lập cửa hàng mới → nhận/đặt tài khoản → mở bán hàng/đăng nhập → tạo đơn test → đóng/mở app → dùng tiếp store cũ → gặp lỗi an toàn → biết cách thử lại hoặc xem chi tiết. Người thử không cần terminal, wp-admin hay hiểu PHP/database. Ghi rõ ai thực hiện và mức hỗ trợ; chưa có thử nghiệm với người dùng không chuyên thì không tuyên bố đã kiểm chứng usability với nhóm đó.
