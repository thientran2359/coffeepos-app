# Phase 4.8 — CoffeePOS provisioning (Windows-first)

## Trạng thái

**Hoàn thành Windows-first ngày 2026-09-18.**

Phase 4.8 đưa exact CoffeePOS 1.0.0 artifact đã pin ở Phase 4.7 vào managed WordPress site bằng ownership-safe ensure semantics. CoffeePOS chưa được activate; activation và plugin-owned migrations/capabilities/settings prerequisite thuộc Phase 4.9.

## Provisioning contract

Native provisioning resolve cùng development target:

    runtime/development/x86_64-pc-windows-msvc/
    ├── manifest.json
    ├── wordpress-manifest.json
    ├── woocommerce-manifest.json
    └── coffeepos-manifest.json

CoffeePOS resolver kiểm:

- manifest schema và target đúng platform;
- path trong manifest không absolute và không có parent traversal;
- exact layout coffeepos/coffeepos cùng entry/readme/LICENSE/autoload paths;
- staged plugin root canonical nằm trong current development target;
- coffeepos.php, readme.txt, LICENSE, assets/includes/languages/templates và production vendor/autoload.php tồn tại;
- plugin header version đúng 1.0.0;
- Requires Plugins chứa woocommerce;
- compatibility baseline khớp exact staged WordPress 7.1, PHP 8.4.25, MariaDB 11.4.13 và WooCommerce 11.1.0.

Runtime không đọc sibling CoffeePOS checkout, LocalWP plugin directory hoặc download artifact mới.

## Dependency preflight

CoffeePOS chỉ được ensure sau khi exact managed WooCommerce artifact vẫn hợp lệ. Lifecycle Phase 4.8:

    WordPress installed
    → WooCommerce provisioned
    → WooCommerce activated + setup baseline verified
    → CoffeePOS files provisioned
    → Ready

Phase 4.8 không gọi WordPress plugin activation cho CoffeePOS. Real E2E đọc trực tiếp active_plugins trong WordPress database và xác nhận WooCommerce active trong khi coffeepos/coffeepos.php chưa active.

## Ownership-safe ensure semantics

Destination:

    <app-local-data>/site/wp-content/plugins/coffeepos/

Fresh provisioning:

1. copy staged CoffeePOS tree vào owned staging directory <app-local-data>/coffeepos.provisioning/;
2. verify copied coffeepos.php vẫn báo version 1.0.0;
3. ghi .coffeepos-managed.json với schema/plugin/version/exact Phase 4.7 archive SHA256;
4. atomic rename staging thành wp-content/plugins/coffeepos.

Nếu destination đã tồn tại:

- non-directory → từ chối và preserve;
- thiếu/unreadable ownership metadata → từ chối và preserve;
- ownership schema/slug không tương thích → từ chối;
- managed version/hash khác pinned artifact → yêu cầu explicit upgrade flow;
- managed header version bị đổi/corrupt → từ chối repair tự động;
- exact managed version/hash/header → no-op.

Retry exact managed artifact vì vậy giữ file bổ sung/sentinel trong CoffeePOS directory và không đụng plugin khác.

## Journal và backward compatibility

Journal có append-only stage mới coffee_pos_provisioned và field optional coffeepos_version.

Journal Phase 4.6 ở stage woo_commerce_activated không có field mới vẫn deserialize được. Store như vậy được inspect thành needs_repair + can_retry; retry giữ/reuse existing database, WordPress và WooCommerce qua các bước ensure/verification idempotent rồi tiếp tục tới CoffeePOS artifact, không reset credential hoặc store data.

Phase 4.8 Ready là trạng thái installation: WordPress/WooCommerce baseline hợp lệ và CoffeePOS files đã được provision. Nó không khẳng định CoffeePOS active; activation state thuộc Phase 4.9.

## UI

Provisioning UI hiện hiển thị Phase 4.8, WordPress version, WooCommerce version + active state và CoffeePOS version + provisioned state. Wording ghi rõ CoffeePOS chưa active và Phase 4.9 sở hữu activation. Existing provision_wordpress command vẫn là orchestration path duy nhất.

## WordPress readiness fix phát hiện trong acceptance

Real WooCommerce-active WordPress có thể mất hơn 10 giây cho dynamic request đầu tiên sau activation/start. E2E A/B chứng minh tình trạng này vẫn xảy ra khi inactive CoffeePOS directory được tạm đưa khỏi wp-content/plugins, nên không phải CoffeePOS file presence.

Runtime timeout được tách:

    PHP HTTP process readiness = 10s
    WordPress application readiness = 45s

PHP process failure vẫn fail nhanh, còn WordPress/WooCommerce có bounded window phù hợp cho first-request initialization.

## Validation

Static/unit:

    Rust tests: 33 passed, 0 failed, 2 ignored
    cargo clippy --locked --all-targets -- -D warnings: PASS
    npm run lint:ui: PASS
    npm run build:ui: PASS

Unit coverage gồm checked-in CoffeePOS manifest schema, path escape rejection, exact layout/compatibility validation, managed retry sentinel preservation, corrupt managed tree refusal without recopy, unmanaged destination refusal, WooCommerce dependency preflight, coffeepos.provisioning cleanup allowlist, Phase 4.6 journal backward compatibility và UI serialization.

Real staged Windows E2E:

    . .\scripts\use-local-rust.ps1
    cargo test --manifest-path .\src-tauri\Cargo.toml --locked provisioning::tests::staged_runtime_provisions_twice_stops_and_cleans_temp_store -- --ignored

Final run:

    1 passed, 0 failed
    181.71s

Acceptance chứng minh fresh WordPress + WooCommerce setup vẫn hoàn tất; CoffeePOS 1.0.0 được cài từ exact staged artifact; ownership metadata được tạo; coffeepos.provisioning được cleanup; WooCommerce active nhưng CoffeePOS chưa active; WordPress health + Woo Store API vẫn usable; runtime restart/process-death/occupied-port recovery cũ vẫn pass; provisioning lần hai vẫn Ready; WordPress/WooCommerce/CoffeePOS/unrelated-plugin sentinels đều được giữ nguyên.

Development app-local-data profile hiện được xác nhận đang ở journal Phase 4.6 stage woo_commerce_activated. Nó sẽ hiển thị needs_repair cho tới khi retry provisioning qua app; Phase 4.8 không tự mutate store chỉ vì app đọc trạng thái.

## Ngoài phạm vi Phase 4.8

- CoffeePOS activation: Phase 4.9.
- CoffeePOS migrations, roles/capabilities, required settings/routes: Phase 4.9.
- Machine-health endpoint implementation/native consumption: Phase 4.10.
- Explicit CoffeePOS upgrade/adoption flow cho existing different/unmanaged plugin: update/repair phase riêng.
- Full-stack idempotency/recovery orchestration: Phase 4.11–4.12.

## Gate sang Phase 4.9

Phase 4.9 phải activate đúng managed CoffeePOS 1.0.0 artifact đã provision, xác minh WooCommerce dependency/version trước activation và dùng plugin-owned lifecycle để kiểm migrations/capabilities/settings/routes. Không coi file presence hoặc journal coffeepos_provisioned là bằng chứng plugin usable.
