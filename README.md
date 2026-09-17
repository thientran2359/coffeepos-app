# CoffeePOS Desktop

Tauri 2 + Rust + giao diện TypeScript thuần. Desktop quản lý môi trường chạy; nghiệp vụ POS vẫn thuộc plugin CoffeePOS trên WordPress/WooCommerce.

## Bắt đầu

Cài toolchain theo [DEVELOPMENT.md](docs/DEVELOPMENT.md), sau đó:

```sh
npm ci
npm run doctor
npm run dev
```

Phase 1 Windows đã hoàn tất shell/config. Phase 2 Windows-first đã có runtime manager cho PHP + MariaDB portable với start/stop/restart, dynamic loopback ports, SQL/HTTP readiness, logging và process containment. **WordPress/WooCommerce/CoffeePOS provisioning vẫn chưa được thực hiện; đó là Phase 3–4.**

```sh
npm run dev:ui    # preview trong trình duyệt; không có native IPC hoặc persistence
npm run build:ui  # typecheck + build frontend
npm run build     # native executable, chưa đóng gói installer
npm run package   # bundle shell trên OS hiện tại; chưa phải bộ cài POS hoàn chỉnh
npm run lint      # TypeScript + Rust formatting + Clippy
npm test          # Rust tests cho cấu hình/persistence/locking
```

## Tài liệu

- [ARCHITECTURE.md](ARCHITECTURE.md): ranh giới kiến trúc, khảo sát Windows/macOS, quyết định và rủi ro.
- [Phase 1](docs/PHASE-01.md): phạm vi, checklist nghiệm thu và bằng chứng kiểm tra.
- [Phase 2](docs/PHASE-02.md): runtime manager Windows-first, pinned binaries và bằng chứng start/stop/restart.
- [Development](docs/DEVELOPMENT.md): setup từng hệ điều hành, lệnh chạy, vị trí dữ liệu.
- [Runtime](docs/RUNTIME.md): hợp đồng runtime cho Phase 2.
- [Provisioning](docs/PROVISIONING.md): kế hoạch Phase 3–4 và health API đề xuất.
- [Backup](docs/BACKUP.md): hợp đồng tương lai, chưa triển khai.

`AGENTS.md` là hướng dẫn dự án. Chưa có Git repository trong thư mục tại thời điểm khảo sát; các file CI chỉ chạy sau khi đưa dự án lên GitHub.
