# CoffeePOS Desktop — AGENTS.md

## 1. Project Overview

CoffeePOS Desktop is a cross-platform desktop application for running a self-contained local WordPress + WooCommerce + CoffeePOS environment.

The goal is to make CoffeePOS usable as a local POS appliance without requiring users to understand or manually install:

- PHP
- MariaDB
- WordPress
- WooCommerce
- Web servers
- WordPress configuration
- Database configuration

The desktop application is responsible for managing the local runtime.

The CoffeePOS WordPress plugin remains a normal WordPress/WooCommerce plugin and must remain independently usable on a normal WordPress installation.

Target platforms:

- Windows
- macOS

Primary architecture:

```text
CoffeePOS Desktop
        │
        ├── Desktop UI
        │
        ├── Runtime Manager
        │       ├── PHP
        │       └── MariaDB
        │
        └── Local WordPress
                ├── WooCommerce
                └── CoffeePOS
```

The desktop application is a distribution/runtime layer, not a replacement for WordPress.

---

# 2. Core Architectural Principles

## 2.1 CoffeePOS plugin and Desktop App are separate products

Do not move CoffeePOS business logic into the desktop application.

The CoffeePOS plugin owns:

- POS business logic
- WooCommerce integration
- Products
- Cart
- Orders
- Customers
- Coupons
- Payments
- Queue
- KDS
- Order history
- Reports
- Shifts
- Store settings
- WordPress REST APIs
- WooCommerce data access

The Desktop App owns:

- Runtime installation
- Runtime startup/shutdown
- PHP process management
- MariaDB process management
- WordPress provisioning
- Configuration
- Health checks
- Backup/restore
- Updates
- Desktop window/WebView
- Local network configuration

Never duplicate CoffeePOS domain logic in Rust or desktop code unless there is a strong platform-specific reason.

---

# 3. Recommended Technology

## Desktop

Use:

- Tauri
- Rust
- WebView-based UI

Avoid Electron unless a concrete technical requirement makes Tauri unsuitable.

The desktop UI should remain lightweight.

The application should not require Node.js, PHP, Composer, MySQL or other development tooling on the user's machine.

---

# 4. Runtime Stack

The initial runtime should contain:

```text
PHP
MariaDB
WordPress
WooCommerce
CoffeePOS
```

Avoid adding Nginx or Apache unless there is a demonstrated requirement.

For the first implementation, prefer PHP's built-in web server:

```text
php -S <host>:<port>
```

This keeps the runtime small and easier to manage.

Architecture:

```text
Tauri
  │
  ├── PHP process
  │       │
  │       └── WordPress
  │               ├── WooCommerce
  │               └── CoffeePOS
  │
  └── MariaDB process
```

The runtime must be completely controlled by the desktop application.

---

# 5. Local Server Binding

The application must support two runtime modes.

## Local-only mode

Bind to:

```text
127.0.0.1
```

Used when only the desktop application needs access.

## LAN mode

Bind to:

```text
0.0.0.0
```

Used when other devices need to access the local CoffeePOS server.

Example:

```text
POS computer
    │
    ├── CoffeePOS Desktop
    │
    └── Local WordPress
             │
             ├── POS
             ├── KDS
             ├── Customer Display
             └── Tablet Ordering
```

LAN mode must be explicit and configurable.

Do not expose the local WordPress installation to the public internet.

---

# 6. Runtime Directory Structure

Never store mutable store data inside the application installation directory.

The application installation contains immutable runtime/application files.

User/store data must be stored in the platform-specific application-data directory.

## Windows

Conceptually:

```text
Program Files/
└── CoffeePOS/
    ├── application
    └── runtime/

AppData/Local/CoffeePOS/
├── site/
├── database/
├── uploads/
├── config/
├── logs/
└── backups/
```

## macOS

Conceptually:

