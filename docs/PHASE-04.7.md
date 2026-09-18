# Phase 4.7 — CoffeePOS artifact (Windows-first)

## Trạng thái

**Hoàn thành Windows-first ngày 2026-09-18.**

Phase 4.7 pin một CoffeePOS release snapshot cụ thể cho Desktop development staging. Phase này chưa copy CoffeePOS vào managed WordPress site và chưa activate plugin; provisioning/activation thuộc Phase 4.8–4.9.

## Artifact đã pin

| Thuộc tính | Giá trị |
| --- | --- |
| CoffeePOS | `1.0.0` |
| Source Git commit | `9473867c65f1409dbeeaa03a98daef564f620b8e` |
| Checked-in archive | `scripts/coffeepos-development/coffeepos-1.0.0.zip` |
| SHA256 | `ee9f241a516e7c6ddddc6e84ad26515d0a9cd9ccd5d6fd101d078a738e598f2a` |
| License | `GPL-2.0-or-later` |
| Required plugin | `woocommerce` |
| Plugin root | `coffeepos/coffeepos` |
| Entry file | `coffeepos/coffeepos/coffeepos.php` |

Archive được build ngày 2026-09-18 từ detached clean worktree tại exact commit trên, dùng Composer `2.10.2` + PHP CLI `8.2.27`, chạy production autoload và CoffeePOS Phase 13 release checks trước khi package. Working tree CoffeePOS đang có thay đổi chưa commit không được đưa vào artifact này.

Artifact ZIP được check vào Desktop repository vì CoffeePOS là project-owned plugin, archive nhỏ và Phase 4.8 cần một immutable input không phụ thuộc sibling checkout, LocalWP hoặc trạng thái working tree khác. SHA256 trong manifest khóa exact bytes mà staging được phép dùng.

Source commit là provenance của snapshot, không phải cam kết rằng chạy lại `tools/build-release.ps1` sẽ tạo byte-identical ZIP: release builder hiện giữ file timestamps và Composer-generated autoload metadata nên rebuilt archive có thể có SHA256 khác. Phase 4.7 lấy checked-in ZIP + SHA256 làm immutable distribution input. Nếu sau này cần byte-reproducible build, release tooling phải pin toolchain và normalize ZIP metadata/timestamps trước khi đổi contract artifact.

## Source of truth trong repo

```text
scripts/coffeepos-development/
├── coffeepos-1.0.0.manifest.json
└── coffeepos-1.0.0.zip

scripts/stage-coffeepos-development.ps1
```

Manifest pin version, source commit, archive SHA256, license, dependency, plugin layout và compatibility baseline. Không dùng `latest`, không đọc plugin từ một WordPress/LocalWP install đang chạy và không copy sibling CoffeePOS working tree lúc staging.

## Deterministic staging

Chạy từ Desktop project root:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\stage-coffeepos-development.ps1
```

Script:

1. đọc checked-in manifest;
2. chặn target/plugin/archive path escape;
3. verify exact checked-in ZIP SHA256;
4. reject ZIP entry ngoài `coffeepos/` hoặc có parent traversal;
5. extract vào ignored temporary directory;
6. verify `coffeepos.php`, `readme.txt`, `LICENSE`, assets/includes/templates/languages và production `vendor/autoload.php`;
7. verify plugin header version, `COFFEEPOS_VERSION`, WordPress/PHP requirements và dependency `woocommerce`;
8. verify readme Stable tag/tested WordPress metadata;
9. chỉ replace owned CoffeePOS staging subtree trong development runtime;
10. copy manifest vào ignored target rồi verify metadata lần nữa.

## Staged development layout

```text
runtime/development/x86_64-pc-windows-msvc/
├── coffeepos-manifest.json
└── coffeepos/
    └── coffeepos/
        ├── coffeepos.php
        ├── LICENSE
        ├── readme.txt
        ├── assets/
        ├── includes/
        ├── languages/
        ├── templates/
        └── vendor/
            └── autoload.php
```

`runtime/development/*` vẫn ignored. Checked-in release ZIP + manifest + staging script là source of truth để Phase 4.8 consume.

## Health contract gate

Artifact `1.0.0` hiện có user-facing `/wp-json/coffeepos/v1/health`; route đó dùng staff/session authorization và không phải Desktop machine-health contract.

Desktop machine-health contract cho Phase 4.10 được chốt riêng trong `docs/PROVISIONING.md`: endpoint `/wp-json/coffeepos/v1/system/status`, schema version 1, machine token riêng, response version/component/POS route rõ ràng. Phase 4.7 không âm thầm sửa CoffeePOS plugin để implement endpoint này.

Vì exact artifact đã được pin, nếu Phase 4.10 cần thay CoffeePOS code để implement machine-health endpoint thì phải bump CoffeePOS version và immutable archive hash, repin manifest và chạy lại acceptance 4.8–4.10. Không được mutate staged `1.0.0` tree tại chỗ.

## Validation

Source release validation trước khi pin:

```text
Phase 13 release scenarios passed.
```

Clean source commit build:

```text
CoffeePOS 1.0.0
source commit = 9473867c65f1409dbeeaa03a98daef564f620b8e
archive SHA256 = ee9f241a516e7c6ddddc6e84ad26515d0a9cd9ccd5d6fd101d078a738e598f2a
archive entries = 318
entries outside coffeepos/ = 0
vendor/autoload.php = present
```

Staging acceptance phải pass clean target và repeated staging với cùng staged tree fingerprint.

Final Windows development staging:

```text
files before = 318
files after  = 318
tree fingerprint before = 1b7871c268d2aceca7e9e60b51d57a31f3a21be82251219abcdb0b0971fd1191
tree fingerprint after  = 1b7871c268d2aceca7e9e60b51d57a31f3a21be82251219abcdb0b0971fd1191
match = True
```

Lần stage thứ hai verify lại SHA256, metadata và archive safety trước khi replace CoffeePOS staging subtree. WordPress, WooCommerce và runtime sibling trees trong cùng development target không bị script đụng tới.

## Ngoài phạm vi Phase 4.7

- Copy/install CoffeePOS vào managed WordPress site: Phase 4.8.
- Ownership/ensure semantics cho installed CoffeePOS: Phase 4.8.
- CoffeePOS activation/migrations/roles/settings prerequisite: Phase 4.9.
- Desktop machine-health endpoint implementation và native consumption: Phase 4.10.
- Full stack idempotency/recovery: Phase 4.11–4.12.

## Gate sang Phase 4.8

Phase 4.8 chỉ consume exact staged CoffeePOS artifact đã pin ở đây. Runtime provisioning không được đọc sibling source repo, LocalWP plugin directory hoặc một mutable development checkout.
