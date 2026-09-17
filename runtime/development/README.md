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