```text
/Applications/CoffeePOS.app/

~/Library/Application Support/CoffeePOS/
├── site/
├── database/
├── uploads/
├── config/
├── logs/
└── backups/
```

The exact implementation must use OS-appropriate application-data APIs.

---

# 7. Runtime Lifecycle

The Desktop App must own the complete lifecycle.

Startup sequence:

```text
Application launch
        ↓
Load configuration
        ↓
Check runtime files
        ↓
Check database
        ↓
Start MariaDB
        ↓
Wait for database readiness
        ↓
Start PHP
        ↓
Wait for HTTP readiness
        ↓
Check WordPress
        ↓
Check WooCommerce
        ↓
Check CoffeePOS
        ↓
Open POS
```

Shutdown sequence:

```text
User closes application
        ↓
Stop accepting new operations
        ↓
Stop PHP
        ↓
Stop MariaDB
        ↓
Persist application state
        ↓
Exit
```

Do not assume that starting a process means the service is ready.

Every service must have an explicit health/readiness check.

---

# 8. Runtime Process Management

The Runtime Manager must:

- Spawn child processes
- Track process IDs
- Capture stdout/stderr
- Write useful logs
- Detect unexpected process termination
- Restart services when appropriate
- Avoid orphan processes
- Gracefully terminate processes
- Handle Windows and macOS differences

Never use shell commands in a way that assumes a specific user's shell environment.

Prefer direct process execution with explicit arguments.

Do not depend on:

- Homebrew
- XAMPP
- MAMP
- LocalWP
- Docker
- Node.js
- Composer
- globally installed PHP
- globally installed MariaDB

The application must ship or provision the required runtime.

---

# 9. First-Run Provisioning

The first launch should automatically provision a working CoffeePOS environment.

Expected flow:

```text
CoffeePOS Desktop
        ↓
Runtime setup
        ↓
Create data directories
        ↓
Initialize MariaDB
        ↓
Create database
        ↓
Install WordPress
        ↓
Configure wp-config.php
        ↓
Install WooCommerce
        ↓
Install CoffeePOS
        ↓
Activate plugins
        ↓
Run CoffeePOS initialization
        ↓
Create initial store/admin
        ↓
Health check
        ↓
Open POS
```

The user should not need to manually visit `/wp-admin/setup-config.php`.

---

# 10. WordPress Template

Maintain a known-good WordPress template/baseline.

Conceptually:

```text
wordpress-template/
├── wp-admin/
├── wp-includes/
├── wp-content/
│   └── plugins/
│       ├── woocommerce/
│       └── coffeepos/
└── ...
```

The template is used to create a new local store installation.

Do not modify WordPress core for CoffeePOS functionality.

Do not put CoffeePOS-specific changes into:

- wp-admin
- wp-includes
- WordPress core files

All application-specific behavior must live in CoffeePOS or the desktop runtime layer.

---

# 11. WooCommerce

WooCommerce is a required dependency of CoffeePOS.

The Desktop App must verify:

```text
WordPress installed
+
WooCommerce installed
+
WooCommerce activated
+
CoffeePOS installed
+
CoffeePOS activated
```

CoffeePOS should never silently assume WooCommerce is available.

The runtime health system should report dependency failures clearly.

Example:

```text
✓ WordPress
✓ Database
✓ WooCommerce
✗ CoffeePOS
```

---

# 12. CoffeePOS Integration

The CoffeePOS plugin should expose a lightweight system-health endpoint.

Example:

```text
GET /wp-json/coffeepos/v1/system/status
```

Expected conceptual response:

```json
{
  "wordpress": true,
  "woocommerce": true,
  "coffeepos": true,
  "database": true,
  "store": {
    "name": "My Coffee"
  }
}
```

The exact endpoint and schema should be documented before implementation.

The Desktop App must use this endpoint for application-level health verification rather than attempting to reproduce WordPress/WooCommerce checks itself.

---

# 13. Desktop App UI

The initial UI should remain intentionally simple.

