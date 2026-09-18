# CoffeePOS Desktop

Tauri 2 + Rust + giao diện TypeScript thuần. Desktop quản lý môi trường chạy; nghiệp vụ POS vẫn thuộc plugin CoffeePOS trên WordPress/WooCommerce.

## Bắt đầu

Cài toolchain theo [DEVELOPMENT.md](docs/DEVELOPMENT.md), sau đó:

```sh
npm ci
npm run doctor
npm run dev
```

Phase 1–3, **Phase 4.1–4.12** và **Phase 5.1–5.2** đã hoàn tất Windows-first. **Phase 5.3 — Trang chính và cài đặt ứng dụng** đã triển khai: Home có Start/Retry theo native state, technical detail ở Chẩn đoán và Desktop có preference Trang mở đầu được persist an toàn. Lightweight automated checks đã pass; native/manual acceptance của 5.3 để người dùng smoke-test trước khi đánh dấu hoàn thành theo Definition of Done.

```sh
npm run dev:ui    # preview trong trình duyệt; không có native IPC hoặc persistence
npm run build:ui  # typecheck + build frontend
npm run build     # native executable, chưa đóng gói installer
npm run package   # bundle shell trên OS hiện tại; chưa phải bộ cài POS hoàn chỉnh
npm run lint      # TypeScript + Rust formatting + Clippy
npm test          # Rust tests cho config, runtime, secrets và provisioning
```

## Tài liệu

- [ARCHITECTURE.md](ARCHITECTURE.md): ranh giới kiến trúc, khảo sát Windows/macOS, quyết định và rủi ro.
- [Phase 1](docs/PHASE-01.md): phạm vi, checklist nghiệm thu và bằng chứng kiểm tra.
- [Phase 2](docs/PHASE-02.md): runtime manager Windows-first, pinned binaries và bằng chứng start/stop/restart.
- [Phase 3](docs/PHASE-03.md): WordPress provisioning Windows-first và real-runtime E2E.
- [Phase 4.1](docs/PHASE-04.1.md): Provisioning UI đã hoàn thành và bằng chứng nghiệm thu Windows.
- [Phase 4.11](docs/PHASE-04.11.md): full DB → WordPress → WooCommerce → CoffeePOS rerun idempotency và credential/data preservation.
- [Phase 4.12](docs/PHASE-04.12.md): recovery qua provisioning journal, interruption E2E và partial-WordPress repair blocker.
- [Phase 5.1](docs/PHASE-05.1.md): shell Trang chính/Cài đặt/Chẩn đoán, navigation/runtime non-respawn, native mouse/keyboard/resize và 150% WebView DPI acceptance.
- [Phase 5.2](docs/PHASE-05.2.md): fresh store/account onboarding, protected admin credential, real CoffeePOS login, recovery/idempotency regressions và native Tauri acceptance.
- [Phase 5.3](docs/PHASE-05.3.md): Home state/action, health retry không reinstall, Desktop startup-view preference và lightweight validation; manual acceptance đang chờ.
- [Roadmap](docs/ROADMAP.md): nguồn sự thật cho các phase/subphase và Definition of Done từ Phase 4.1 trở đi.
- [Development](docs/DEVELOPMENT.md): setup từng hệ điều hành, lệnh chạy, vị trí dữ liệu.
- [Runtime](docs/RUNTIME.md): hợp đồng runtime cho Phase 2.
- [Provisioning](docs/PROVISIONING.md): implementation Phase 3 và contract provisioning/health cho Phase 4.x.
- [Backup](docs/BACKUP.md): hợp đồng tương lai, chưa triển khai.

`AGENTS.md` là hướng dẫn dự án. Repository hiện dùng Git; trạng thái implementation phải được đối chiếu với các tài liệu Phase và validation thực tế, không suy luận từ roadmap cũ.
