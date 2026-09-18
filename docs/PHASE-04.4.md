# Phase 4.4 — WooCommerce artifact (Windows-first)

## Trạng thái

**Hoàn thành Windows-first ngày 2026-09-18.**

Phase 4.4 chỉ pin và stage artifact WooCommerce. Chưa copy plugin vào managed WordPress site, chưa activate WooCommerce và chưa chạy database/setup jobs; các phần đó thuộc Phase 4.5–4.6.

## Artifact đã pin

| Thuộc tính | Giá trị |
| --- | --- |
| WooCommerce | `11.1.0` |
| Release date | `2026-09-03` |
| Official archive | `https://downloads.wordpress.org/plugin/woocommerce.11.1.0.zip` |
| SHA256 | `6bae9bf74d722b6deb15f049687c311cfafc26e3a5d8fa55ac6ea4b9a3a8df19` |
| SHA256 source | Tính độc lập từ exact official archive ngày 2026-09-18 |
| License | `GPL-3.0-or-later` |
| Plugin root | `woocommerce/woocommerce` |
| Entry file | `woocommerce/woocommerce/woocommerce.php` |

Official release list xác nhận `11.1.0` là stable release ngày 2026-09-03. WordPress.org plugin metadata ghi WooCommerce 11.1.0 yêu cầu WordPress 7.0+ và PHP 7.4+, tested up to WordPress 7.1. WooCommerce server recommendations cho dòng 10.8+ khuyến nghị PHP 8.3+ và MariaDB 10.6+, tested PHP đến 8.4. Baseline CoffeePOS Desktop WordPress 7.1 + PHP 8.4.25 + MariaDB 11.4.13 phù hợp với các requirement này.

## Source of truth trong repo

Manifest version-controlled:

```text
scripts/woocommerce-development/woocommerce-11.1.0.manifest.json
```

Manifest pin:

- exact version và exact download URL;
- independent SHA256;
- official release/plugin/server requirement sources;
- license/readme/entry file paths;
- WordPress/PHP/MariaDB compatibility baseline.

Không dùng `latest` trong staging hoặc runtime.

## Deterministic staging

Chạy từ project root:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\stage-woocommerce-development.ps1
```

Script:

1. đọc checked-in manifest;
2. chặn target/plugin path escape khỏi `runtime/development`;
3. dùng cache archive chỉ khi SHA256 đúng;
4. download exact official archive nếu cache thiếu/sai;
5. verify SHA256 trước extract;
6. verify required plugin files/directories;
7. verify `woocommerce.php` header version `11.1.0`;
8. verify minimum WordPress `7.0`, minimum PHP `7.4`;
9. verify `readme.txt` tested up to WordPress `7.1` và có changelog `11.1.0 2026-09-03`;
10. chỉ replace owned WooCommerce staging subtree;
11. copy manifest vào ignored development target;
12. verify metadata lần nữa sau stage.

## Staged development layout

Actual staged artifact nằm trong ignored target:

```text
runtime/development/x86_64-pc-windows-msvc/
├── woocommerce-manifest.json
└── woocommerce/
    └── woocommerce/
        ├── woocommerce.php
        ├── license.txt
        ├── readme.txt
        ├── assets/
        ├── includes/
        ├── packages/
        ├── src/
        ├── templates/
        └── vendor/
```

`.gitignore` đã ignore `runtime/development/*`, nên binary/package tree không được commit. Checked-in manifest + staging script là reproducible source of truth.

## Package metadata note

Official `woocommerce.php` trong archive ghi `Version: 11.1.0`. `readme.txt` có changelog `11.1.0 2026-09-03`, nhưng field `Stable tag` trong chính archive hiện vẫn ghi `11.0.1`.

Vì field đó không đồng bộ với release/package header, Phase 4.4 **không** dùng `Stable tag` làm version authority. Version contract dựa trên:

- exact pinned official archive URL;
- pinned SHA256;
- official WooCommerce release list;
- `woocommerce.php` plugin header;
- matching 11.1.0 changelog entry.

## Validation

Clean-target staging ngày 2026-09-18:

```text
Verified WooCommerce SHA256
6bae9bf74d722b6deb15f049687c311cfafc26e3a5d8fa55ac6ea4b9a3a8df19

Staged WooCommerce 11.1.0
```

Sau lần stage đầu, archive/plugin metadata/layout/license/readme đều được xác minh. Lần stage thứ hai dùng cùng pinned cache và tạo lại cùng tree:

```text
files=5862
tree fingerprint before = 3b7cf53a8d6b1135c062d3c43130d20f11ba7342633b5a40ad8740af4ea6ac6c
tree fingerprint after  = 3b7cf53a8d6b1135c062d3c43130d20f11ba7342633b5a40ad8740af4ea6ac6c
match=True
```

Điều này chứng minh repeated staging không phụ thuộc nội dung WooCommerce target cũ và tái tạo cùng plugin tree từ artifact đã pin.

## Ngoài phạm vi Phase 4.4

- Copy/install WooCommerce vào managed WordPress site: Phase 4.5.
- Ownership/ensure semantics cho plugin destination: Phase 4.5.
- Activate WooCommerce: Phase 4.6.
- WooCommerce database/setup migrations và background jobs: Phase 4.6.
- Onboarding suppression/configuration cho unattended local store: Phase 4.6.
- CoffeePOS plugin artifact: Phase 4.7.

## Gate sang Phase 4.5

**Đã mở.** Phase tiếp theo là **Phase 4.5 — WooCommerce provisioning**. Phase 4.5 phải consume đúng staged WooCommerce 11.1.0 artifact này, không download `latest` và không copy ngẫu nhiên từ một WordPress/LocalWP install khác.
