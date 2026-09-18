# CoffeePOS Desktop — Đặc tả UI/UX

Ngày cập nhật: 2026-09-18. Đây là đặc tả trải nghiệm; bằng chứng triển khai nằm trong tài liệu từng phase. Phase 1–4.12 giữ nguyên kết quả kỹ thuật đã nghiệm thu; Phase 5.1 shell/navigation và Phase 5.2 store/account onboarding đã hoàn thành Windows-first. Phase 5.3 Home/app settings đã triển khai và pass lightweight automated checks, còn manual acceptance do người dùng thực hiện. Scope, thứ tự và trạng thái phase do [ROADMAP.md](ROADMAP.md) quản lý.

## 1. Mục tiêu

Người vận hành biết mình cần làm gì tiếp theo mà không phải hiểu WordPress, PHP, database, port hoặc health token. Desktop quản lý môi trường; giao diện bán hàng và nghiệp vụ vẫn do CoffeePOS cung cấp. Không xây dashboard doanh thu, giỏ hàng hoặc quản lý nhân viên lần nữa trong shell.

Giao diện hiện tại là công cụ development. Việc gom setup, runtime controls và cấu hình trên một trang không phải thiết kế sản phẩm cuối cùng. UI/UX phải được triển khai cùng các luồng thật, không để đến cuối dự án mới trang trí.

## 2. Cấu trúc màn hình

| Màn hình | Nội dung và hành động chính | Mốc triển khai |
| --- | --- | --- |
| Chào mừng | Giải thích ngắn: cài một lần, dữ liệu nằm trên máy; nút **Thiết lập cửa hàng** | 4.12: bắt đầu setup tối thiểu; 5.1–5.2: hoàn thiện |
| Thiết lập cửa hàng | Tên cửa hàng, tài khoản ban đầu, validation; nút **Cài đặt** | 5.2 |
| Tiến trình thiết lập | Các bước thật đã xong/đang chạy; lỗi và tiếp tục khi an toàn | 4.12, tích hợp form ở 5.2 |
| Trang chính | Tên cửa hàng, trạng thái sử dụng, một hành động chính phù hợp trạng thái | 5.3 |
| Cài đặt ứng dụng | Cấu hình thuộc Desktop, trạng thái lưu và lỗi; không nhân bản settings nghiệp vụ | 5.3; LAN bổ sung ở 8.x |
| Chẩn đoán | Component health, thông tin kỹ thuật, thao tác runtime; log/repair mở rộng sau | Khung cơ bản 5.1; đầy đủ 6.x |
| Sao lưu và khôi phục | Tạo backup, thời điểm/kết quả, chọn và kiểm bản phục hồi | 7.x |

Trước setup, dùng luồng theo bước, không đưa người dùng vào dashboard chưa có cửa hàng. Sau setup, điều hướng ổn định giữa Trang chính, Cài đặt và Chẩn đoán; mục Sao lưu chỉ xuất hiện khi chức năng có thật. Không tạo trang rỗng hoặc nút hoạt động giả cho phase tương lai.

## 3. Luồng chính

### Lần đầu

```text
Chào mừng → Thông tin cửa hàng/tài khoản → Cài đặt
         → Tiến trình thật → Hoàn tất → Trang chính → Mở bán hàng
```

Phase 4.12 hoàn thiện tiến trình/recovery trên input setup cũ; Phase 5.2 đã bổ sung form tài khoản, review/complete, protected user-set credential và nghiệm thu fresh flow đầy đủ. Không kéo các hành vi Phase 5.4 như mở POS vào onboarding.

Màn hình tiến trình lấy trạng thái từ native. Nếu chưa có sự kiện từng bước, dùng trạng thái tổng quát có thật; không chạy thanh phần trăm theo timer. Sau thành công, nói rõ cửa hàng đã cài xong; chỉ nói hệ thống sẵn sàng khi live application health đạt.

### Lần sau

```text
Mở app → Đọc installation → Trang chính: Đang khởi động
       → Hệ thống sẵn sàng → Mở bán hàng → Đăng nhập/POS
```

Auto-start chỉ có từ 5.5. Trước đó trang chính cho phép Khởi động thủ công. Reload UI không tự tạo provisioning, reset credentials hoặc spawn trùng. Khi cửa hàng đã cài thì không quay lại form fresh setup chỉ vì runtime đang dừng.

### Khi lỗi hoặc bị gián đoạn

```text
Setup lỗi/đóng giữa chừng → Mở lại → Native xác định checkpoint
  → có thể tiếp tục: thông báo + Tiếp tục thiết lập
  → không thể retry: giữ dữ liệu + hướng xử lý + Xem chi tiết
```