Primary states:

### First Run

```text
CoffeePOS

Your local POS environment is not installed.

[ Install CoffeePOS ]
```

### Starting

```text
CoffeePOS

Starting local POS...

✓ Database
✓ WordPress
• CoffeePOS

Please wait...
```

### Running

```text
CoffeePOS

● POS is running

[ Open POS ]

[ Settings ]
```

### Error

```text
CoffeePOS

Something went wrong.

Database: OK
WordPress: OK
WooCommerce: OK
CoffeePOS: ERROR

[ Repair ]
[ View Logs ]
```

Do not build a complex desktop dashboard before the runtime is stable.

---

# 14. POS WebView

The primary POS interface should remain the existing CoffeePOS web application.

The desktop application should load:

```text
http://127.0.0.1:<port>/pos
```

or the configured LAN address.

The desktop shell should not reimplement the POS UI natively.

Conceptually:

```text
Tauri Window
└── WebView
    └── CoffeePOS /pos
```

This allows CoffeePOS to remain a normal web application.

---

# 15. Authentication

The local installation should create an initial WordPress administrator/store account during provisioning.

Do not hard-code credentials.

Credentials must:

- Be generated or supplied during setup
- Be stored securely where required
- Never be written into source code
- Never be logged

The desktop application should avoid exposing WordPress admin functionality unless explicitly requested.

---

# 16. Security

Default behavior:

```text
127.0.0.1
```

LAN access must be opt-in.

Never expose:

- MariaDB directly to LAN
- MariaDB to the public internet
- WordPress debug endpoints publicly
- Internal runtime control endpoints publicly

Runtime management commands should be accessible only through the desktop application or protected local mechanisms.

Never accept arbitrary shell commands from the WebView.

The WebView must not be able to execute arbitrary system commands.

---

# 17. Backup and Restore

Store data must be separable from application/runtime files.

A backup must contain enough information to restore the store.

Conceptually:

```text
CoffeePOS Backup
├── database
├── uploads
├── CoffeePOS configuration
└── metadata/version
```

Do not blindly backup:

- PHP runtime
- MariaDB binaries
- WordPress core binaries

Backups should be portable between compatible CoffeePOS Desktop versions.

Before restore:

```text
validate backup
        ↓
check version
        ↓
stop runtime
        ↓
backup current state
        ↓
restore
        ↓
run migrations
        ↓
health check
```

Never destroy the current installation before the restore has been validated.

---

# 18. Updates

Application updates and store updates are different concerns.

## Desktop update

Updates:

```text
Tauri application
runtime binaries
desktop UI
```

Must not delete:

```text
site/
database/
uploads/
backups/
```

## CoffeePOS update

Updates:

```text
CoffeePOS plugin
```

May require:

```text
database migrations
```

Database migrations must be explicit and versioned.

Never assume a plugin update can safely overwrite arbitrary database state.

---

# 19. Version Compatibility

Maintain explicit versions for:

```text
Desktop App
Runtime
PHP
MariaDB
WordPress
WooCommerce
CoffeePOS
Database schema
```

Example:

```json
{
  "desktop": "1.0.0",
  "runtime": "1.0.0",
  "wordpress": "6.x",
  "woocommerce": "x.x",
  "coffeepos": "1.0.0",
  "database_schema": 1
}
```

The actual schema should evolve during implementation.

Do not rely on package filenames alone to determine compatibility.

---

# 20. Logging

Logs must be useful for diagnosing problems without exposing secrets.

Separate:

```text
logs/
├── application.log
├── runtime.log
├── php.log
├── database.log
└── wordpress.log
```

Never log:

- passwords
- API keys
- authentication cookies
- payment credentials
- full sensitive customer data

Provide a way to open/export diagnostic logs from the desktop app.

---

# 21. Payment Architecture

CoffeePOS may support payment methods such as:

- Cash
- QR transfer
- Other WooCommerce-compatible methods

