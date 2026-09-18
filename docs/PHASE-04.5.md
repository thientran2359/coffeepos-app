# Phase 4.5 — WooCommerce provisioning (Windows-first)

## Trạng thái

**Hoàn thành Windows-first ngày 2026-09-18.**

Phase 4.5 đưa exact WooCommerce `11.1.0` artifact đã pin ở Phase 4.4 vào managed WordPress site bằng ownership-safe ensure semantics. WooCommerce **chưa được activate**; activation, setup/schema và background jobs thuộc Phase 4.6.

## Provisioning contract

Native provisioning resolve `runtime/development/<target>/woocommerce-manifest.json` và staged plugin tree trước khi thao tác với store.

Resolver kiểm:

- manifest schema/target đúng platform;
- path trong manifest không absolute và không chứa parent traversal;
- staged plugin root nằm bên trong `runtime/development`;
- required files/directories tồn tại;
- `woocommerce.php` báo đúng version `11.1.0`;
- pinned archive SHA256 metadata khớp manifest.

Không download WooCommerce trong runtime/provisioning. App chỉ consume staged artifact đã pin từ Phase 4.4.

## Ownership-safe ensure semantics

Destination:

```text
<app-local-data>/site/wp-content/plugins/woocommerce/
```

Fresh provisioning:

1. Copy staged WooCommerce tree vào owned staging directory:

```text
<app-local-data>/woocommerce.provisioning/
```

2. Verify copied `woocommerce.php` vẫn báo version `11.1.0`.
3. Ghi ownership metadata ngay trong staged plugin:

```text
.coffeepos-managed.json
```

Metadata pin:

- schema version;
- plugin slug `woocommerce`;
- plugin version `11.1.0`;
- exact Phase 4.4 archive SHA256.

4. Rename staging directory atomically thành `wp-content/plugins/woocommerce`.

Staging nằm ngoài site cho tới bước rename cuối nên WordPress không nhìn thấy partial plugin tree trong lúc copy.

## Existing destination policy

Nếu `wp-content/plugins/woocommerce` đã tồn tại:

- không phải directory → từ chối, preserve path;
- không có readable CoffeePOS ownership metadata → từ chối, preserve toàn bộ plugin;
- ownership metadata invalid/incompatible → từ chối, preserve plugin;
- managed version/hash khác pinned artifact → từ chối và yêu cầu explicit upgrade flow;
- managed metadata đúng nhưng `woocommerce.php` bị đổi/corrupt version → từ chối repair tự động;
- managed version/hash đúng → no-op, không recopy plugin tree.

Vì retry cùng version là no-op, file/data bổ sung bên trong managed WooCommerce directory không bị xóa bởi provisioning lần hai.

Plugin khác trong `wp-content/plugins` không bị đụng tới.

## Provisioning journal

Journal có stage mới:

```text
woocommerce_provisioned
```

và ghi `woocommerce_version`.

Store chỉ được native báo `ready` khi đồng thời:

- WordPress/database layout hợp lệ;
- journal WordPress version đúng;
- journal WooCommerce version đúng;
- stage ít nhất `woocommerce_provisioned`;
- WooCommerce ownership metadata + installed plugin header khớp pinned 11.1.0 artifact.

Journal cũ từ Phase 4.3/4.4 vẫn deserialize được vì `woocommerce_version` là backward-compatible optional field. Store cũ vì chưa có WooCommerce sẽ hiện `needs_repair` + `can_retry`, sau retry provisioning sẽ được nâng lên stage mới mà không reset WordPress/database.

## UI contract

Provisioning UI hiện hiển thị:

- Phase 4.5;
- WordPress version;
- WooCommerce version;
- trạng thái Ready chỉ sau WooCommerce provisioning;
- wording rõ WooCommerce đã được provision nhưng chưa activate.

Existing app flow dùng cùng `provision_wordpress` command; không thêm parallel provisioning path riêng.

## Validation

### Unit/static

| Kiểm tra | Kết quả |
| --- | --- |
| `npm run lint:ui` | PASS |
| `npm run build:ui` | PASS |
| Rust tests | PASS: 26 passed, 0 failed, 2 ignored |
| `cargo clippy --locked --all-targets -- -D warnings` | PASS |
| WooCommerce checked-in manifest schema | PASS |
| WooCommerce manifest path escape rejection | PASS |
| Managed plugin retry preserves sentinel | PASS |
| Existing unmanaged WooCommerce is refused/preserved | PASS |

### Real staged E2E

Command:

```powershell
. .\scripts\use-local-rust.ps1
cargo test --manifest-path .\src-tauri\Cargo.toml --locked provisioning::tests::staged_runtime_provisions_twice_stops_and_cleans_temp_store -- --ignored
```

Final run:

```text
1 passed, 0 failed
98.19s
```

Acceptance chứng minh:

- fresh WordPress store được provision WooCommerce `11.1.0`;
- `.coffeepos-managed.json` được tạo;
- `woocommerce.provisioning` được cleanup sau atomic rename;
- provisioning lần hai vẫn Ready;
- sentinel bên trong managed WooCommerce directory được giữ nguyên;
- unrelated existing plugin + sentinel được giữ nguyên;
- WordPress/runtime lifecycle acceptance cũ vẫn pass.

### Existing development store migration

App-local-data store từ Phase 4.3 trước khi retry hiển thị:

```text
needs_repair
```

với action **Thử lại provisioning** enabled. Sau khi invoke provisioning thật qua app development:

```text
ready
WordPress 7.1 + WooCommerce 11.1.0
```

File thực tế tại app-local-data xác nhận:

- `woocommerce.php` version `11.1.0`;
- ownership metadata đúng slug/version/SHA256;
- không còn `woocommerce.provisioning` staging directory.

## Ngoài phạm vi Phase 4.5

- WooCommerce activation: Phase 4.6.
- WooCommerce DB/setup migrations: Phase 4.6.
- Onboarding/background jobs/WP-Cron handling: Phase 4.6.
- Explicit upgrade từ một managed WooCommerce version khác: phase update/upgrade riêng; provisioning 4.5 cố ý không overwrite.
- Adoption một WooCommerce directory có sẵn nhưng không thuộc CoffeePOS ownership: repair/adoption flow riêng.
- CoffeePOS plugin: Phase 4.7+.

## Gate sang Phase 4.6

**Đã mở.** Phase tiếp theo là **Phase 4.6 — WooCommerce activation**. Phase 4.6 phải activate đúng managed WooCommerce 11.1.0 đã provision, kiểm dependency/version trước activation và xử lý setup/schema/background jobs có bounded failure. Không được coi file presence là active/usable.
