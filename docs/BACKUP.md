# Backup / restore — hợp đồng Phase 6

Chưa triển khai. Backup portable gồm logical database dump, uploads, cấu hình cần phục hồi và metadata exact component/schema versions, checksums. Không bundle PHP/MariaDB binaries hoặc WordPress core trong backup dữ liệu.

Restore phải validate archive paths (chặn traversal/symlinks thoát root), checksums và compatibility trước khi thay dữ liệu. Dừng runtime, backup current state, restore vào staging, chạy explicit migrations, health check, rồi chuyển active store. Thất bại giữ khả năng phục hồi trạng thái trước; downgrade schema chỉ khi có đường rollback đã kiểm chứng.

Credential không xuất plaintext tùy tiện trong backup. Chính sách encryption/key recovery phải chốt trước implementation. Copy live MariaDB datadir không được coi là backup portable Windows ↔ macOS.