Do not make the desktop runtime responsible for payment business logic.

Payment state belongs to CoffeePOS/WooCommerce.

The desktop app only provides the runtime environment.

---

# 22. Local Network Architecture

The intended future architecture is:

```text
                    Local Wi-Fi
                        │
          ┌─────────────┼─────────────┐
          │             │             │
          ▼             ▼             ▼
      POS Desktop     KDS        Tablet
          │
          ▼
    CoffeePOS Server
          │
    ┌─────┴─────┐
    │           │
WooCommerce   CoffeePOS
    │
 MariaDB
```

The POS computer is the local server.

Other devices connect through the local network.

This architecture should be considered when designing:

- URLs
- authentication
- CORS
- WebSocket/event architecture
- service discovery
- QR ordering
- KDS
- customer display

Do not introduce unnecessary cloud dependencies.

---

# 23. Local Service Discovery

Do not make users manually type IP addresses when avoidable.

Future versions may support:

```text
coffee.local
```

or another local hostname.

However, service discovery should be introduced only after the basic runtime works reliably.

Do not over-engineer discovery in the first milestone.

---

# 24. Development Environment

Development should be possible without installing the production runtime globally.

The repository should provide clear commands/scripts for:

```text
development
build
package
test
lint
```

Do not require the developer to manually reproduce the user's production installation.

The project must distinguish:

```text
Development runtime
```

from:

```text
Bundled production runtime
```

---

# 25. Repository Structure

Start with a structure similar to:

```text
coffeepos-desktop/
├── AGENTS.md
├── README.md
├── src/
│   ├── ...
│
├── src-tauri/
│   ├── src/
│   │   ├── main.rs
│   │   ├── runtime/
│   │   ├── wordpress/
│   │   ├── database/
│   │   ├── health/
│   │   ├── backup/
│   │   └── config/
│   └── tauri.conf.json
│
├── ui/
│   ├── ...
│
├── runtime/
│   └── development/
│
├── wordpress-template/
│   └── ...
│
├── scripts/
│   ├── ...
│
└── docs/
    ├── ARCHITECTURE.md
    ├── RUNTIME.md
    ├── PROVISIONING.md
    ├── BACKUP.md
    └── DEVELOPMENT.md
```

The exact structure may evolve.

Do not create unnecessary abstractions before the first working vertical slice exists.

---

# 26. Implementation Strategy

Build vertically, not horizontally.

`docs/ROADMAP.md` is the source of truth for phase numbering, scope, status and Definition of Done. Do not use an older broad Phase 4/5/6/7/8 breakdown when planning new work.

Current Windows-first status:

- Phase 1 — Desktop Shell: complete.
- Phase 2 — Runtime Manager: complete.
- Phase 3 — WordPress Provisioning: complete.
- Phase 4.1 — Provisioning UI: complete.
- Phase 4.2 — WordPress runtime UX: complete.
- Phase 4.3 — Open WordPress test: complete Windows-first.
- Phase 4.4 — WooCommerce artifact: complete Windows-first.
- Phase 4.5 — WooCommerce provisioning: complete Windows-first.
- Phase 4.6 — WooCommerce activation: complete Windows-first.
- Phase 4.7 — CoffeePOS artifact: complete Windows-first.
- Phase 4.8 — CoffeePOS provisioning: complete Windows-first.
- Phase 4.9 — CoffeePOS activation: complete Windows-first.
- Phase 4.10 — CoffeePOS health endpoint: complete Windows-first.
- Phase 4.11 — Full install idempotency: complete Windows-first.
- Phase 4.12 — First-run recovery: complete Windows-first.
- Phase 5.1 — UI shell and navigation: complete Windows-first.
- Phase 5.2 — Store and account onboarding: complete Windows-first.
- Phase 5.3 — Home and app settings: implemented; lightweight validation passed, manual Windows acceptance pending.
- Next implementation milestone after Phase 5.3 acceptance: Phase 5.4 — Open POS and login.

