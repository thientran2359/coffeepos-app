# Phase 5.6 — Thu nhỏ, thoát và shutdown

Ngày triển khai: 2026-09-18.

## Mục tiêu

Phase 5.6 chốt vòng đời cửa sổ và runtime cho baseline Windows-first:

- Thu nhỏ cửa sổ chỉ thu nhỏ UI; runtime tiếp tục phục vụ POS.
- Đóng cửa sổ chính hoặc Alt+F4 đi qua native close interception.
- Nếu runtime còn active, người dùng được cảnh báo rằng POS/thiết bị sẽ mất kết nối và có thể chọn ở lại.
- Chỉ cho app thoát sau khi runtime đã dừng hoặc native xác nhận không còn managed process.
- Đóng app không chốt ca, không logout browser session và không tự thay trạng thái thanh toán.
- Tray/background mode chưa thuộc scope.

## Close contract

Tauri xử lý CloseRequested của cửa sổ main:

1. Nếu exit đã được native authorize thì cho event đi tiếp.
2. Nếu chưa, native prevent_close trước khi làm bất kỳ shutdown nào.
3. Native dùng lifecycle mutex hiện có; nếu provisioning/start/stop/restart đang chạy thì từ chối đóng và yêu cầu chờ.
4. Nếu RuntimeManager đang active, hiển thị native Windows confirmation:
   - Dừng cửa hàng và thoát.
   - Ở lại.
5. Nếu chọn ở lại, không đổi runtime/process/state.
6. Nếu chọn thoát, runtime stop chạy dưới cùng lifecycle guard.
7. Chỉ gọi app exit khi không còn managed runtime process.
8. Nếu forced cleanup vẫn để process sống, cửa sổ ở lại và báo lỗi.

Runtime stopped/not-installed không cần confirmation; close vẫn đi qua native cleanup path trước khi authorize exit.

## Minimize

Minimize dùng behavior mặc định của cửa sổ Tauri/Windows. Không gọi stop_runtime, không đổi provisioning state và không mở/đóng browser tab. Runtime health polling vẫn tiếp tục khi WebView còn hoạt động.

## Bounded request drain

Trước khi terminate PHP trong RuntimeManager::stop:

- runtime bật request-admission marker trước khi chuyển sang stopping;
- PHP luôn chạy qua runtime router wrapper do Desktop quản lý; khi marker bật, request mới nhận HTTP 503 trước khi được delegate sang WordPress router;
- chỉ drain probe tạm do Desktop tạo được phép bypass admission gate;
- cron managed process được dừng trước;
- runtime tạo một probe PHP tạm trong site;
- tối đa 3 giây chờ PHP xử lý probe;
- với PHP built-in server single-process hiện tại, probe chỉ trả về sau các request đã đứng trước nó trong hàng xử lý;
- nếu probe hoàn tất, runtime log php request drain complete;
- request đến sau marker không vào WordPress/CoffeePOS nên không bắt đầu mutation mới trong drain window;
- nếu hết thời gian hoặc probe không dùng được, runtime chờ đủ drain budget rồi tiếp tục forced-stop path hiện có;
- MariaDB sau đó vẫn nhận SHUTDOWN command có timeout trước khi forced termination.

Marker chỉ được xóa khi managed PHP đã dừng. Nếu forced terminate thất bại và PHP còn sống, marker được giữ để process còn sót tiếp tục trả 503 thay vì nhận mutation mới trong trạng thái shutdown dở. Startup luôn xóa marker stale trước khi spawn PHP để crash giữa shutdown không khóa store vĩnh viễn. Runtime wrapper delegate sang managed wordpress-router.php hiện có nên store cũ không cần provisioning/reinstall để nhận gate mới.

Đây là bounded admission gate + best-effort drain, không phải cam kết mọi request hoặc giao dịch luôn hoàn tất trong mất điện, OS kill hay request treo quá deadline.

## Crash containment

Windows RuntimeManager đã đặt managed child vào kill-on-close Job Object. Nếu desktop process chết bất thường và không chạy close flow, Windows đóng job handle khi process thoát và kết thúc child thuộc job, tránh process mồ côi.

Crash/kill không được mô tả là graceful shutdown. Database recovery sau abrupt termination vẫn thuộc MariaDB/WordPress và acceptance/recovery thực tế.

## UX

Confirmation Windows nói rõ:

- dừng và thoát làm POS/thiết bị đang kết nối mất kết nối;
- thao tác không chốt ca;
- thao tác không tự đổi payment state;
- lựa chọn mặc định an toàn là ở lại.

Nếu lifecycle đang bận hoặc stop chưa hoàn tất, Desktop giữ cửa sổ mở thay vì thoát nửa chừng.

## Acceptance

1. Runtime healthy → minimize → PHP/MariaDB PID giữ nguyên, POS vẫn truy cập được.
2. Runtime healthy → Close/Alt+F4 → confirmation xuất hiện; chọn ở lại giữ PID và Home state.
3. Runtime healthy → chọn dừng và thoát → bounded drain chạy, runtime dừng rồi Desktop thoát.
4. Relaunch sau clean exit → Phase 5.5 auto-start store cũ, không reinstall và không reset browser auth/session.
5. Runtime stopped → Close không hỏi cảnh báo disconnect không cần thiết.
6. Provision/start/restart/stop đang giữ lifecycle → Close bị chặn, không kill operation giữa chừng.
7. Stop failure còn managed child → app ở lại và hiển thị lỗi; retry close có thể thử shutdown lại.
8. Request test đang chạy trước probe → shutdown đợi trong request_drain budget; timeout thì forced cleanup có log rõ.
9. Sau khi drain marker bật, request mới nhận 503 và không vào WordPress/CoffeePOS; probe nội bộ vẫn 200 để xác nhận pre-existing queue đã qua.
10. Kill/crash desktop trên Windows không để managed PHP/MariaDB/cron mồ côi nhờ Job Object.
11. Đóng browser tab POS không ảnh hưởng runtime; đóng Desktop không được coi là chốt ca.
12. Recovery/relaunch không tự tạo đơn mới hoặc chạy provisioning lại; stale drain marker được clear trước PHP start.

## Validation hiện tại

Theo preference kiểm thử gọn:

~~~text
git diff --check — PASS
npm run lint:ui — PASS
npm run build:ui — PASS (7 modules)
cargo fmt --manifest-path .\src-tauri\Cargo.toml -- --check — PASS
cargo test --manifest-path .\src-tauri\Cargo.toml --locked runtime::tests::exit_confirmation_tracks_active_runtime_states — PASS
cargo test --manifest-path .\src-tauri\Cargo.toml --locked runtime::tests::request_drain_gate_marker_is_recoverable_across_startup — PASS
cargo test --manifest-path .\src-tauri\Cargo.toml --locked runtime::tests::failed_child_start_is_cleaned_and_returns_to_stopped — PASS
staged PHP admission gate: normal 200 → marker-on 503, drain probe remains 200 — PASS
~~~

Native minimize/Close/Alt+F4, in-flight POS request/order và crash containment vẫn là manual acceptance trên disposable store.

## Ngoài scope

- Tray/background service.
- Auto-start cùng Windows login.
- Graceful guarantee khi mất điện/Task Manager hard kill.
- Chốt ca hoặc business transaction orchestration.
- Repair UI/log export đầy đủ: Phase 6.
