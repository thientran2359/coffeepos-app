# Phase 4.2 — WordPress runtime UX (Windows-first)

## Trạng thái

**Hoàn thành Windows-first ngày 2026-09-18.** Phase này hợp nhất lifecycle runtime với WordPress health sau provisioning, giữ rõ ranh giới giữa `provisioning.ready`, process readiness và WordPress health.

Phase 4.2 chưa mở WordPress ra trình duyệt. Action đó thuộc Phase 4.3.

## Mục tiêu

Sau khi WordPress đã được provision, người dùng có thể nhìn thấy và điều khiển lifecycle local runtime mà không nhầm trạng thái “đã cài” với “đang chạy” hoặc “WordPress healthy”.

Flow chính:

```text
provisioning.ready
  ↓
runtime stopped
  ↓ Start
runtime starting
  ↓
MariaDB ready
  ↓
PHP ready
  ↓
WordPress health probe
  ↓
healthy | unhealthy
```

## Native contract

`RuntimeInfo` bổ sung:

```text
wordpress_health:
  unavailable | checking | healthy | unhealthy

wordpress_error:
  RuntimeErrorInfo | null
```

Ý nghĩa:

- `unavailable`: runtime chưa chạy, đang dừng hoặc startup đã fail; không giữ health cũ.
- `checking`: đang xác minh WordPress sau khi runtime readiness hoàn tất.
- `healthy`: `/wp-login.php` của WordPress trả response mong đợi trên dynamic loopback port hiện tại.
- `unhealthy`: MariaDB/PHP vẫn chạy nhưng WordPress health chưa đạt; đây không phải tín hiệu reinstall.

Probe WordPress dùng loopback HTTP thật sau mỗi `start`/`restart`, bounded bằng runtime HTTP timeout và trả lỗi component `wordpress`, operation `health` khi fail.

Provisioning dùng `start_for_provisioning()` để khởi động MariaDB/PHP mà không chờ WordPress health trước khi schema WordPress tồn tại. Sau `install_wordpress` thành công, native layer chạy `refresh_wordpress_health()` để state đầu tiên sau fresh install không bị stale.

## Lifecycle conflict

Mutating command dùng một native lifecycle mutex chung:

- `start_runtime`
- `stop_runtime`
- `restart_runtime`
- `provision_wordpress`

Nếu operation khác đang giữ lifecycle lock, command mới trả lỗi busy ngay thay vì chỉ dựa vào disabled button ở frontend.

`get_runtime_info` và `get_provisioning_info` vẫn là read/refresh path; refresh UI không tự spawn process hoặc reinstall.

## UI behavior

Runtime card hiển thị riêng:

- runtime state
- WordPress health
- PHP version
- MariaDB version
- HTTP endpoint
- database endpoint
- WordPress health error/recovery khi có

Start/restart hiển thị `starting` trong lúc command đang chạy; stop hiển thị `stopping`. Khi runtime đang chạy nhưng WordPress `unhealthy`, UI giữ provisioning ở `ready` và hướng người dùng restart/kiểm tra logs thay vì đề nghị cài lại WordPress.

Frontend refresh runtime mỗi 2 giây khi không có operation local đang chạy. Poll này chỉ gọi `get_runtime_info`, nhờ đó process chết khi UI đang mở được phản ánh mà không tạo process mới.

## Validation — Windows x64 2026-09-18

| Kiểm tra | Kết quả |
| --- | --- |
| `npm run lint:ui` | PASS |
| `npm run build:ui` | PASS |
| `cargo test --locked` | PASS: 20 passed, 0 failed, 2 ignored |
| `cargo clippy --locked --all-targets -- -D warnings` | PASS |
| Real staged provisioning/runtime E2E | PASS: 1 ignored acceptance test, 81.29s |
| Fresh provision → health refresh | PASS |
| stopped → start → WordPress healthy | PASS |
| stop clears WordPress health | PASS |
| restart while provisioned → WordPress healthy | PASS |
| PHP process dies → refresh detects failure and clears health | PASS |
| retry after child exit → healthy | PASS |
| injected PHP startup failure → stopped/unavailable | PASS |
| retry after restoring executable → healthy | PASS |
| second provisioning preserves sentinel/store data | PASS |

Acceptance command:

```powershell
. .\scripts\use-local-rust.ps1
cargo test --manifest-path .\src-tauri\Cargo.toml --locked provisioning::tests::staged_runtime_provisions_twice_stops_and_cleans_temp_store -- --ignored
```

Test dùng store tạm dưới `src-tauri/target/phase3-e2e`, không dùng store vận hành.

## Definition of Done

- `provisioning.ready` không tự suy ra runtime/WordPress health.
- Start/restart chạy MariaDB/PHP readiness trước WordPress probe.
- WordPress health có state/error typed trong native contract.
- Stop/start failure/process death không giữ `healthy` cũ.
- WordPress health failure không đổi provisioning thành `needs_repair` và không reinstall.
- Lifecycle conflict bị chặn ở native command.
- UI phản ánh stopped/starting/running/WordPress healthy-unhealthy và retry đúng.
- Refresh/reload chỉ đọc state, không spawn hoặc reinstall.
- Static checks và real staged Windows acceptance pass.

## Ngoài phạm vi

- Mở WordPress bằng browser/system URL: Phase 4.3.
- WooCommerce: Phase 4.4–4.6.
- CoffeePOS plugin/health endpoint: Phase 4.7–4.10.
- Auto-start khi mở app: Phase 5.5 (roadmap UI/UX cập nhật).
- Diagnostics/log viewer đầy đủ: Phase 6.1/6.3.
- Repair engine: Phase 6.2.

## Gate sang Phase 4.3

Phase 4.3 chỉ dùng URL runtime hiện tại khi:

```text
provisioning = ready
runtime = running
wordpress_health = healthy
```

Không hard-code port và không mở site khi health chưa đạt.