From Phase 4 onward, work is intentionally split into small independently verifiable milestones:

| Phase | Deliverable |
| --- | --- |
| 4.1 | Provisioning UI |
| 4.2 | WordPress runtime UX |
| 4.3 | Open WordPress test |
| 4.4 | WooCommerce artifact |
| 4.5 | WooCommerce provisioning |
| 4.6 | WooCommerce activation |
| 4.7 | CoffeePOS artifact |
| 4.8 | CoffeePOS provisioning |
| 4.9 | CoffeePOS activation |
| 4.10 | CoffeePOS health endpoint |
| 4.11 | Full install idempotency |
| 4.12 | First-run recovery |
| 5.1 | UI shell and navigation |
| 5.2 | Store and account onboarding |
| 5.3 | Home and app settings |
| 5.4 | Open POS and login |
| 5.5 | Daily startup |
| 5.6 | Minimize, exit and shutdown |
| 6.1 | Health diagnostics |
| 6.2 | Repair flow |
| 6.3 | Log viewer/export |
| 7.1 | Backup format |
| 7.2 | Database backup |
| 7.3 | Uploads/config backup |
| 7.4 | Restore |
| 8.1 | LAN bind |
| 8.2 | LAN address UI |
| 8.3 | LAN security |
| 9.1 | Runtime bundle |
| 9.2 | Windows installer |
| 9.3 | Fresh-machine test |
| 9.4 | Upgrade safety |

Every phase/subphase must finish with implementation, appropriate tests/lint/build, a real runnable flow on the target platform, failure/retry/cleanup validation where applicable, and documentation of what remains outside scope. Code compiling by itself is not enough to mark a phase complete.

Do not start a later plugin/distribution slice merely because its code can be written independently. Preserve the vertical ordering and gates in `docs/ROADMAP.md`; for example, Phase 4.1–4.3 must make WordPress setup testable from the app before WooCommerce work starts.

---

# 27. Important Engineering Rule

Do not implement everything at once.

Every phase must produce a runnable system.

Bad:

```text
Build complete architecture
↓
Implement 30 abstractions
↓
Try running WordPress
```

Preferred:

```text
Tauri
↓
spawn PHP
↓
PHP serves WordPress
↓
add MariaDB
↓
WordPress works
↓
WooCommerce works
↓
CoffeePOS works
↓
WebView works
```

A working vertical slice is more important than theoretical architecture.

---

# 28. Error Handling

Errors must be actionable.

Bad:

```text
Runtime failed.
```

Good:

```text
MariaDB failed to start.

Possible causes:
- Database directory is locked
- Port is already in use
- Previous MariaDB process did not exit correctly

[ View Logs ]
[ Retry ]
```

Errors should include:

- Component
- Operation
- Human-readable explanation
- Technical diagnostic information in logs
- Recovery action where possible

---

# 29. Port Management

Do not hard-code a single port without checking availability.

Prefer:

```text
127.0.0.1:<available-port>
```

The selected port must be stored in configuration for the current installation.

For LAN mode, the application should expose the actual reachable address.

Never assume:

```text
localhost:8080
```

is always available.

---

# 30. Database Management

MariaDB data belongs to the store installation.

Never recreate the database every time the application starts.

Startup means:

```text
open existing database
```

not:

```text
create fresh database
```

Database initialization must be idempotent.

Running provisioning twice must not destroy the existing store.

---

# 31. Idempotency

All provisioning operations should be safe to retry.

Examples:

```text
ensure_database()
ensure_wordpress()
ensure_woocommerce()
ensure_coffeepos()
ensure_admin()
ensure_configuration()
```

Prefer "ensure" semantics over destructive setup operations.

---

# 32. Do Not Depend on LocalWP

LocalWP can be useful during research/prototyping, but it is not a required production dependency.

