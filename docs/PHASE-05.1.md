# Phase 5.1 — Khung giao diện và điều hướng

Phase 5.1 chuyển CoffeePOS Desktop từ trang công cụ development của Phase 4.x sang shell ứng dụng có cấu trúc rõ ràng, nhưng giữ nguyên toàn bộ provisioning/recovery contract của Phase 4.12. Milestone này chỉ tổ chức lại UI quanh các native command đã có; không thêm onboarding tài khoản, POS host, auto-start hay repair engine.

> **Product IA revision (2026-09-18, sau Phase 6.1):** phần dưới ghi lại đúng shell đã được nghiệm thu ở Phase 5.1 với **Trang chính / Cài đặt / Chẩn đoán**. Target UI hiện tại đã được đổi thành **Tổng quan / Cấu hình / Hệ thống**, trong đó **Chẩn đoán** là chức năng cấp hai trong **Hệ thống**. Các invariant đã nghiệm thu ở phase này — navigation không gọi lifecycle, focus bàn phím, responsive/zoom và không respawn runtime — tiếp tục bắt buộc. Xem [UI-UX.md](UI-UX.md).

## Phạm vi

- Store chưa cài hoặc provisioning chưa hoàn tất ở trong luồng **Thiết lập cửa hàng** riêng, không thấy navigation của store đã cài.
- Store `ready` vào shell có ba mục thật: **Trang chính**, **Cài đặt**, **Chẩn đoán**.
- Trang chính chỉ tóm tắt trạng thái đã xác minh và hướng người vận hành sang Chẩn đoán khi cần; không có nút Mở bán hàng trước Phase 5.4.
- Setup giữ input `store_name` + feedback save/error hiện có vì native provisioning vẫn dùng giá trị này trước Phase 5.2.
- Cài đặt sau setup chỉ hiển thị cấu hình Desktop có thật ở mức read-only trong Phase 5.1; không dùng `store_name` như một tên cửa hàng thứ hai sau provisioning.
- Chẩn đoán chứa runtime controls, WordPress/CoffeePOS health, ports, versions và development action mở WordPress đã có từ Phase 4.x.
- Navigation chỉ đổi phần UI đang hiển thị. Nó không gọi provisioning, start, stop, restart hay tự tạo process.
- Typography, spacing, button/input/error states, focus keyboard và responsive layout dùng chung cho toàn shell.

## Wireframe

### Đang đọc trạng thái

```text
┌ CoffeePOS ───────────────────────────────┐
│ Đang mở cửa hàng…                       │
│ Đang đọc trạng thái cài đặt.            │
└─────────────────────────────────────────┘
```

Nếu bootstrap native lỗi, cùng vùng này hiển thị lỗi và action **Thử lại**. Không render dashboard giả khi chưa biết installation state.

### Chưa cài / setup có thể retry

```text
┌ CoffeePOS                                │
│ Thiết lập cửa hàng                      │
│ Cài một lần; dữ liệu nằm trên máy này. │
│                                         │
│ [ Cài CoffeePOS / Tiếp tục thiết lập ] │
│ trạng thái + lỗi/recovery thật           │
└─────────────────────────────────────────┘
```

Phase 4.12 recovery giữ nguyên: `can_retry=true` cho phép chạy lại ensure flow; blocker `can_retry=false` chỉ cho **Kiểm tra lại trạng thái**, không tạo nút Repair giả.

### Store đã cài

```text
┌ CoffeePOS ───────────────────────────────┐
│ Trang chính | Cài đặt | Chẩn đoán       │
├─────────────────────────────────────────┤
│ Trang chính                             │
│ <tên cửa hàng>                          │
│ trạng thái ngắn dựa trên runtime/health │
└─────────────────────────────────────────┘
```

`Cài đặt` hiển thị cấu hình Desktop hiện có ở mức read-only. `Chẩn đoán` hiển thị runtime controls và technical state đã có. Không có mục Backup/POS hoặc chức năng tương lai rỗng.

## Native contract được tái sử dụng

Frontend chỉ gọi các command hiện có:

- `get_shell_info`
- `get_provisioning_info`
- `provision_wordpress`
- `get_runtime_info`
- `start_runtime`
- `stop_runtime`
- `restart_runtime`
- `open_wordpress`
- `save_store_name`

