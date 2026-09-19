# Backup / restore — hợp đồng Phase 7.x

Phase 7 được tách thành bốn milestone độc lập:

- [Phase 7.1 — Backup format](PHASE-07.1.md): encrypted portable container, manifest/inventory/checksum, compatibility và secret portability.
- [Phase 7.2 — Database backup](PHASE-07.2.md): logical MariaDB dump bằng exact managed tool, maintenance snapshot và restore-to-disposable verification.
- [Phase 7.3 — Uploads/config backup](PHASE-07.3.md): complete encrypted user backup gồm database + uploads + portable store config + administrator secret trong cùng snapshot.
- [Phase 7.4 — Restore](PHASE-07.4.md): inspect → recovery backup → isolated staging → migrations/health → cutover → rollback/crash recovery.

## Contract chung

Portable backup là **store-data backup**, không phải image của runtime. Không bundle PHP/Caddy/MariaDB binaries, WordPress core, managed plugin code hoặc live MariaDB datadir.

User backup schema 1 dùng encrypted container; DPAPI blob của source profile không được copy sang backup. Target restore tạo lại database credential, WordPress salts và machine token rồi protect bằng secret store của target profile. Administrator password được mang theo chỉ bên trong encrypted payload để giữ flow đăng nhập/Copy admin password sau cross-profile restore.

Database + uploads/config phải thuộc cùng một maintenance snapshot: Caddy/PHP/cron không ghi trong khoảng từ database dump tới khi uploads/config đã được capture. Initial Phase 7 chấp nhận POS downtime ngắn để ưu tiên consistency.

Restore luôn validate encryption, archive schema, path safety, checksums và compatibility trước mutation. Existing store phải có validated pre-restore recovery snapshot; restored store được dựng trong isolated staging, dùng target-local credentials, pass health rồi mới cutover. Interrupted cutover dùng restore journal để resume/rollback; daily startup bị gate cho tới khi transaction được reconcile.

Phase 7 dùng dump/import tools từ development artifact đã pin/verify; Phase 9.1 mới bundle production. Windows cross-profile restore là acceptance của 7.4. Windows ↔ macOS chỉ được công bố sau khi có acceptance trên cả hai target.