The final CoffeePOS Desktop product should be able to run independently.

Do not build architecture around LocalWP-specific internals.

---

# 33. Do Not Rewrite CoffeePOS

The existing CoffeePOS plugin is the source of truth for POS functionality.

Do not rewrite CoffeePOS into:

- React Native
- Swift
- C#
- Rust
- Electron-specific UI
- Tauri-specific UI

unless explicitly required for a future feature.

The desktop application should make the existing WordPress application easier to install and run.

---

# 34. Native vs Web Responsibilities

Native/Desktop layer:

```text
Process lifecycle
Files
Runtime
Database process
Networking
Installer
OS integration
Backup
Updates
```

Web layer:

```text
POS UI
WooCommerce
Products
Orders
Customers
Coupons
KDS
Reports
Shifts
Business rules
```

Never mix these responsibilities without a clear reason.

---

# 35. Coding Style

Prefer:

- Small modules
- Explicit interfaces
- Clear error types
- Minimal dependencies
- Platform-specific code isolated behind interfaces
- Testable runtime management
- Deterministic provisioning

Avoid:

- Giant manager classes
- Global mutable state
- Shell-script-driven architecture
- Hidden background processes
- Magic paths
- Hard-coded credentials
- Hard-coded ports
- Unnecessary frameworks

---

# 36. AI/Codex Development Rules

Codex must read this file before modifying the project.

Before implementing a significant feature:

1. Inspect the existing repository.
2. Identify the relevant architecture boundary.
3. Reuse existing abstractions.
4. Avoid introducing a new abstraction if an existing one is sufficient.
5. Make the smallest coherent change.
6. Verify the change.
7. Update documentation when architecture changes.

Do not rewrite working modules merely to make them stylistically different.

Do not introduce large dependencies without explaining why they are necessary.

Do not silently change the runtime architecture.

If a proposed implementation conflicts with this document, stop and explain the conflict before making a large architectural change.

---

# 37. Definition of Done

A feature is not complete merely because the code compiles.

For runtime features, verify:

```text
✓ Starts
✓ Stops
✓ Restarts
✓ Handles failure
✓ Does not orphan processes
✓ Works with existing store data
✓ Produces useful logs
✓ Works on target platforms where applicable
```

For provisioning features:

```text
✓ Fresh installation works
✓ Running twice is safe
✓ Existing installation is preserved
✓ Failure can be retried
```

For CoffeePOS integration:

```text
✓ WordPress works
✓ WooCommerce works
✓ CoffeePOS works
✓ POS opens
✓ Existing CoffeePOS functionality remains unchanged
```

---

# 38. Long-Term Product Direction

CoffeePOS Desktop should eventually provide:

```text
┌───────────────────────────────────────┐
│ CoffeePOS                             │
│                                       │
│ ● POS Running                         │
│                                       │
│ [ Open POS ]                          │
│                                       │
│ Store                                 │
│ My Coffee                             │
│                                       │
│ Devices                               │
│ 1 POS                                 │
│ 1 KDS                                 │
│ 2 Tablets                             │
│                                       │
│ System                                │
│ ✓ WordPress                           │
│ ✓ WooCommerce                         │
│ ✓ CoffeePOS                           │
│ ✓ Database                            │
│                                       │
│ [ Backup ] [ Settings ]               │
└───────────────────────────────────────┘
```

The end-user experience should feel like installing a dedicated POS product, even though the underlying application is:

```text
Tauri
+
PHP
+
MariaDB
+
WordPress
+
WooCommerce
+
CoffeePOS
```

The abstraction should be invisible to the café operator.

---

# 39. Primary Success Criterion

The most important success criterion is:

> A non-technical café operator should be able to install CoffeePOS Desktop on a fresh Windows or macOS machine and start using the POS without manually installing or configuring WordPress, WooCommerce, PHP, MariaDB, or a web server.

Everything in this project should serve that goal.
