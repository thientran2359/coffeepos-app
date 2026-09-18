# Phase 6.1 — Health diagnostics

Phase 6.1 biến trang Chẩn đoán từ bảng kỹ thuật của Phase 5.1 thành màn hình sức khỏe theo component. Nguồn sự thật vẫn là Runtime Manager và CoffeePOS machine-health hiện có; Desktop không tạo một probe thứ hai hoặc suy đoán dependency từ việc endpoint biến mất.

> **Product IA revision (2026-09-18):** implementation snapshot của Phase 6.1 ban đầu đặt Chẩn đoán ở top-level navigation. Target sản phẩm hiện tại đặt màn hình này tại **Hệ thống → Chẩn đoán** dưới ba mục cấp cao **Tổng quan / Cấu hình / Hệ thống**. Native diagnostics contract, health classification và recovery semantics của phase này giữ nguyên; manual UX acceptance sau refactor dùng hierarchy mới. Xem [UI-UX.md](UI-UX.md).

## Phạm vi

- Hiển thị riêng **Database / PHP / WordPress / WooCommerce / CoffeePOS** với trạng thái dễ hiểu, mô tả ngắn và hướng phục hồi.
- Database/PHP dùng lifecycle + managed process/readiness đã được Runtime Manager xác minh. Lỗi start/monitor có `RuntimeErrorInfo.component` được gắn đúng component.
- WordPress dùng `wordpress_health` + `wordpress_error` hiện có.
- WooCommerce/CoffeePOS chỉ kết luận healthy/unhealthy theo authenticated machine-health payload khi payload hợp lệ. Khi machine-health transport/auth/contract fail, dependency không có payload giữ trạng thái **Chưa xác minh** thay vì bị gán lỗi giả.
- Nút **Kiểm tra lại** gọi snapshot diagnostics native mới; không provisioning, reinstall, reset credential hoặc repair file/config.
- Start/stop/restart vẫn ở card Runtime. Port, PID/path, version và lỗi kỹ thuật nằm trong phần chi tiết để màn hình chính dễ đọc.
- Tối ưu responsiveness/concurrency runtime thuộc Phase 6.2. Repair tự động thuộc Phase 6.3. Xem/export log thuộc Phase 6.4.

## Wireframe

### Healthy

```text
Hệ thống > Chẩn đoán

Sức khỏe thành phần                         Sẵn sàng
Database      Khỏe     MariaDB đang chạy và đã qua readiness.
PHP           Khỏe     PHP HTTP runtime đang chạy.
WordPress     Khỏe     WordPress trả readiness hợp lệ.
WooCommerce   Khỏe     Machine-health xác nhận dependency sẵn sàng.
CoffeePOS     Khỏe     Machine-health xác nhận ứng dụng sẵn sàng.

[ Kiểm tra lại ]
```

### Checking / runtime transition

```text
Sức khỏe thành phần                         Đang kiểm tra
Database      Đang kiểm tra
PHP           Đang kiểm tra
WordPress     Đang kiểm tra
WooCommerce   Chưa xác minh
CoffeePOS     Chưa xác minh

[ Kiểm tra lại ] disabled
```

### Component failure

```text
Sức khỏe thành phần                         Cần xử lý
Database      Khỏe
PHP           Khỏe
WordPress     Khỏe
WooCommerce   Có lỗi
              CoffeePOS machine-health xác nhận WooCommerce chưa sẵn sàng.
              Hướng xử lý: Kiểm tra lại; nếu vẫn lỗi, khởi động lại runtime.
CoffeePOS     Khỏe

[ Kiểm tra lại ]
```

Nếu machine-health itself thất bại do transport/auth/contract, WooCommerce giữ **Chưa xác minh**; CoffeePOS hiển thị **Chưa xác minh** cùng structured native error/recovery. Endpoint mất không được dùng làm bằng chứng rằng riêng WooCommerce bị lỗi.

## Native contract

Phase 6.1 tái sử dụng toàn bộ probe hiện có và thêm một command chỉ để gom snapshot chẩn đoán:

- `get_runtime_info()` trả lifecycle state, PID/ports/version, `wordpress_health`, `wordpress_error`, `coffeepos_health`, `last_error`.
- `retry_runtime_health()` chỉ hợp lệ khi runtime `running`; rerun WordPress readiness rồi CoffeePOS machine-health.
- `get_health_diagnostics()` được serialize bằng lifecycle lock, refresh process state rồi chạy one-shot authenticated MariaDB probe, nonce PHP HTTP probe, WordPress readiness và CoffeePOS machine-health. Command chỉ đọc/kiểm tra health; probe PHP tạm luôn đi qua cleanup path và không chạy đồng thời với start/stop/restart/shutdown.
- `CoffeePosHealthPayload` schema 1 cung cấp booleans `database`, `wordpress`, `woocommerce`, `coffeepos` cùng version/store/POS route khi endpoint được authenticate và validate.
- `RuntimeErrorInfo { component, operation, message, recovery }` giữ lỗi có hành động phục hồi. Frontend không parse message để đoán component; chỉ dùng `component` và structured health state.

