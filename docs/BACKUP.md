# Backup / restore — hợp đồng Phase 7.x

Chưa triển khai. Roadmap chia backup/restore thành Phase 7.1–7.4; xem [ROADMAP.md](ROADMAP.md). Backup portable gồm logical database dump, uploads, cấu hình cần phục hồi và metadata exact component/schema versions, checksums. Không bundle PHP/MariaDB binaries hoặc WordPress core trong backup dữ liệu.

Restore phải validate archive paths (chặn traversal/symlinks thoát root), checksums và compatibility trước khi thay dữ liệu. Dừng runtime, backup current state, restore vào staging, chạy explicit migrations, health check, rồi chuyển active store. Thất bại giữ khả năng phục hồi trạng thái trước; downgrade schema chỉ khi có đường rollback đã kiểm chứng.

Credential không xuất plaintext tùy tiện trong backup. Chính sách encryption/key recovery phải chốt trước implementation. Copy live MariaDB datadir không được coi là backup portable Windows ↔ macOS.

Phase 7 dùng dump/import tools từ development artifact đã pin/verify; Phase 9.1 mới bundle production. Snapshot database và uploads/config phải nhất quán: chặn writers/request/jobs trong thời gian tạo backup hoặc chứng minh cơ chế tương đương. Archive chỉ được đánh dấu hoàn tất khi tất cả thành phần đã validate.

Portable restore cần tái tạo secrets/config cho profile đích; không coi copy DPAPI blob của profile cũ là giải pháp chuyển máy. Test restore sang profile Windows khác trước; cross-platform restore chỉ được công bố khi có acceptance cả Windows lẫn macOS. Bản current-state backup đã validate phải tồn tại trước khi chuyển active store; lỗi staging/migration/health không được làm mất bản gốc.