Không bắt đầu lại từ đầu hoặc xóa store để che lỗi. Lỗi input hiện tại field; lỗi setup ở bước đang làm; lỗi runtime ở trang chính kèm liên kết chẩn đoán. Một lỗi không tự biến thành màn hình trắng toàn app. Repair chỉ xuất hiện khi operation đó đã được triển khai.

## 4. Trang chính và trạng thái

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

Trang chính không hiển thị thường trực PHP version, database port, manifest hash hay đường dẫn nội bộ. Thông tin đó ở Chẩn đoán. Start/stop/restart là thao tác vận hành có chủ đích, không phải ba nút nổi bật ngang hàng với Mở bán hàng.

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

Baseline 5.6: thu nhỏ giữ runtime hoạt động; thoát app dừng runtime. Khi thoát/dừng hệ thống đang chạy, giải thích rằng POS và thiết bị đang kết nối sẽ mất kết nối, cho lựa chọn ở lại hoặc dừng và thoát. Không dùng xác nhận cho mọi thao tác điều hướng thông thường. Nếu bổ sung tray/background mode, phải đổi contract và kiểm ownership/process cleanup; chưa coi đó là tính năng đã có.

Đóng app không thay chốt ca. Khi dùng browser, không khẳng định mọi phiên bán hàng đã kết thúc vì shell không quan sát được tab. Bounded shutdown, request đang chạy và crash recovery thuộc native/plugin contracts; UI phản ánh thật, không hứa chống mất điện hoàn toàn.

## 8. Quy tắc trình bày và tương tác

- Một hành động chính cho mỗi bước/trạng thái; nhãn dùng từ của người vận hành: Cài đặt, Tiếp tục thiết lập, Mở bán hàng, Xem chi tiết.
- Layout có tiêu đề, mô tả ngắn, vùng nội dung và action rõ ràng. Shell dùng cùng typography, spacing, button/input/error styles; không thêm UI framework chỉ để chia màn hình.
- Dùng navigation gọn cho app sau setup; wizard có thứ tự bước trước setup. Lỗi/chờ là state của màn hình, không bắt buộc tạo route riêng cho mọi state.
- Có focus bàn phím rõ; Tab/Enter hoạt động; chuyển màn hình đưa focus tới tiêu đề phù hợp, lỗi form tới field liên quan. Progress dùng thông báo accessible, không đọc lặp mỗi poll.
- Không chỉ dùng màu để báo lỗi/thành công. Text tiếng Việt nhất quán, không cắt nội dung quan trọng; resize và DPI/zoom phải giữ được nút chính, cho cuộn khi cần.
- Operation đang chạy không bị nhân đôi bởi double click, Back hoặc reload. Draft không nhạy cảm được giữ khi điều hướng hợp lệ; secret phải có quy tắc vòng đời riêng.

Trước triển khai mỗi màn hình, spec phase phải có wireframe cho trạng thái chính/lỗi/chờ và danh sách action thật. Dùng thiết kế đó để review bố cục trước code; tài liệu này chưa chốt palette/font hay mockup hình ảnh cuối cùng.

## 9. UX đi cùng tính năng tương lai

| Nhóm | Phần UX phải nghiệm thu cùng backend |
| --- | --- |
| 6.x Diagnostics/Repair | Tóm tắt dễ hiểu, chi tiết kỹ thuật mở khi cần; repair giải thích phạm vi/kết quả; export log có trạng thái và redaction |
| 7.x Backup/Restore | Địa điểm lưu, tiến trình, thành công/lỗi; validate bản restore và giải thích dữ liệu sẽ thay trước xác nhận; lỗi giữ đường phục hồi |
| 8.x LAN | Mặc định tắt; bật/tắt rõ ràng, URL thực có thể copy, lỗi network/firewall, ảnh hưởng khi đổi địa chỉ hoặc dừng server |
| 9.x Distribution/Update | Setup trên máy sạch, thông báo prerequisite; update tiến trình/lỗi và dữ liệu được giữ; không bắt người dùng chạy lệnh |

## 10. Nghiệm thu UX

Mỗi milestone ghi riêng implementation, test/build, native acceptance và giới hạn. Ảnh/mockup không thay việc bấm app thật. Với UI thay đổi, lưu bằng chứng màn hình và kết quả flow cho success/loading/error/retry, bàn phím, resize/DPI và reload/relaunch phù hợp scope.

Cuối Phase 5, một lượt nghiệm thu theo kịch bản người vận hành phải hoàn thành: thiết lập cửa hàng mới → nhận/đặt tài khoản → mở bán hàng/đăng nhập → tạo đơn test → đóng/mở app → dùng tiếp store cũ → gặp lỗi an toàn → biết cách thử lại hoặc xem chi tiết. Người thử không cần terminal, wp-admin hay hiểu PHP/database. Ghi rõ ai thực hiện và mức hỗ trợ; chưa có thử nghiệm với người dùng không chuyên thì không tuyên bố đã kiểm chứng usability với nhóm đó.