Phân loại UI phải giữ các invariant:

- `running + managed PID` chứng minh Database/PHP đã qua startup readiness; machine-health `database=false` có thể hạ Database thành lỗi application-level.
- WordPress `unhealthy` là lỗi WordPress; CoffeePOS payload chưa có thì WooCommerce/CoffeePOS chưa được xác minh.
- Machine-health `degraded` + payload hợp lệ cho phép gán lỗi đúng boolean component.
- Machine-health `failed` không tự biến mọi dependency thành lỗi; giữ unknown/unverified cho phần không có bằng chứng.
- Runtime stopped làm các component stopped/unavailable, trừ component có structured `last_error` trực tiếp từ lần start/monitor gần nhất thì component đó vẫn được chỉ ra là lỗi cần xử lý.

## Acceptance

1. Runtime healthy hiển thị đủ 5 component healthy và summary **Sẵn sàng**; thông tin kỹ thuật vẫn xem được nhưng không lấn phần tóm tắt.
2. Start/stop/restart/health-retry chuyển ngay sang state chờ, khóa **Kiểm tra lại** trong lúc lifecycle command đang chạy và không giữ healthy stale.
3. Database hoặc PHP start/monitor failure được chỉ đúng component bằng `RuntimeErrorInfo.component`, giữ message/recovery có hành động.
4. WordPress unhealthy chỉ WordPress lỗi; WooCommerce/CoffeePOS chưa có authenticated payload hiển thị **Chưa xác minh**, không suy diễn lỗi dependency.
5. Machine-health `degraded` hiển thị đúng component boolean false, ví dụ WooCommerce false không làm CoffeePOS/database bị gán lỗi nếu các boolean đó true.
6. Machine-health auth/transport/contract failure hiển thị probe error/recovery và giữ dependency chưa xác minh; không rotate credential hoặc gọi repair.
7. **Kiểm tra lại** gọi native diagnostics snapshot thật, không provisioning; **Khởi động lại** vẫn là lifecycle action riêng.
8. Poll 2 giây không tạo live-region announcement lặp khi text không đổi; bàn phím, viewport hẹp và zoom vẫn truy cập được health rows/actions.

## Validation hiện tại

Implementation snapshot ngày 2026-09-18 đã pass bộ kiểm tra gọn theo yêu cầu của người dùng:

```text
git diff --check — PASS
npm run lint:ui — PASS
npm run build:ui — PASS (7 modules)
cargo fmt --manifest-path .\src-tauri\Cargo.toml -- --check — PASS
cargo test --manifest-path .\src-tauri\Cargo.toml --locked health_diagnostics — PASS (1 focused test)
cargo test --manifest-path .\src-tauri\Cargo.toml --locked machine_component_health_only_marks_the_reported_component_unhealthy — PASS (1 focused test)
cargo test --manifest-path .\src-tauri\Cargo.toml --locked coffeepos_health_parser_accepts_healthy_and_degraded_contracts — PASS (1 existing contract regression)
cargo test --manifest-path .\src-tauri\Cargo.toml --locked request_drain_gate_marker_is_recoverable_across_startup — PASS (1 Phase 5.6 regression)
```

Rust checks dùng workspace-local toolchain trong `.tools/cargo` + `.tools/rustup` vì shell hiện tại không có Cargo trên `PATH`. Focused Rust compile đi qua Tauri build script, nên command manifest và permission `allow-get-health-diagnostics` cũng được validate.

Manual Windows acceptance còn chờ người dùng: mở **Hệ thống → Chẩn đoán** trên store healthy; stop/start/recheck; inject một machine-health degraded component để xác nhận chỉ component đó báo lỗi; kiểm auth/transport/contract failure giữ dependency chưa xác minh; và smoke-test keyboard + viewport hẹp. Acceptance UI dùng hierarchy mới **Tổng quan / Cấu hình / Hệ thống**. Không chạy staged/full provisioning E2E dài cho phase này.

## Ngoài phạm vi

- Runtime performance, OPcache, async lifecycle và concurrent serving: Phase 6.2.
- Tự repair file/config/plugin/credential: Phase 6.3.
- Log viewer, redaction/export support bundle: Phase 6.4.
- LAN reachability/device health: Phase 8.x.
