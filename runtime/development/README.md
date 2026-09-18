# Development runtime

Phase 2 Windows-first dùng runtime portable đã pin, không lấy PHP/MariaDB từ `PATH`, LocalWP hoặc cài đặt toàn hệ thống.

Chạy từ project root:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\stage-runtime-development.ps1
```

Script đọc manifest template được version-control tại `scripts/runtime-development/x86_64-pc-windows-msvc.manifest.json`, tải đúng artifact chính thức, kiểm SHA256 trước khi extract, rồi tạo layout ignored:

```text
runtime/development/
├── .downloads/                         # cache archive, ignored
└── x86_64-pc-windows-msvc/             # ignored
    ├── manifest.json
    ├── php/
    │   ├── php.exe
    │   └── php.ini
    ├── mariadb/
    │   └── bin/
    │       ├── mariadbd.exe
    │       ├── mariadb.exe
    │       └── mariadb-install-db.exe
    └── fixture/
        ├── router.php
        └── site/
```

Pinned archives:

- PHP 8.4.25 NTS VS17 x64: `php-8.4.25-nts-Win32-vs17-x64.zip`, SHA256 `43a8f67ed2e5223fafb21293c85976361808855405278cef2cf3037c3ae2529c`.
- MariaDB 11.4.13 x64 ZIP: `mariadb-11.4.13-winx64.zip`, SHA256 `d62986d433eeebfde218560b276103831604a61e929e87f1a17f5aebd80257e2`.

Nguồn download trong manifest dùng archive URL chính thức theo exact version. `php.ini` staged materialize `extension_dir` thành đường dẫn tuyệt đối bên trong target development runtime, nên kiểm extension không phụ thuộc working directory của shell.

Manifest cũng giữ nguồn checksum và license metadata. File license gốc (`php/license.txt`, `mariadb/COPYING`) đi cùng bundle sau extract.

`fixture/router.php` chỉ phục vụ readiness của Phase 2 tại `/__coffeepos_runtime_health`; nó không provision hoặc mô phỏng WordPress.

## WordPress core cho Phase 3

WordPress core được stage riêng, không trộn vào PHP/MariaDB và không lấy từ site LocalWP đang chạy:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\stage-wordpress-development.ps1
```

Script đọc `scripts/wordpress-development/wordpress-7.1.manifest.json`, xác minh cả SHA256 đã pin và SHA1 do WordPress.org công bố trước khi extract. Core thật nằm trong target ignored:

```text
runtime/development/x86_64-pc-windows-msvc/
├── wordpress-manifest.json
└── wordpress/
    └── wordpress/
        ├── index.php
        ├── license.txt
        ├── readme.html
        ├── wp-admin/
        ├── wp-content/
        └── wp-includes/
```

WordPress 7.1 archive: `https://wordpress.org/wordpress-7.1.zip`; SHA256 `d1ae02b5ae18428031ffc3943659fa87ab361d827f4aa804adf9276e4dc75df6`; official SHA1 `b2b81d9242a122a8c7104a92387794eb64fcde97`. `wordpress-manifest.json` resolves `core_root` as `wordpress/wordpress` relative to the development target root. Actual core and staged manifest remain ignored; the checked-in manifest template/script are the reproducible source of truth.

## WooCommerce artifact cho Phase 4.4

WooCommerce được stage riêng khỏi WordPress core và chưa được copy/activate vào managed site ở Phase 4.4:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\stage-woocommerce-development.ps1
```

Script đọc `scripts/woocommerce-development/woocommerce-11.1.0.manifest.json`, tải exact official WordPress.org plugin archive, kiểm SHA256 đã pin và metadata plugin trước/sau khi stage. Layout ignored:

```text
runtime/development/x86_64-pc-windows-msvc/
├── woocommerce-manifest.json
└── woocommerce/
    └── woocommerce/
        ├── woocommerce.php
        ├── license.txt
        ├── readme.txt
        ├── includes/
        ├── src/
        └── vendor/
```

Pinned WooCommerce 11.1.0 archive: `https://downloads.wordpress.org/plugin/woocommerce.11.1.0.zip`; SHA256 `6bae9bf74d722b6deb15f049687c311cfafc26e3a5d8fa55ac6ea4b9a3a8df19`. Package header requires WordPress 7.0+ and PHP 7.4+; official plugin metadata is tested through WordPress 7.1. Runtime development versions WordPress 7.1, PHP 8.4.25 and MariaDB 11.4.13 satisfy the selected baseline. The archive `readme.txt` currently contains a stale `Stable tag: 11.0.1`, so the staging contract intentionally verifies the `woocommerce.php` version plus pinned release/checksum rather than treating that field as authoritative.