Bootstrap đọc `ShellInfo`, sau đó `ProvisioningInfo`. `ProvisioningInfo.state === "ready"` là gate duy nhất để vào installed-store shell. Runtime state không quyết định quay lại setup: store `ready + stopped` vẫn ở shell. Reload chỉ đọc state; không gọi start/provision tự động.

## Navigation và accessibility

- Navigation dùng button thật với `aria-current="page"`; Tab + Enter/Space hoạt động theo semantics native.
- Khi đổi mục, focus chuyển tới heading của view để bàn phím và screen reader nhận biết context mới.
- Khi chuyển view chỉ cập nhật `hidden`, `aria-current` và focus; không gọi native lifecycle command.
- Status/error tiếp tục dùng `role="status"`, `aria-live` và `role="alert"` như flow hiện có.
- Layout cho phép cuộn ở cửa sổ hẹp/zoom cao; action không bị đẩy ra ngoài viewport do fixed-height container.

## Implementation

Phase 5.1 được triển khai hoàn toàn ở frontend, không đổi native contract:

- `index.html` có ba vùng độc lập: bootstrap/loading, setup/recovery và installed-store shell.
- `ProvisioningInfo.state === "ready"` là gate duy nhất để vào installed-store shell. Runtime stopped/starting/error không làm quay lại setup.
- Router installed-store chỉ đổi `hidden`, `aria-current` và focus heading. Handler điều hướng không gọi `invoke()` hay runtime lifecycle command.
- Trang chính tóm tắt state hiện có. Tên cửa hàng chỉ lấy từ CoffeePOS machine-health payload khi endpoint đã xác minh; khi runtime dừng dùng nhãn trung tính thay vì coi `ShellInfo.config.store_name` là nguồn dữ liệu nghiệp vụ.
- Form `store_name` cũ vẫn ở setup trước provisioning để không phá input mà Phase 4.12 đang dùng. Sau `ready`, Cài đặt không expose field này như một store-name editor độc lập.
- Runtime controls, ports, versions, data directory, WordPress health, CoffeePOS health và action mở WordPress kiểm thử nằm trong Chẩn đoán.
- Các `role="status"` / `role="alert"` được giữ lại; text của vùng live chỉ được thay khi giá trị thực sự đổi để poll 2 giây không tạo announcement lặp vô ích.
- CSS không dùng fixed-height shell; viewport hẹp dùng wrap/scroll và nội dung dài được phép wrap.

## Acceptance Windows-first

1. Fresh/not-installed store mở thẳng setup; navigation installed-store không xuất hiện.
2. Setup success hoặc store đã `ready` từ lần trước mở vào Trang chính; reload/relaunch không quay lại setup chỉ vì runtime stopped.
3. Setup retryable và non-retryable blocker vẫn giữ đúng action Phase 4.12.
4. Chuột chuyển được Trang chính → Cài đặt → Chẩn đoán → Trang chính; bàn phím Tab + Enter/Space cũng chuyển được, focus tới heading của view.
5. Điều hướng không thay đổi runtime process identity và không gọi start/restart; runtime đang stopped vẫn stopped, runtime đang running vẫn giữ process hiện tại.
6. Chẩn đoán vẫn chạy được start/stop/restart/open WordPress và hiển thị native success/error hiện có.
7. Setup trước `ready` vẫn save được tên cấu hình provisioning hiện có và hiển thị lỗi native khi có; installed Cài đặt không tạo một store-name source thứ hai.
8. Ở viewport hẹp và zoom/DPI cao, navigation wrap/scroll hợp lý, nội dung dài wrap và các action chính vẫn truy cập được bằng cuộn.
9. `npm run lint:ui`, `npm run build:ui`, Rust test/fmt/clippy hiện có đều pass; app Tauri thật được dùng cho interaction acceptance.

## Validation hiện tại

Automated/native regression trên Windows x64 ngày 2026-09-18:

```text
TypeScript: npm run lint:ui — PASS
Frontend production build: npm run build:ui — PASS (7 modules)
Full lint: npm run lint — PASS (TypeScript + cargo fmt --check + Clippy -D warnings)
Rust unit tests: 42 passed, 0 failed, 4 ignored
Phase 4.12 journal-boundary recovery E2E — PASS, 242.36s
Phase 4.12 partial-WordPress repair-blocker E2E — PASS, 101.10s
Full provisioning/idempotency E2E — PASS on clean final attempt, 255.65s test time
git diff --check — PASS
```

