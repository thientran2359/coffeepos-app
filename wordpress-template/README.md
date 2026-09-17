# WordPress template

Phase 3 Windows-first pin WordPress core 7.1 từ archive chính thức. Core không được commit vào repo và không được sửa; development staging nằm tại `runtime/development/x86_64-pc-windows-msvc/wordpress/wordpress/`, với `wordpress-manifest.json` ngay tại target root `x86_64-pc-windows-msvc/` để native provisioning resolve version/hash/core root từ cùng một base path. Tất cả được tái tạo bằng `scripts/stage-wordpress-development.ps1` từ manifest checked-in `scripts/wordpress-development/wordpress-7.1.manifest.json`.

Native Phase 3 provisioning copy từ baseline này vào store `site/`, sau đó tạo `wp-config.php` và chạy install theo ensure semantics. Baseline không chứa credential, database, uploads, WooCommerce hoặc CoffeePOS. WooCommerce/CoffeePOS được triển khai theo các subphase 4.4–4.12 trong `docs/ROADMAP.md`.

WordPress 7.1 được chọn vì đây là stable release hiện được WordPress.org duy trì. Trang download chính thức khuyến nghị PHP 8.3+ và MariaDB 10.11+; development runtime hiện pin PHP 8.4.25 và MariaDB 11.4.13. Archive SHA256 được tính trực tiếp từ download chính thức và pin trong manifest; SHA1 được đối chiếu với file checksum chính thức của WordPress.org.
