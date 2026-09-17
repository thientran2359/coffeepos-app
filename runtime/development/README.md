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