App Tauri development thật được mở bằng local Rust toolchain và store đã provision. Bằng chứng interaction/runtime đã thu được trực tiếp từ WebView/native IPC:

- Relaunch khi store `ready + stopped` vào installed shell → Trang chính, focus ở `home-title`; setup không xuất hiện và runtime vẫn stopped.
- Chuyển Home → Settings → Diagnostics → Home cập nhật đúng active view và focus heading. Chín lượt đổi view khi runtime đang chạy giữ nguyên `database_pid=15972`, `php_pid=356`; số log `runtime start requested` giữ `25 → 25`.
- `Page.reload` khi runtime đang chạy giữ nguyên hai PID trên, quay về installed shell/Home và số `runtime start requested` vẫn `25 → 25`. Reload khi stopped cũng giữ `24 → 24` và không auto-start.
- Diagnostics start thật đạt `running + WordPress healthy + CoffeePOS healthy`; restart thật tạo PID mới `database_pid=13532`, `php_pid=9048` rồi trở lại healthy; stop thật trả PID về `null` và WordPress/CoffeePOS health về `unavailable`.
- Windows mouse input thật click mục **Cài đặt** trong cửa sổ Tauri và WebView chuyển sang `settings`, focus `settings-title`.
- Windows keyboard input thật từ heading dùng `Shift+Tab` + `Enter` chuyển sang **Chẩn đoán** với focus `diagnostics-title`; `Shift+Tab` + `Space` chuyển lại **Cài đặt** với focus `settings-title`.
- Native window được resize xuống outer `476×599`, tương ứng WebView viewport đúng `460×560`; document cao `660px` nên cuộn dọc thay vì cắt nội dung, navigation không overflow (`scrollWidth=317`, `clientWidth=317`).
- Hai monitor test đều là Windows 100% DPI (`96 DPI`). Không thay đổi display setting của máy; kiểm tra 150% được chạy trong chính WebView2/Tauri bằng `deviceScaleFactor=1.5` tại viewport `460×560`, cho `devicePixelRatio=1.5` và navigation vẫn `317/317` không overflow.
- `staged_runtime_provisions_twice_stops_and_cleans_temp_store` có hai attempt đầu timeout ở các boundary khác nhau khi máy đồng thời chạy Cargo/rustc; sau khi build contention về 0, clean final attempt pass. Không còn PHP/MariaDB staged process sau test.

Các lệnh gate dùng cho snapshot cuối:

```powershell
. .\scripts\use-local-rust.ps1
npm run lint:ui
npm run build:ui
npm test
npm run lint
npm run build
cargo test --manifest-path .\src-tauri\Cargo.toml --locked provisioning::tests::staged_runtime_recovers_across_first_run_journal_boundaries -- --ignored
cargo test --manifest-path .\src-tauri\Cargo.toml --locked provisioning::tests::staged_runtime_blocks_retry_for_partial_wordpress_tables -- --ignored
cargo test --manifest-path .\src-tauri\Cargo.toml --locked provisioning::tests::staged_runtime_provisions_twice_stops_and_cleans_temp_store -- --ignored
```

## Kết quả

Phase 5.1 hoàn thành Windows-first ngày 2026-09-18. Navigation được nghiệm thu bằng input Windows thật, runtime identity không đổi do navigation/reload, resize minimum vẫn truy cập được action bằng scroll, và 150% rendering được kiểm bằng WebView2 DPI emulation trong app Tauri thật vì host không có monitor 150%. Không có blocker kiến trúc còn lại cho việc bắt đầu Phase 5.2.

## Ngoài scope

- Form tài khoản/onboarding hoàn chỉnh: Phase 5.2.
- Trang chính/cài đặt sản phẩm đầy đủ: Phase 5.3.
- Mở POS và login: Phase 5.4.
- Auto-start runtime: Phase 5.5.
- Minimize/exit/shutdown UX: Phase 5.6.
- Diagnostics/repair/log export đầy đủ: Phase 6.x.
