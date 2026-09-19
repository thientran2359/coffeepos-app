import { invoke, isTauri } from "@tauri-apps/api/core";
import "./styles.css";

interface ShellInfo {
  version: string;
  data_dir: string;
  config: {
    schema_version: number;
    store_name: string;
    bind_host: string;
    startup_view: "home" | "settings" | "diagnostics";
    setup_admin_username?: string;
    setup_admin_email?: string;
  };
}

interface SetupInfo {
  store_name: string;
  admin_username: string;
  admin_email: string;
  password_configured: boolean;
  editable: boolean;
}

interface RuntimeErrorInfo {
  component: string;
  operation: string;
  message: string;
  recovery: string;
}

interface CoffeePosHealthPayload {
  schema_version: number;
  status: "healthy" | "degraded";
  wordpress: boolean;
  woocommerce: boolean;
  coffeepos: boolean;
  database: boolean;
  versions: {
    wordpress: string;
    woocommerce: string;
    coffeepos: string;
    coffeepos_schema: string;
  };
  store: { name: string };
  pos_path: string;
}

interface CoffeePosHealthInfo {
  state: "unavailable" | "checking" | "healthy" | "degraded" | "failed";
  failure_kind: "transport_bootstrap" | "authentication" | "contract" | null;
  payload: CoffeePosHealthPayload | null;
  error: RuntimeErrorInfo | null;
}

interface RuntimeInfo {
  state: "not_installed" | "installing" | "stopped" | "starting" | "running" | "stopping";
  runtime_version: string | null;
  php_version: string | null;
  web_server_version: string | null;
  mariadb_version: string | null;
  database_port: number | null;
  http_port: number | null;
  database_pid: number | null;
  php_pid: number | null;
  web_server_pid: number | null;
  wordpress_health: "unavailable" | "checking" | "healthy" | "unhealthy";
  wordpress_error: RuntimeErrorInfo | null;
  coffeepos_health: CoffeePosHealthInfo;
  last_error: RuntimeErrorInfo | null;
}

type ComponentHealthState = "unavailable" | "healthy" | "unhealthy" | "unknown";

interface ComponentHealthInfo {
  state: ComponentHealthState;
  error: RuntimeErrorInfo | null;
}

interface HealthDiagnosticsInfo {
  runtime_state: RuntimeInfo["state"];
  database: ComponentHealthInfo;
  php: ComponentHealthInfo;
  wordpress: ComponentHealthInfo;
  woocommerce: ComponentHealthInfo;
  coffeepos: ComponentHealthInfo;
}

type HealthComponent = "database" | "php" | "wordpress" | "woocommerce" | "coffeepos";

type ProvisioningState = "not_installed" | "installing" | "ready" | "needs_repair";

interface ProvisioningInfo {
  state: ProvisioningState;
  wordpress_version: string;
  woocommerce_version: string;
  woocommerce_active: boolean;
  coffeepos_version: string;
  coffeepos_active: boolean;
  admin_username: string | null;
  can_retry: boolean;
  last_error: RuntimeErrorInfo | null;
}

type InstalledView = "home" | "settings" | "diagnostics";
type SystemSection = "diagnostics" | "repair" | "logs" | "backup";
type RepairClassification = "repairable" | "requires_input" | "blocked";

interface RepairItem {
  id: string;
  component: string;
  target: string;
  classification: RepairClassification;
  action: string;
  reason: string;
  impact: string;
  requires_runtime_stop: boolean;
  input_kind: "admin_password" | null;
}

interface RepairPlan {
  plan_id: string;
  generated_at: number;
  store_state: ProvisioningState;
  runtime_was_running: boolean;
  items: RepairItem[];
  can_apply: boolean;
}

interface RepairItemResult {
  id: string;
  status: "repaired" | "skipped" | "blocked";
  message: string;
}

interface RepairApplyResult {
  plan_id: string;
  status: "repaired" | "partial" | "stale";
  items: RepairItemResult[];
  provisioning_info: ProvisioningInfo;
  health_diagnostics: HealthDiagnosticsInfo | null;
  last_error: RuntimeErrorInfo | null;
}

interface LogCatalogEntry {
  id: string;
  label: string;
  exists: boolean;
  size_bytes: number;
  modified_at: number | null;
}

interface LogCatalog {
  generated_at: number;
  logs: LogCatalogEntry[];
}

interface LogPage {
  log_id: string;
  lines: string[];
  older_cursor: string | null;
  has_older: boolean;
  truncated: boolean;
  redaction_count: number;
}

interface SupportBundleResult {
  status: "exported" | "cancelled";
  destination?: string | null;
}

interface BackupErrorInfo {
  component: string;
  action: string;
  code: string;
  message: string;
  recovery: string;
}

interface BackupStatus {
  operation_id: string | null;
  stage: string;
  processed_files: number;
  estimated_files: number;
  processed_bytes: number;
  estimated_bytes: number;
  started_at: number | null;
  finished_at: number | null;
  cancelled: boolean;
  succeeded: boolean;
  failed: boolean;
  warnings: string[];
  last_error: BackupErrorInfo | null;
}

interface BackupResult {
  operation_id: string;
  status: string;
}

type BackupView = "landing" | "password" | "progress" | "success" | "failure";

interface RestoreInspection {
  candidate_id?: string;
  restore_candidate_id?: string;
  backup?: unknown;
  backup_id?: string;
  store_name?: string;
  created_at?: number | string | null;
  wordpress_version?: string;
  woocommerce_version?: string;
  coffeepos_version?: string;
  uploads_bytes?: number;
  compatible?: boolean;
  compatibility?: unknown;
  warnings?: string[];
  manifest?: unknown;
  [key: string]: unknown;
}

interface RestoreStatus {
  operation_id: string | null;
  stage: string;
  started_at?: number | null;
  finished_at?: number | null;
  succeeded?: boolean;
  failed?: boolean;
  cancelled?: boolean;
  rolled_back?: boolean;
  needs_recovery?: boolean;
  recovery_required?: boolean;
  original_state?: "existing_store" | "no_previous_store" | null;
  warnings?: string[];
  last_error?: BackupErrorInfo | RuntimeErrorInfo | null;
  [key: string]: unknown;
}

interface RestoreResult {
  operation_id?: string;
  status?: string;
}

type RestoreView = "password" | "review" | "progress" | "success" | "failure" | "recovery";
type RestoreEntrySource = "fresh" | "installed";

type SetupStep = "welcome" | "details" | "review" | "progress" | "complete";

function element<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (!node) throw new Error(`Missing UI element: ${id}`);
  return node as T;
}

function installRestoreUi(): void {
  const setupWelcomeActions = document.querySelector<HTMLElement>("#setup-welcome .actions");
  if (setupWelcomeActions && !document.getElementById("setup-restore")) {
    const button = document.createElement("button");
    button.id = "setup-restore";
    button.type = "button";
    button.className = "secondary";
    button.textContent = "Khôi phục từ bản sao lưu";
    setupWelcomeActions.append(button);
  }

  const backupCardNode = document.getElementById("backup");
  const backupLead = backupCardNode?.querySelector<HTMLElement>(".lead");
  if (backupCardNode && backupLead && !document.getElementById("restore-installed-start")) {
    const actions = document.createElement("div");
    actions.className = "actions";
    actions.id = "restore-entry-actions";
    const button = document.createElement("button");
    button.id = "restore-installed-start";
    button.type = "button";
    button.className = "secondary";
    button.textContent = "Khôi phục từ bản sao lưu";
    actions.append(button);
    backupLead.insertAdjacentElement("afterend", actions);
    const titleNode = document.getElementById("backup-title");
    if (titleNode) titleNode.textContent = "Sao lưu và khôi phục";
  }

  if (document.getElementById("restore-screen")) return;
  const main = element<HTMLElement>("main-content");
  const screen = document.createElement("section");
  screen.id = "restore-screen";
  screen.className = "setup-screen";
  screen.hidden = true;
  screen.setAttribute("aria-labelledby", "restore-title");
  screen.setAttribute("aria-busy", "false");
  screen.innerHTML = `
    <div class="card backup-card" id="restore-card">
      <div class="section-heading compact-heading">
        <div><span class="section-label">KHÔI PHỤC</span><h2 id="restore-title" tabindex="-1">Khôi phục cửa hàng</h2></div>
        <span id="restore-state" class="state-badge">Sẵn sàng</span>
      </div>

      <div data-restore-view="password">
        <p class="lead">Chọn một bản sao lưu CoffeePOS và kiểm tra tính toàn vẹn, phiên bản và khả năng tương thích trước khi thay đổi cửa hàng.</p>
        <form id="restore-password-form" class="backup-password-form" novalidate>
          <div class="field-group">
            <label for="restore-password">Mật khẩu bản sao lưu</label>
            <input id="restore-password" type="password" maxlength="128" autocomplete="current-password" required />
          </div>
          <p class="hint">Cửa sổ Open của Windows sẽ chọn file. Đường dẫn file và dữ liệu giải mã không được đưa vào giao diện.</p>
          <p id="restore-password-error" class="field-error" role="alert" hidden></p>
          <div class="actions split-actions">
            <button id="restore-password-cancel" class="secondary" type="button">Quay lại</button>
            <button id="restore-inspect" type="submit">Chọn và kiểm tra bản sao lưu</button>
          </div>
        </form>
      </div>

      <div data-restore-view="review" hidden>
        <p class="lead">Bản sao lưu đã vượt qua bước kiểm tra read-only. Xác nhận thông tin an toàn bên dưới trước khi khôi phục.</p>
        <dl class="review-list">
          <dt>Cửa hàng</dt><dd id="restore-review-store">—</dd>
          <dt>Tạo lúc</dt><dd id="restore-review-created">—</dd>
          <dt>WordPress</dt><dd id="restore-review-wordpress">—</dd>
          <dt>WooCommerce</dt><dd id="restore-review-woocommerce">—</dd>
          <dt>CoffeePOS</dt><dd id="restore-review-coffeepos">—</dd>
          <dt>Uploads</dt><dd id="restore-review-uploads">—</dd>
          <dt>Tương thích</dt><dd id="restore-review-compatibility">—</dd>
        </dl>
        <div id="restore-unmanaged-warning" class="backup-result backup-result-error" hidden>
          <strong>Có mã site không được quản lý</strong>
          <p>Bản sao lưu có dấu hiệu plugin, theme hoặc mã site tùy chỉnh không thuộc gói managed. CoffeePOS chỉ dựng lại mã managed tương thích; hãy kiểm tra chức năng tùy chỉnh sau khi khôi phục.</p>
        </div>
        <p id="restore-review-impact" class="hint"></p>
        <p id="restore-review-error" class="error" role="alert" hidden></p>
        <div class="actions split-actions">
          <button id="restore-review-cancel" class="secondary" type="button">Hủy</button>
          <button id="restore-apply" type="button">Khôi phục</button>
        </div>
      </div>

      <div data-restore-view="progress" hidden>
        <div class="backup-progress-copy">
          <strong>Đang khôi phục…</strong>
          <p id="restore-progress-stage" role="status" aria-live="polite">Đang chuẩn bị giao dịch khôi phục.</p>
          <progress id="restore-progress" max="1">Đang xử lý</progress>
          <p id="restore-progress-detail" class="hint">CoffeePOS đang giữ cửa hàng ở trạng thái an toàn trong khi kiểm tra và chuyển dữ liệu.</p>
        </div>
        <p id="restore-close-guidance" class="hint">Không tắt máy trong khi dữ liệu đang được chuyển.</p>
        <div class="actions"><button id="restore-cancel" class="secondary" type="button">Hủy khôi phục</button></div>
        <p id="restore-progress-status" class="hint" role="status" aria-live="polite"></p>
      </div>

      <div data-restore-view="success" hidden>
        <div class="backup-result">
          <strong>Khôi phục hoàn tất</strong>
          <p>CoffeePOS đã kiểm tra dữ liệu và trạng thái cửa hàng sau khôi phục.</p>
        </div>
        <div class="actions"><button id="restore-open-home" type="button">Mở Tổng quan</button></div>
      </div>

      <div data-restore-view="failure" hidden>
        <div class="backup-result backup-result-error" role="alert">
          <strong id="restore-failure-title">Không thể khôi phục</strong>
          <p id="restore-failure-message">CoffeePOS chưa thể hoàn tất thao tác khôi phục.</p>
          <p id="restore-failure-recovery" class="hint"></p>
        </div>
        <div class="actions split-actions">
          <button id="restore-failure-back" class="secondary" type="button">Quay lại</button>
          <button id="restore-retry" type="button">Thử lại</button>
        </div>
      </div>

      <div data-restore-view="recovery" hidden>
        <div class="backup-result backup-result-error" role="alert">
          <strong>Khôi phục cần xử lý</strong>
          <p>CoffeePOS đang giữ trạng thái cửa hàng và dữ liệu phục hồi để tránh tiếp tục vận hành trên trạng thái chưa được xác minh.</p>
        </div>
        <details class="technical-details">
          <summary>Xem chi tiết kỹ thuật</summary>
          <p id="restore-recovery-details" class="hint"></p>
        </details>
        <div class="actions"><button id="restore-recovery-refresh" type="button">Kiểm tra lại trạng thái</button></div>
      </div>
    </div>`;
  main.append(screen);
}

installRestoreUi();

const bootstrapPanel = element("bootstrap");
const title = element("status-title");
const description = element("status-description");
const retry = element<HTMLButtonElement>("retry");
const setup = element("setup");
const setupTitle = element<HTMLElement>("setup-title");
const setupPanels = Array.from(document.querySelectorAll<HTMLElement>("[data-setup-panel]"));
const setupBegin = element<HTMLButtonElement>("setup-begin");
const setupBack = element<HTMLButtonElement>("setup-back");
const setupEdit = element<HTMLButtonElement>("setup-edit");
const setupRetry = element<HTMLButtonElement>("setup-retry");
const setupCompleteTitle = element<HTMLElement>("setup-complete-title");
const completeContinue = element<HTMLButtonElement>("complete-continue");
const completeCopyPassword = element<HTMLButtonElement>("complete-copy-password");
const completeCopyStatus = element("complete-copy-status");
const installedShell = element("installed-shell");

const navButtons = Array.from(document.querySelectorAll<HTMLButtonElement>("[data-view]"));
const viewPanels = Array.from(document.querySelectorAll<HTMLElement>("[data-view-panel]"));

const homeTitle = element<HTMLElement>("home-title");
const homeStoreName = element("home-store-name");
const homeState = element("home-state");
const homeStatus = element("home-status");
const homeDetail = element("home-detail");
const homeAction = element<HTMLButtonElement>("home-action");
const homeOpenStatus = element("home-open-status");
const homeDiagnostics = element<HTMLButtonElement>("home-diagnostics");

const name = element<HTMLInputElement>("store-name");
const adminUsername = element<HTMLInputElement>("admin-username");
const adminEmail = element<HTMLInputElement>("admin-email");
const adminPassword = element<HTMLInputElement>("admin-password");
const adminPasswordConfirm = element<HTMLInputElement>("admin-password-confirm");
const save = element<HTMLButtonElement>("save");
const message = element("save-status");
const passwordHint = element("password-hint");
const setupForm = element<HTMLFormElement>("setup-config-form");
const provisioningState = element("provisioning-state");
const provisioningStatus = element("provisioning-status");
const provisioningError = element("provisioning-error");
const provisioningDetails = element("provisioning-details");
const provisioningWordPress = element("provisioning-wordpress");
const provisioningWooCommerce = element("provisioning-woocommerce");
const provisioningCoffeePos = element("provisioning-coffeepos");
const provisioningAdmin = element("provisioning-admin");
const provisionWordPress = element<HTMLButtonElement>("provision-wordpress");
const settingsAdminUsername = element("settings-admin-username");
const settingsCopyPassword = element<HTMLButtonElement>("settings-copy-password");
const settingsCopyStatus = element("settings-copy-status");
const appSettingsForm = element<HTMLFormElement>("app-settings-form");
const settingsStartupView = element<HTMLSelectElement>("startup-view");
const settingsSave = element<HTMLButtonElement>("settings-save");
const settingsSaveStatus = element("settings-save-status");

const runtimeDescription = element("runtime-description");
const runtimeStart = element<HTMLButtonElement>("runtime-start");
const runtimeStop = element<HTMLButtonElement>("runtime-stop");
const runtimeRestart = element<HTMLButtonElement>("runtime-restart");
const openWordPress = element<HTMLButtonElement>("open-wordpress");
const openWordPressStatus = element("open-wordpress-status");
const wordpressHealth = element("wordpress-health");
const wordpressHealthError = element("wordpress-health-error");
const coffeeposHealth = element("coffeepos-health");
const coffeeposHealthError = element("coffeepos-health-error");
const coffeeposHealthDetails = element("coffeepos-health-details");
const healthSummaryState = element("health-summary-state");
const healthSummary = element("health-summary");
const healthRecheck = element<HTMLButtonElement>("health-recheck");
const healthCheckStatus = element("health-check-status");
const systemSectionButtons = Array.from(document.querySelectorAll<HTMLButtonElement>("[data-system-section]"));
const systemSectionPanels = Array.from(document.querySelectorAll<HTMLElement>("[data-system-panel]"));
const repairSummaryState = element("repair-summary-state");
const repairSummary = element("repair-summary");
const repairList = element("repair-list");
const repairAdminInput = element("repair-admin-input");
const repairAdminPassword = element<HTMLInputElement>("repair-admin-password");
const repairAdminPasswordConfirm = element<HTMLInputElement>("repair-admin-password-confirm");
const repairAdminError = element("repair-admin-error");
const repairError = element("repair-error");
const repairApply = element<HTMLButtonElement>("repair-apply");
const repairInspect = element<HTMLButtonElement>("repair-inspect");
const repairOpenDiagnostics = element<HTMLButtonElement>("repair-open-diagnostics");
const repairStatus = element("repair-status");
const logsPanel = element("logs");
const logsReadState = element("logs-read-state");
const logSource = element<HTMLSelectElement>("log-source");
const logSourceStatus = element("log-source-status");
const logCurrentLabel = element("log-current-label");
const logCurrentMeta = element("log-current-meta");
const logRefreshStatus = element("log-refresh-status");
const logEmpty = element("log-empty");
const logReadError = element("log-read-error");
const logReadErrorTitle = element("log-read-error-title");
const logReadErrorMessage = element("log-read-error-message");
const logReadErrorDetails = element<HTMLDetailsElement>("log-read-error-details");
const logReadErrorTechnical = element("log-read-error-technical");
const logContent = element<HTMLPreElement>("log-content");
const logLoadOlder = element<HTMLButtonElement>("log-load-older");
const logRefresh = element<HTMLButtonElement>("log-refresh");
const logExport = element<HTMLButtonElement>("log-export");
const logExportStatus = element("log-export-status");
const backupCard = element("backup");
const backupState = element("backup-state");
const backupViews = Array.from(document.querySelectorAll<HTMLElement>("[data-backup-view]"));
const backupLastSuccess = element("backup-last-success");
const backupLastSuccessTime = element("backup-last-success-time");
const backupCreate = element<HTMLButtonElement>("backup-create");
const backupLandingStatus = element("backup-landing-status");
const backupPasswordForm = element<HTMLFormElement>("backup-password-form");
const backupPassword = element<HTMLInputElement>("backup-password");
const backupPasswordConfirm = element<HTMLInputElement>("backup-password-confirm");
const backupPasswordError = element("backup-password-error");
const backupPasswordCancel = element<HTMLButtonElement>("backup-password-cancel");
const backupChooseDestination = element<HTMLButtonElement>("backup-choose-destination");
const backupProgressTitle = element("backup-progress-title");
const backupProgressStage = element("backup-progress-stage");
const backupProgress = element<HTMLProgressElement>("backup-progress");
const backupProgressCount = element("backup-progress-count");
const backupCancel = element<HTMLButtonElement>("backup-cancel");
const backupProgressStatus = element("backup-progress-status");
const backupOpenFolder = element<HTMLButtonElement>("backup-open-folder");
const backupCreateAnother = element<HTMLButtonElement>("backup-create-another");
const backupSuccessWarning = element("backup-success-warning");
const backupSuccessStatus = element("backup-success-status");
const backupFailureMessage = element("backup-failure-message");
const backupFailureRecovery = element("backup-failure-recovery");
const backupRetry = element<HTMLButtonElement>("backup-retry");
const setupRestore = element<HTMLButtonElement>("setup-restore");
const restoreInstalledStart = element<HTMLButtonElement>("restore-installed-start");
const restoreScreen = element<HTMLElement>("restore-screen");
const restoreCard = element<HTMLElement>("restore-card");
const restoreViews = Array.from(document.querySelectorAll<HTMLElement>("[data-restore-view]"));
const restoreState = element("restore-state");
const restorePasswordForm = element<HTMLFormElement>("restore-password-form");
const restorePassword = element<HTMLInputElement>("restore-password");
const restorePasswordError = element("restore-password-error");
const restorePasswordCancel = element<HTMLButtonElement>("restore-password-cancel");
const restoreInspect = element<HTMLButtonElement>("restore-inspect");
const restoreReviewStore = element("restore-review-store");
const restoreReviewCreated = element("restore-review-created");
const restoreReviewWordPress = element("restore-review-wordpress");
const restoreReviewWooCommerce = element("restore-review-woocommerce");
const restoreReviewCoffeePos = element("restore-review-coffeepos");
const restoreReviewUploads = element("restore-review-uploads");
const restoreReviewCompatibility = element("restore-review-compatibility");
const restoreUnmanagedWarning = element("restore-unmanaged-warning");
const restoreReviewImpact = element("restore-review-impact");
const restoreReviewError = element("restore-review-error");
const restoreReviewCancel = element<HTMLButtonElement>("restore-review-cancel");
const restoreApply = element<HTMLButtonElement>("restore-apply");
const restoreProgressStage = element("restore-progress-stage");
const restoreProgressDetail = element("restore-progress-detail");
const restoreCloseGuidance = element("restore-close-guidance");
const restoreCancel = element<HTMLButtonElement>("restore-cancel");
const restoreProgressStatus = element("restore-progress-status");
const restoreOpenHome = element<HTMLButtonElement>("restore-open-home");
const restoreFailureTitle = element("restore-failure-title");
const restoreFailureMessage = element("restore-failure-message");
const restoreFailureRecovery = element("restore-failure-recovery");
const restoreFailureBack = element<HTMLButtonElement>("restore-failure-back");
const restoreRetry = element<HTMLButtonElement>("restore-retry");
const restoreRecoveryDetails = element("restore-recovery-details");
const restoreRecoveryRefresh = element<HTMLButtonElement>("restore-recovery-refresh");

let provisioningBusy = false;
let runtimeBusy = false;
let currentProvisioning: ProvisioningInfo | null = null;
let currentRuntime: RuntimeInfo | null = null;
let currentSetupInfo: SetupInfo | null = null;
let currentView: InstalledView = "home";
let preferredStartupView: InstalledView = "home";
let currentSetupStep: SetupStep = "welcome";
let provisioningAction: "provision" | "refresh" = "refresh";
let setupProfileBusy = false;
let completionPending = false;
let settingsBusy = false;
let runtimeTransition: "starting" | "stopping" | "checking" | null = null;
let runtimeLoadError: string | null = null;
let homeActionKind: "start" | "retry_health" | "refresh" | "open_pos" | null = null;
let posOpenBusy = false;
let bootstrapBusy = false;
let diagnosticsBusy = false;
let runtimeRefreshBusy = false;
let runtimeMaintenanceBusy = false;
let currentDiagnostics: HealthDiagnosticsInfo | null = null;
let currentSystemSection: SystemSection = "diagnostics";
let currentRepairPlan: RepairPlan | null = null;
let repairOperation: "inspect" | "apply" | null = null;
let repairRouteRequired = false;
let currentLogCatalog: LogCatalog | null = null;
let currentLogId: string | null = null;
let currentLogLines: string[] = [];
let currentLogOlderCursor: string | null = null;
let currentLogHasOlder = false;
let currentLogTruncated = false;
let currentLogRedactionCount = 0;
let logOperation: "catalog" | "read" | "older" | null = null;
let logExportBusy = false;
let currentBackupStatus: BackupStatus | null = null;
let currentBackupView: BackupView = "landing";
let backupOperation: "create" | "cancel" | "open_folder" | null = null;
let backupPollTimer: number | null = null;
let backupStatusRefreshBusy = false;
let currentRestoreInspection: RestoreInspection | null = null;
let currentRestoreStatus: RestoreStatus | null = null;
let currentRestoreView: RestoreView = "password";
let restoreEntrySource: RestoreEntrySource = "installed";
let restoreOperation: "inspect" | "apply" | "cancel" | null = null;
let restorePollTimer: number | null = null;
let restoreStatusRefreshBusy = false;

const LOG_PAGE_MAX_LINES = 200;
const LOG_VIEW_MAX_LINES = 1000;
const BACKUP_POLL_INTERVAL_MS = 750;
const RESTORE_POLL_INTERVAL_MS = 750;

function getLogCatalog(): Promise<LogCatalog> {
  return invoke<LogCatalog>("get_log_catalog");
}

function readLogPage(logId: string, cursor: string | null, direction: "tail" | "older"): Promise<LogPage> {
  return invoke<LogPage>("read_log_page", {
    logId,
    cursor,
    direction,
    maxLines: LOG_PAGE_MAX_LINES,
  });
}

function exportSupportBundle(): Promise<SupportBundleResult> {
  return invoke<SupportBundleResult>("export_support_bundle");
}

function nativeErrorText(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  try {
    return JSON.stringify(error);
  } catch {
    return String(error);
  }
}

function recordValue(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function firstString(...values: unknown[]): string {
  for (const value of values) {
    if (typeof value === "string" && value.trim()) return value.trim();
  }
  return "";
}

function firstFiniteNumber(...values: unknown[]): number | null {
  for (const value of values) {
    if (typeof value === "number" && Number.isFinite(value)) return value;
  }
  return null;
}

function restoreErrorInfo(error: unknown): { message: string; recovery: string } {
  const value = recordValue(error);
  return {
    message: value && typeof value.message === "string"
      ? value.message
      : "CoffeePOS không thể hoàn tất thao tác khôi phục.",
    recovery: value && typeof value.recovery === "string"
      ? value.recovery
      : "Kiểm tra bản sao lưu và thử lại. Chi tiết đường dẫn file được giữ ở native layer.",
  };
}

function restoreCandidateId(inspection = currentRestoreInspection): string {
  if (!inspection) return "";
  return firstString(inspection.candidate_id, inspection.restore_candidate_id);
}

function restoreInspectionWarnings(inspection: RestoreInspection): string[] {
  const backup = recordValue(inspection.backup);
  const manifest = recordValue(inspection.manifest);
  const values = [backup?.warnings, inspection.warnings, manifest?.warnings];
  for (const value of values) {
    if (Array.isArray(value)) return value.filter((item): item is string => typeof item === "string");
  }
  return [];
}

function restoreCompatibility(inspection: RestoreInspection): { compatible: boolean; label: string } {
  const backup = recordValue(inspection.backup);
  if (backup && typeof backup.can_restore === "boolean") {
    const compatibility = recordValue(backup.compatibility);
    const status = firstString(compatibility?.status);
    const label = firstString(compatibility?.message, status)
      || (backup.can_restore
        ? "Tương thích với phiên bản CoffeePOS Desktop hiện tại."
        : "Bản sao lưu không tương thích với phiên bản hiện tại.");
    return { compatible: backup.can_restore, label };
  }
  if (typeof inspection.compatible === "boolean") {
    return {
      compatible: inspection.compatible,
      label: inspection.compatible ? "Tương thích với phiên bản CoffeePOS Desktop hiện tại." : "Bản sao lưu không tương thích với phiên bản hiện tại.",
    };
  }
  if (typeof inspection.compatibility === "boolean") {
    return {
      compatible: inspection.compatibility,
      label: inspection.compatibility ? "Tương thích với phiên bản CoffeePOS Desktop hiện tại." : "Bản sao lưu không tương thích với phiên bản hiện tại.",
    };
  }
  if (typeof inspection.compatibility === "string") {
    const value = inspection.compatibility.trim();
    const compatible = !/blocked|incompatible|unsupported|reject/i.test(value);
    return { compatible, label: value || (compatible ? "Tương thích." : "Không tương thích.") };
  }
  const compatibility = recordValue(inspection.compatibility);
  if (compatibility) {
    const compatibleValue = compatibility.compatible;
    const status = firstString(compatibility.status);
    const compatible = typeof compatibleValue === "boolean"
      ? compatibleValue
      : !/blocked|incompatible|unsupported|reject/i.test(status);
    const label = firstString(compatibility.message, compatibility.reason, status)
      || (compatible ? "Tương thích với phiên bản CoffeePOS Desktop hiện tại." : "Bản sao lưu không tương thích với phiên bản hiện tại.");
    return { compatible, label };
  }
  return {
    compatible: false,
    label: "Không thể xác nhận khả năng tương thích từ kết quả kiểm tra bản sao lưu.",
  };
}

function restoreInspectionProjection(inspection: RestoreInspection): {
  storeName: string;
  createdAt: number | string | null;
  wordpress: string;
  woocommerce: string;
  coffeepos: string;
  uploadsBytes: number | null;
} {
  const backup = recordValue(inspection.backup);
  const source = recordValue(backup?.source);
  const manifest = recordValue(inspection.manifest);
  const store = recordValue(manifest?.store);
  const versions = recordValue(manifest?.versions);
  const uploads = recordValue(manifest?.uploads);
  return {
    storeName: firstString(backup?.store_name, inspection.store_name, manifest?.store_name, store?.name) || "—",
    createdAt: typeof backup?.created_at === "string" || typeof backup?.created_at === "number"
      ? backup.created_at
      : typeof inspection.created_at === "string" || typeof inspection.created_at === "number"
        ? inspection.created_at
        : firstFiniteNumber(manifest?.created_at),
    wordpress: firstString(source?.wordpress_version, inspection.wordpress_version, manifest?.wordpress_version, versions?.wordpress) || "—",
    woocommerce: firstString(source?.woocommerce_version, inspection.woocommerce_version, manifest?.woocommerce_version, versions?.woocommerce) || "—",
    coffeepos: firstString(source?.coffeepos_version, inspection.coffeepos_version, manifest?.coffeepos_version, versions?.coffeepos) || "—",
    uploadsBytes: firstFiniteNumber(backup?.uploads_bytes, inspection.uploads_bytes, manifest?.uploads_bytes, uploads?.total_bytes, uploads?.bytes),
  };
}

function restoreTerminalKind(status = currentRestoreStatus): "success" | "cancelled" | "rolled_back" | "failed" | "recovery" | null {
  if (!status) return null;
  const stage = status.stage ?? "";
  if (status.needs_recovery || status.recovery_required || ["blocked", "needs_recovery", "recovery_required", "rollback_failed"].includes(stage)) return "recovery";
  if (stage === "committed" || stage === "cleanup") return "success";
  if (stage === "rolled_back") return "rolled_back";
  if (stage === "aborted") return status.failed ? "failed" : "cancelled";
  if ([
    "planned",
    "validated",
    "runtime_stopped",
    "recovery_backup_ready",
    "staging_prepared",
    "database_imported",
    "uploads_restored",
    "target_secrets_bound",
    "staging_verified",
    "cutover_started",
    "active_swapped",
    "active_verified",
    "abort_started",
    "rollback_started",
  ].includes(stage)) return null;
  if (status.succeeded) return "success";
  if (status.rolled_back) return "rolled_back";
  if (status.cancelled) return "cancelled";
  if (status.failed || stage === "failed") return "failed";
  return null;
}

function restoreIsActive(status = currentRestoreStatus): boolean {
  return !!status?.operation_id && restoreTerminalKind(status) === null;
}

function restoreNeedsRecovery(status = currentRestoreStatus): boolean {
  return restoreTerminalKind(status) === "recovery";
}

function restoreSystemBusy(): boolean {
  return restoreOperation === "inspect"
    || restoreOperation === "apply"
    || restoreOperation === "cancel"
    || restoreIsActive()
    || restoreNeedsRecovery();
}

function backupErrorInfo(error: unknown): BackupErrorInfo | null {
  if (!error || typeof error !== "object") return null;
  const value = error as Record<string, unknown>;
  if (typeof value.message !== "string") return null;
  return {
    component: typeof value.component === "string" ? value.component : "backup",
    action: typeof value.action === "string" ? value.action : "create",
    code: typeof value.code === "string" ? value.code : "backup_failed",
    message: value.message,
    recovery: typeof value.recovery === "string" ? value.recovery : "",
  };
}

function backupIsActive(status = currentBackupStatus): boolean {
  return !!status?.operation_id && !status.cancelled && !status.succeeded && !status.failed;
}

function backupSystemBusy(): boolean {
  return backupOperation === "create" || backupOperation === "cancel" || backupIsActive() || restoreSystemBusy();
}

function backupCancellationAvailable(status = currentBackupStatus): boolean {
  return (
    backupIsActive(status) &&
    !!status &&
    ["selecting_destination", "preflight", "quiesce", "database", "uploads", "archive"].includes(status.stage)
  );
}

function formatBackupTimestamp(value: number | string | null | undefined): string {
  if (value === null || value === undefined || value === "") return "—";
  const date = typeof value === "string"
    ? new Date(value)
    : Number.isFinite(value)
      ? new Date(value < 10_000_000_000 ? value * 1000 : value)
      : new Date(Number.NaN);
  if (Number.isNaN(date.getTime())) return "—";
  return new Intl.DateTimeFormat("vi-VN", {
    dateStyle: "short",
    timeStyle: "short",
  }).format(date);
}

function formatBackupBytes(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let amount = value;
  let unit = 0;
  while (amount >= 1024 && unit < units.length - 1) {
    amount /= 1024;
    unit += 1;
  }
  return `${amount >= 10 || unit === 0 ? amount.toFixed(0) : amount.toFixed(1)} ${units[unit]}`;
}

function backupStageLabel(stage: string): string {
  const labels: Record<string, string> = {
    planned: "Đang chuẩn bị sao lưu",
    selecting_destination: "Đang chờ chọn nơi lưu",
    preflight: "Đang kiểm tra điều kiện sao lưu",
    quiesce: "Đang tạm dừng dịch vụ cửa hàng",
    database: "Đang sao lưu cơ sở dữ liệu",
    uploads: "Đang sao lưu hình ảnh tải lên",
    archive: "Đang tạo file sao lưu mã hóa",
    validate: "Đang kiểm tra file sao lưu",
    finalize: "Đang hoàn tất file sao lưu",
    cleanup: "Đang dọn dữ liệu tạm",
    resume: "Đang khôi phục trạng thái cửa hàng",
  };
  return labels[stage] ?? "Đang xử lý dữ liệu cửa hàng";
}

function selectBackupView(view: BackupView, moveFocus = false): void {
  currentBackupView = view;
  for (const panel of backupViews) panel.hidden = panel.dataset.backupView !== view;
  if (!moveFocus) return;
  if (view === "password") backupPassword.focus();
  else element<HTMLElement>("backup-title").focus();
}

function clearBackupPasswordFields(): void {
  backupPassword.value = "";
  backupPasswordConfirm.value = "";
  backupPassword.removeAttribute("aria-invalid");
  backupPasswordConfirm.removeAttribute("aria-invalid");
  backupPasswordError.textContent = "";
  backupPasswordError.hidden = true;
}

function setBackupControls(): void {
  const active = backupIsActive();
  const mutating = backupOperation === "create" || backupOperation === "cancel";
  const restoring = restoreSystemBusy();
  const eligible = currentProvisioning?.state === "ready" && !repairRouteRequired;
  backupCard.setAttribute("aria-busy", active || mutating || restoring ? "true" : "false");
  backupCreate.disabled = !eligible || backupSystemBusy();
  backupPassword.disabled = mutating || active || restoring;
  backupPasswordConfirm.disabled = mutating || active || restoring;
  backupPasswordCancel.disabled = mutating || active || restoring;
  backupChooseDestination.disabled = !eligible || mutating || active || restoring;
  backupCancel.disabled = !backupCancellationAvailable() || backupOperation === "cancel" || restoring;
  backupOpenFolder.disabled = restoring || backupOperation === "open_folder" || !currentBackupStatus?.succeeded || !currentBackupStatus.operation_id;
  backupCreateAnother.disabled = backupSystemBusy();
  backupRetry.disabled = backupSystemBusy() || !eligible;
}

function syncOperationControls(): void {
  setBackupControls();
  setRestoreControls();
  setRuntimeControls(currentRuntime);
  setHealthControls();
  setRepairControls();
  settingsSave.disabled = settingsBusy || backupSystemBusy();
  renderHome();
}

function renderBackupProgress(status: BackupStatus): void {
  const processedFiles = Math.max(0, status.processed_files ?? 0);
  const estimatedFiles = Math.max(0, status.estimated_files ?? 0);
  const processedBytes = Math.max(0, status.processed_bytes ?? 0);
  const estimatedBytes = Math.max(0, status.estimated_bytes ?? 0);
  setTextIfChanged(backupProgressStage, backupStageLabel(status.stage));
  if (estimatedFiles > 0) {
    backupProgress.max = estimatedFiles;
    backupProgress.value = Math.min(processedFiles, estimatedFiles);
    setTextIfChanged(
      backupProgressCount,
      `Đã xử lý ${processedFiles.toLocaleString("vi-VN")} / ${estimatedFiles.toLocaleString("vi-VN")} tệp.`,
    );
  } else if (estimatedBytes > 0) {
    backupProgress.max = estimatedBytes;
    backupProgress.value = Math.min(processedBytes, estimatedBytes);
    setTextIfChanged(
      backupProgressCount,
      `Đã xử lý ${formatBackupBytes(processedBytes)} / ${formatBackupBytes(estimatedBytes)}.`,
    );
  } else {
    backupProgress.max = 1;
    backupProgress.removeAttribute("value");
    setTextIfChanged(backupProgressCount, "Đang chờ thông tin tiến độ từ hệ thống.");
  }
}

function renderBackupStatus(status: BackupStatus): void {
  currentBackupStatus = status;
  const unmanagedSiteCodeExcluded = status.warnings.some(
    (warning) => warning === "unmanaged_site_code_not_included" || warning === "unmanaged_extensions_excluded",
  );
  backupSuccessWarning.hidden = !status.succeeded || !unmanagedSiteCodeExcluded;
  const lastSuccess = status.succeeded ? status.finished_at : null;
  backupLastSuccess.hidden = !lastSuccess;
  if (lastSuccess) setTextIfChanged(backupLastSuccessTime, formatBackupTimestamp(lastSuccess));

  if (status.succeeded) {
    setTextIfChanged(backupState, "Đã hoàn thành");
    setTextIfChanged(backupSuccessStatus, "");
    selectBackupView("success");
  } else if (status.failed) {
    setTextIfChanged(backupState, "Có lỗi");
    setTextIfChanged(backupFailureMessage, status.last_error?.message ?? "CoffeePOS chưa thể hoàn tất thao tác sao lưu.");
    setTextIfChanged(backupFailureRecovery, status.last_error?.recovery ?? "");
    selectBackupView("failure");
  } else if (status.cancelled) {
    setTextIfChanged(backupState, "Đã hủy");
    setTextIfChanged(backupLandingStatus, "Đã hủy sao lưu. Không có file backup mới được hoàn tất.");
    selectBackupView("landing");
  } else if (backupIsActive(status)) {
    setTextIfChanged(backupState, "Đang sao lưu");
    setTextIfChanged(backupProgressTitle, "Đang sao lưu…");
    setTextIfChanged(backupProgressStatus, backupOperation === "cancel" ? "Đã yêu cầu hủy. CoffeePOS đang dọn dữ liệu tạm an toàn…" : "");
    renderBackupProgress(status);
    selectBackupView("progress");
  } else if (currentBackupView !== "password" || backupOperation === null) {
    setTextIfChanged(backupState, "Sẵn sàng");
    if (currentBackupView !== "password") selectBackupView("landing");
  }
  syncOperationControls();
}

function showBackupFailure(error: unknown): void {
  const structured = backupErrorInfo(error);
  currentBackupStatus = {
    operation_id: currentBackupStatus?.operation_id ?? null,
    stage: "failed",
    processed_files: currentBackupStatus?.processed_files ?? 0,
    estimated_files: currentBackupStatus?.estimated_files ?? 0,
    processed_bytes: currentBackupStatus?.processed_bytes ?? 0,
    estimated_bytes: currentBackupStatus?.estimated_bytes ?? 0,
    started_at: currentBackupStatus?.started_at ?? null,
    finished_at: null,
    cancelled: false,
    succeeded: false,
    failed: true,
    warnings: [],
    last_error: structured,
  };
  setTextIfChanged(backupState, "Có lỗi");
  setTextIfChanged(backupFailureMessage, structured?.message ?? nativeErrorText(error));
  setTextIfChanged(backupFailureRecovery, structured?.recovery ?? "");
  selectBackupView("failure", true);
  syncOperationControls();
}

function stopBackupPolling(): void {
  if (backupPollTimer === null) return;
  window.clearInterval(backupPollTimer);
  backupPollTimer = null;
}

function ensureBackupPolling(): void {
  if (backupPollTimer !== null) return;
  backupPollTimer = window.setInterval(() => {
    void refreshBackupStatus();
  }, BACKUP_POLL_INTERVAL_MS);
}

async function refreshBackupStatus(moveFocus = false): Promise<BackupStatus | null> {
  if (!isTauri()) return null;
  if (backupStatusRefreshBusy) return currentBackupStatus;
  backupStatusRefreshBusy = true;
  const wasActive = backupIsActive();
  try {
    const status = await invoke<BackupStatus>("get_backup_status");
    renderBackupStatus(status);
    if (backupIsActive(status) || backupOperation === "create" || backupOperation === "cancel") ensureBackupPolling();
    else stopBackupPolling();
    if (wasActive && !backupIsActive(status) && backupOperation === null) void refreshRuntime();
    if (moveFocus) element<HTMLElement>("backup-title").focus();
    return status;
  } catch (error) {
    if (backupOperation === "create" || backupOperation === "cancel" || backupIsActive()) {
      setTextIfChanged(backupProgressStatus, "Không thể cập nhật tiến độ tạm thời. CoffeePOS vẫn giữ thao tác sao lưu hiện tại.");
      ensureBackupPolling();
      return currentBackupStatus;
    }
    stopBackupPolling();
    if (currentSystemSection === "backup") showBackupFailure(error);
    return null;
  } finally {
    backupStatusRefreshBusy = false;
  }
}

function validateBackupPassword(): string | null {
  backupPasswordError.hidden = true;
  backupPasswordError.textContent = "";
  backupPassword.removeAttribute("aria-invalid");
  backupPasswordConfirm.removeAttribute("aria-invalid");
  const password = backupPassword.value;
  if (password.length === 0) {
    backupPassword.setAttribute("aria-invalid", "true");
    setTextIfChanged(backupPasswordError, "Nhập mật khẩu cho bản sao lưu.");
    backupPasswordError.hidden = false;
    backupPassword.focus();
    return null;
  }
  if (password !== backupPasswordConfirm.value) {
    backupPasswordConfirm.setAttribute("aria-invalid", "true");
    setTextIfChanged(backupPasswordError, "Hai lần nhập mật khẩu chưa khớp.");
    backupPasswordError.hidden = false;
    backupPasswordConfirm.focus();
    return null;
  }
  return password;
}

async function createBackup(): Promise<void> {
  if (!isTauri() || backupSystemBusy() || currentProvisioning?.state !== "ready" || repairRouteRequired) return;
  let password = validateBackupPassword();
  if (password === null) return;
  backupOperation = "create";
  setTextIfChanged(backupState, "Đang chuẩn bị");
  setTextIfChanged(backupProgressStage, "Chọn nơi lưu trong cửa sổ Save As của Windows.");
  setTextIfChanged(backupProgressCount, "CoffeePOS sẽ bắt đầu snapshot sau khi bạn chọn vị trí lưu.");
  setTextIfChanged(backupProgressStatus, "");
  backupProgress.max = 1;
  backupProgress.removeAttribute("value");
  selectBackupView("progress");
  syncOperationControls();
  ensureBackupPolling();
  const createPromise = invoke<BackupResult>("create_backup", { backupPassword: password });
  clearBackupPasswordFields();
  password = "";
  try {
    await createPromise;
    const status = await refreshBackupStatus();
    if (status && !status.operation_id && !status.succeeded && !status.failed) {
      setTextIfChanged(backupState, "Sẵn sàng");
      setTextIfChanged(backupLandingStatus, "");
      selectBackupView("landing");
    }
  } catch (error) {
    const status = await refreshBackupStatus();
    if (!status?.failed && !backupIsActive(status)) showBackupFailure(error);
  } finally {
    backupOperation = null;
    const status = await refreshBackupStatus();
    if (!backupIsActive(status)) {
      stopBackupPolling();
      await refreshRuntime();
    }
    syncOperationControls();
  }
}

async function cancelBackup(): Promise<void> {
  const operationId = currentBackupStatus?.operation_id;
  if (!isTauri() || !operationId || !backupIsActive() || backupOperation) return;
  backupOperation = "cancel";
  setTextIfChanged(backupProgressStatus, "Đang yêu cầu hủy sao lưu…");
  syncOperationControls();
  try {
    await invoke<unknown>("cancel_backup", { operationId });
    ensureBackupPolling();
  } catch (error) {
    setTextIfChanged(backupProgressStatus, backupErrorInfo(error)?.message ?? nativeErrorText(error));
  } finally {
    backupOperation = null;
    await refreshBackupStatus();
    syncOperationControls();
  }
}

async function openBackupFolder(): Promise<void> {
  const operationId = currentBackupStatus?.operation_id;
  if (!isTauri() || !operationId || !currentBackupStatus?.succeeded || backupOperation) return;
  backupOperation = "open_folder";
  setTextIfChanged(backupSuccessStatus, "Đang mở thư mục…");
  setBackupControls();
  try {
    await invoke<void>("open_backup_folder", { operationId });
    setTextIfChanged(backupSuccessStatus, "Đã yêu cầu Windows mở thư mục chứa file sao lưu.");
  } catch (error) {
    setTextIfChanged(backupSuccessStatus, backupErrorInfo(error)?.message ?? nativeErrorText(error));
  } finally {
    backupOperation = null;
    setBackupControls();
  }
}

function showBackupPasswordStep(): void {
  if (backupSystemBusy() || currentProvisioning?.state !== "ready" || repairRouteRequired) return;
  clearBackupPasswordFields();
  setTextIfChanged(backupLandingStatus, "");
  setTextIfChanged(backupState, "Sẵn sàng");
  selectBackupView("password", true);
  setBackupControls();
}

function restoreStageLabel(stage: string): string {
  const labels: Record<string, string> = {
    planned: "Đã lập kế hoạch khôi phục",
    validated: "Đã kiểm tra bản sao lưu",
    runtime_stopped: "Đã tạm dừng cửa hàng",
    recovery_backup_ready: "Đã tạo bản sao an toàn của trạng thái hiện tại",
    staging_prepared: "Đang chuẩn bị dữ liệu khôi phục",
    database_imported: "Đã khôi phục cơ sở dữ liệu vào staging",
    uploads_restored: "Đã khôi phục uploads vào staging",
    target_secrets_bound: "Đã tạo credential cho Windows profile này",
    staging_verified: "Đã kiểm tra staging",
    cutover_started: "Đang chuyển cửa hàng đã kiểm tra sang trạng thái active",
    active_swapped: "Đang kiểm tra cửa hàng sau khi chuyển dữ liệu",
    active_verified: "Cửa hàng mới đã vượt qua kiểm tra cuối",
    committed: "Đã commit khôi phục",
    cleanup: "Đang dọn dữ liệu tạm an toàn",
    abort_started: "Đang hủy khôi phục và khôi phục trạng thái trước",
    rollback_started: "Đang hoàn tác về cửa hàng trước khi khôi phục",
    rolled_back: "Đã hoàn tác về trạng thái trước",
    aborted: "Đã hủy khôi phục",
  };
  return labels[stage] ?? "CoffeePOS đang xử lý giao dịch khôi phục";
}

function restoreCancellationAvailable(status = currentRestoreStatus): boolean {
  if (!restoreIsActive(status) || !status) return false;
  return !["active_verified", "committed", "cleanup", "abort_started", "rollback_started"].includes(status.stage);
}

function restoreCutoverStarted(status = currentRestoreStatus): boolean {
  if (!status) return false;
  return [
    "cutover_started",
    "active_swapped",
    "active_verified",
    "rollback_started",
  ].includes(status.stage);
}

function clearRestorePassword(): void {
  restorePassword.value = "";
  restorePassword.removeAttribute("aria-invalid");
  restorePasswordError.textContent = "";
  restorePasswordError.hidden = true;
}

function selectRestoreView(view: RestoreView, moveFocus = false): void {
  currentRestoreView = view;
  for (const panel of restoreViews) panel.hidden = panel.dataset.restoreView !== view;
  if (!moveFocus) return;
  if (view === "password") restorePassword.focus();
  else element<HTMLElement>("restore-title").focus();
}

function showRestoreScreen(view: RestoreView, moveFocus = false): void {
  bootstrapPanel.hidden = true;
  setup.hidden = true;
  installedShell.hidden = true;
  restoreScreen.hidden = false;
  selectRestoreView(view, moveFocus);
}

function hideRestoreScreen(): void {
  restoreScreen.hidden = true;
  restoreScreen.setAttribute("aria-busy", "false");
}

function setRestoreControls(): void {
  const active = restoreIsActive();
  const mutating = restoreOperation === "apply" || restoreOperation === "cancel";
  const inspecting = restoreOperation === "inspect";
  const compatibility = currentRestoreInspection ? restoreCompatibility(currentRestoreInspection) : null;
  const installedEligible = currentProvisioning?.state === "ready" && !repairRouteRequired;
  const freshEligible = currentProvisioning?.state === "not_installed";
  const otherMaintenance = backupOperation === "create" || backupOperation === "cancel" || backupIsActive();

  restoreCard.setAttribute("aria-busy", active || mutating || inspecting ? "true" : "false");
  restoreScreen.setAttribute("aria-busy", active || mutating ? "true" : "false");
  setupRestore.disabled = !freshEligible || otherMaintenance || restoreSystemBusy();
  restoreInstalledStart.disabled = !installedEligible || otherMaintenance || restoreSystemBusy();
  restorePassword.disabled = active || mutating || inspecting;
  restoreInspect.disabled = active || mutating || inspecting || otherMaintenance;
  restorePasswordCancel.disabled = active || mutating || inspecting;
  restoreReviewCancel.disabled = active || mutating;
  restoreApply.disabled = active
    || mutating
    || !restoreCandidateId()
    || compatibility?.compatible === false
    || otherMaintenance;
  restoreCancel.disabled = !restoreCancellationAvailable() || restoreOperation === "cancel";
  restoreFailureBack.disabled = active || mutating;
  restoreRetry.disabled = active || mutating || otherMaintenance;
  restoreRecoveryRefresh.disabled = restoreStatusRefreshBusy;
}

function renderRestoreInspection(inspection: RestoreInspection): void {
  const backup = recordValue(inspection.backup);
  const projection = restoreInspectionProjection(inspection);
  const compatibility = restoreCompatibility(inspection);
  const warnings = restoreInspectionWarnings(inspection);
  const candidateId = restoreCandidateId(inspection);
  currentRestoreInspection = {
    candidate_id: candidateId,
    backup_id: firstString(backup?.backup_id, inspection.backup_id),
    store_name: projection.storeName === "—" ? "" : projection.storeName,
    created_at: projection.createdAt,
    wordpress_version: projection.wordpress === "—" ? "" : projection.wordpress,
    woocommerce_version: projection.woocommerce === "—" ? "" : projection.woocommerce,
    coffeepos_version: projection.coffeepos === "—" ? "" : projection.coffeepos,
    uploads_bytes: projection.uploadsBytes ?? undefined,
    compatible: compatibility.compatible,
    compatibility: compatibility.label,
    warnings,
  };
  const unmanaged = warnings.some((warning) => /unmanaged|custom_(?:plugin|theme|code)|extensions_excluded/i.test(warning));

  setTextIfChanged(restoreReviewStore, projection.storeName);
  setTextIfChanged(restoreReviewCreated, projection.createdAt ? formatBackupTimestamp(projection.createdAt) : "—");
  setTextIfChanged(restoreReviewWordPress, projection.wordpress);
  setTextIfChanged(restoreReviewWooCommerce, projection.woocommerce);
  setTextIfChanged(restoreReviewCoffeePos, projection.coffeepos);
  setTextIfChanged(restoreReviewUploads, projection.uploadsBytes === null ? "—" : formatBackupBytes(projection.uploadsBytes));
  setTextIfChanged(restoreReviewCompatibility, compatibility.label);
  restoreUnmanagedWarning.hidden = !unmanaged;
  restoreReviewError.hidden = compatibility.compatible;
  setTextIfChanged(
    restoreReviewError,
    compatibility.compatible ? "" : "Bản sao lưu này chưa thể được áp dụng trên phiên bản CoffeePOS Desktop hiện tại.",
  );
  setTextIfChanged(
    restoreReviewImpact,
    restoreEntrySource === "fresh"
      ? "Khôi phục sẽ tạo cửa hàng trên Windows profile này từ bản sao lưu đã kiểm tra."
      : "Khôi phục sẽ thay dữ liệu cửa hàng hiện tại. CoffeePOS sẽ tạo một bản sao an toàn của trạng thái hiện tại trước khi thay.",
  );
  setTextIfChanged(restoreState, compatibility.compatible ? "Đã kiểm tra" : "Không tương thích");
  showRestoreScreen("review", true);
  setRestoreControls();
}

function renderRestoreProgress(status: RestoreStatus): void {
  setTextIfChanged(restoreState, "Đang khôi phục");
  setTextIfChanged(restoreProgressStage, restoreStageLabel(status.stage));
  const afterCutover = restoreCutoverStarted(status);
  const canCancel = restoreCancellationAvailable(status);
  setTextIfChanged(
    restoreProgressDetail,
    afterCutover
      ? "Dữ liệu đã bước vào giai đoạn chuyển active. Nếu dừng lúc này, CoffeePOS phải hoàn tất rollback có kiểm tra thay vì bỏ dở giao dịch."
      : "CoffeePOS đang giữ thao tác này trong admission gate để backup, repair, provisioning và runtime action khác không chạy đồng thời.",
  );
  setTextIfChanged(
    restoreCloseGuidance,
    afterCutover
      ? "Không tắt máy trong khi dữ liệu đang được chuyển hoặc hoàn tác. Đóng cửa sổ sẽ đi qua native lifecycle guard."
      : "Không tắt máy trong khi dữ liệu đang được chuẩn bị. Bạn có thể dùng Hủy khi nút còn khả dụng.",
  );
  setTextIfChanged(restoreCancel, afterCutover ? "Dừng và hoàn tác" : "Hủy khôi phục");
  restoreCancel.hidden = !canCancel && !restoreOperation;
  showRestoreScreen("progress");
}

function restoreStatusError(status: RestoreStatus): { message: string; recovery: string } {
  const error = status.last_error;
  if (!error) {
    return {
      message: "CoffeePOS chưa thể hoàn tất thao tác khôi phục.",
      recovery: "Kiểm tra trạng thái hiện tại rồi thử lại khi hệ thống cho phép.",
    };
  }
  return restoreErrorInfo(error);
}

function renderRestoreStatus(status: RestoreStatus): void {
  currentRestoreStatus = status;
  if (status.original_state === "no_previous_store") restoreEntrySource = "fresh";
  else if (status.original_state === "existing_store") restoreEntrySource = "installed";

  const terminal = restoreTerminalKind(status);
  if (terminal === "recovery") {
    const error = restoreStatusError(status);
    setTextIfChanged(restoreState, "Cần xử lý");
    setTextIfChanged(
      restoreRecoveryDetails,
      [error.message, error.recovery].filter(Boolean).join(" ") || "Native restore recovery đang giữ admission gate để bảo vệ dữ liệu cửa hàng.",
    );
    showRestoreScreen("recovery");
  } else if (terminal === "success") {
    setTextIfChanged(restoreState, "Hoàn tất");
    setTextIfChanged(restoreProgressStatus, "");
    clearRestorePassword();
    showRestoreScreen("success");
  } else if (terminal === "rolled_back") {
    const error = restoreStatusError(status);
    setTextIfChanged(restoreState, "Đã hoàn tác");
    setTextIfChanged(restoreFailureTitle, "Không thể khôi phục");
    setTextIfChanged(
      restoreFailureMessage,
      restoreEntrySource === "fresh"
        ? "CoffeePOS đã đưa profile về trạng thái chưa cài đặt sau khi khôi phục không hoàn tất."
        : "CoffeePOS đã đưa cửa hàng về trạng thái trước khi khôi phục.",
    );
    setTextIfChanged(restoreFailureRecovery, [error.message, error.recovery].filter(Boolean).join(" "));
    clearRestorePassword();
    showRestoreScreen("failure");
  } else if (terminal === "cancelled") {
    setTextIfChanged(restoreState, "Đã hủy");
    setTextIfChanged(restoreFailureTitle, "Đã hủy khôi phục");
    setTextIfChanged(
      restoreFailureMessage,
      restoreEntrySource === "fresh"
        ? "CoffeePOS đã dọn dữ liệu tạm và giữ profile ở trạng thái chưa cài đặt."
        : "CoffeePOS đã dọn dữ liệu tạm và xác minh lại trạng thái cửa hàng trước đó.",
    );
    setTextIfChanged(restoreFailureRecovery, "");
    clearRestorePassword();
    showRestoreScreen("failure");
  } else if (terminal === "failed") {
    const error = restoreStatusError(status);
    setTextIfChanged(restoreState, "Có lỗi");
    setTextIfChanged(restoreFailureTitle, "Không thể khôi phục");
    setTextIfChanged(restoreFailureMessage, error.message);
    setTextIfChanged(restoreFailureRecovery, error.recovery);
    clearRestorePassword();
    showRestoreScreen("failure");
  } else if (restoreIsActive(status)) {
    renderRestoreProgress(status);
  }

  setRestoreControls();
  setBackupControls();
  setRuntimeControls(currentRuntime);
  setHealthControls();
  setRepairControls();
  renderHome();
}

function showRestoreFailure(error: unknown): void {
  const safe = restoreErrorInfo(error);
  setTextIfChanged(restoreState, "Có lỗi");
  setTextIfChanged(restoreFailureTitle, "Không thể khôi phục");
  setTextIfChanged(restoreFailureMessage, safe.message);
  setTextIfChanged(restoreFailureRecovery, safe.recovery);
  clearRestorePassword();
  showRestoreScreen("failure", true);
  setRestoreControls();
}

function stopRestorePolling(): void {
  if (restorePollTimer === null) return;
  window.clearInterval(restorePollTimer);
  restorePollTimer = null;
}

function ensureRestorePolling(): void {
  if (restorePollTimer !== null) return;
  restorePollTimer = window.setInterval(() => {
    void refreshRestoreStatus();
  }, RESTORE_POLL_INTERVAL_MS);
}

async function refreshRestoreStatus(moveFocus = false): Promise<RestoreStatus | null> {
  if (!isTauri()) return null;
  if (restoreStatusRefreshBusy) return currentRestoreStatus;
  restoreStatusRefreshBusy = true;
  const wasActive = restoreIsActive();
  try {
    const status = await invoke<RestoreStatus>("get_restore_status");
    renderRestoreStatus(status);
    if (restoreIsActive(status) || restoreOperation === "apply" || restoreOperation === "cancel") ensureRestorePolling();
    else stopRestorePolling();
    if (wasActive && !restoreIsActive(status) && !restoreNeedsRecovery(status) && restoreOperation === null) {
      void refreshProvisioning().then((loaded) => {
        if (loaded && currentProvisioning?.state === "ready") void refreshRuntime();
      });
    }
    if (moveFocus && !restoreScreen.hidden) element<HTMLElement>("restore-title").focus();
    return status;
  } catch (error) {
    if (restoreIsActive() || restoreOperation === "apply" || restoreOperation === "cancel") {
      setTextIfChanged(restoreProgressStatus, "Không thể cập nhật tiến độ tạm thời. Native restore transaction vẫn đang giữ trạng thái hiện tại.");
      ensureRestorePolling();
      return currentRestoreStatus;
    }
    stopRestorePolling();
    if (!restoreScreen.hidden) showRestoreFailure(error);
    return null;
  } finally {
    restoreStatusRefreshBusy = false;
    setRestoreControls();
  }
}

function restoreEntryAllowed(source: RestoreEntrySource): boolean {
  if (source === "fresh") return currentProvisioning?.state === "not_installed";
  return currentProvisioning?.state === "ready" && !repairRouteRequired;
}

function beginRestore(source: RestoreEntrySource): void {
  const backupBusy = backupOperation === "create" || backupOperation === "cancel" || backupIsActive();
  if (backupBusy || restoreSystemBusy() || !restoreEntryAllowed(source)) return;
  restoreEntrySource = source;
  currentRestoreInspection = null;
  currentRestoreStatus = null;
  clearRestorePassword();
  restoreReviewError.hidden = true;
  setTextIfChanged(restoreState, "Sẵn sàng");
  showRestoreScreen("password", true);
  setRestoreControls();
  setBackupControls();
}

async function inspectRestoreBackup(): Promise<void> {
  if (!isTauri() || restoreOperation || restoreIsActive()) return;
  const password = restorePassword.value;
  restorePasswordError.hidden = true;
  restorePassword.removeAttribute("aria-invalid");
  if (password.length === 0) {
    restorePassword.setAttribute("aria-invalid", "true");
    setTextIfChanged(restorePasswordError, "Nhập mật khẩu của bản sao lưu.");
    restorePasswordError.hidden = false;
    restorePassword.focus();
    return;
  }

  restoreOperation = "inspect";
  currentRestoreInspection = null;
  setTextIfChanged(restoreState, "Đang kiểm tra");
  setTextIfChanged(restorePasswordError, "");
  setRestoreControls();
  setBackupControls();
  try {
    const inspection = await invoke<RestoreInspection | null>("inspect_restore_backup", { backupPassword: password });
    if (!inspection || !restoreCandidateId(inspection)) {
      clearRestorePassword();
      setTextIfChanged(restoreState, "Sẵn sàng");
      return;
    }
    renderRestoreInspection(inspection);
  } catch (error) {
    const safe = restoreErrorInfo(error);
    clearRestorePassword();
    setTextIfChanged(restoreState, "Không thể kiểm tra");
    setTextIfChanged(restorePasswordError, [safe.message, safe.recovery].filter(Boolean).join(" "));
    restorePasswordError.hidden = false;
    showRestoreScreen("password");
  } finally {
    restoreOperation = null;
    setRestoreControls();
    setBackupControls();
  }
}

async function applyRestore(): Promise<void> {
  const candidateId = restoreCandidateId();
  const compatibility = currentRestoreInspection ? restoreCompatibility(currentRestoreInspection) : null;
  if (!isTauri() || !candidateId || compatibility?.compatible === false || restoreOperation || restoreIsActive()) return;
  let password = restorePassword.value;
  if (password.length === 0) {
    currentRestoreInspection = null;
    setTextIfChanged(restorePasswordError, "Nhập lại mật khẩu để kiểm tra bản sao lưu trước khi khôi phục.");
    restorePasswordError.hidden = false;
    showRestoreScreen("password", true);
    return;
  }

  restoreOperation = "apply";
  currentRestoreStatus = {
    operation_id: null,
    stage: "planned",
    original_state: restoreEntrySource === "fresh" ? "no_previous_store" : "existing_store",
  };
  setTextIfChanged(restoreProgressStatus, "");
  renderRestoreProgress(currentRestoreStatus);
  setRestoreControls();
  setBackupControls();
  ensureRestorePolling();
  const applyPromise = invoke<RestoreResult>("apply_restore", {
    candidateId,
    backupPassword: password,
  });
  clearRestorePassword();
  password = "";
  try {
    await applyPromise;
    await refreshRestoreStatus();
  } catch (error) {
    const status = await refreshRestoreStatus();
    if (!restoreIsActive(status) && !restoreNeedsRecovery(status) && restoreTerminalKind(status) === null) {
      showRestoreFailure(error);
    }
  } finally {
    restoreOperation = null;
    const status = await refreshRestoreStatus();
    if (!restoreIsActive(status)) stopRestorePolling();
    setRestoreControls();
    setBackupControls();
  }
}

async function cancelRestore(): Promise<void> {
  const operationId = currentRestoreStatus?.operation_id;
  if (!isTauri() || !operationId || !restoreCancellationAvailable() || restoreOperation) return;
  const afterCutover = restoreCutoverStarted();
  restoreOperation = "cancel";
  setTextIfChanged(
    restoreProgressStatus,
    afterCutover
      ? "Đã yêu cầu dừng. CoffeePOS đang hoàn tác và xác minh trạng thái trước khi khôi phục…"
      : "Đã yêu cầu hủy. CoffeePOS đang dọn staging và xác minh trạng thái trước khi khôi phục…",
  );
  setRestoreControls();
  try {
    await invoke<unknown>("cancel_restore", { operationId });
    ensureRestorePolling();
  } catch (error) {
    const safe = restoreErrorInfo(error);
    setTextIfChanged(restoreProgressStatus, [safe.message, safe.recovery].filter(Boolean).join(" "));
  } finally {
    restoreOperation = null;
    await refreshRestoreStatus();
    setRestoreControls();
  }
}

async function leaveRestoreFlow(openHome = false): Promise<void> {
  if (restoreIsActive() || restoreNeedsRecovery() || restoreOperation) return;
  clearRestorePassword();
  currentRestoreInspection = null;
  hideRestoreScreen();
  const loaded = await refreshProvisioning();
  if (!loaded || !currentProvisioning) {
    if (restoreEntrySource === "fresh") setup.hidden = false;
    return;
  }
  if (currentProvisioning.state === "not_installed") await refreshSetupInfo();
  applyInstallationLayout(currentProvisioning);
  if (currentProvisioning.state === "ready") {
    await refreshRuntime();
    if (openHome) selectInstalledView("home", true);
    else if (restoreEntrySource === "installed") {
      selectInstalledView("diagnostics", false);
      selectSystemSection("backup", true, true);
    }
  }
}

function structuredErrorText(error: RuntimeErrorInfo): string {
  return `${error.message}\n${error.recovery}`;
}

function setTextIfChanged(node: HTMLElement, value: string): void {
  if (node.textContent !== value) node.textContent = value;
}

function setHiddenIfChanged(node: HTMLElement, hidden: boolean): void {
  if (node.hidden !== hidden) node.hidden = hidden;
}

const healthComponents: HealthComponent[] = ["database", "php", "wordpress", "woocommerce", "coffeepos"];

function healthComponentName(component: HealthComponent): string {
  if (component === "database") return "Database";
  if (component === "php") return "PHP";
  if (component === "wordpress") return "WordPress";
  if (component === "woocommerce") return "WooCommerce";
  return "CoffeePOS";
}

function healthStateLabel(state: ComponentHealthState): string {
  if (state === "healthy") return "Khỏe";
  if (state === "unhealthy") return "Có lỗi";
  if (state === "unknown") return "Chưa xác minh";
  return "Không hoạt động";
}

function defaultHealthSummary(component: HealthComponent, state: ComponentHealthState, runtimeState: RuntimeInfo["state"]): string {
  const displayName = healthComponentName(component);
  if (state === "healthy") {
    if (component === "database") return "MariaDB trả lời truy vấn chẩn đoán đã xác thực.";
    if (component === "php") return "PHP thực thi đúng nonce probe trên HTTP runtime hiện tại.";
    if (component === "wordpress") return "WordPress trả readiness response hợp lệ.";
    return `Machine-health đã xác nhận ${displayName} sẵn sàng.`;
  }
  if (state === "unavailable") {
    return runtimeState === "running"
      ? `${displayName} chưa thể được kiểm tra ở trạng thái hiện tại.`
      : `${displayName} chưa được kiểm tra vì runtime không chạy.`;
  }
  if (state === "unknown") return `Chưa đủ bằng chứng để kết luận sức khỏe ${displayName}.`;
  return `${displayName} chưa vượt qua health check.`;
}

function defaultHealthRecovery(component: HealthComponent, state: ComponentHealthState, runtimeState: RuntimeInfo["state"]): string {
  if (state === "healthy") return "";
  if (state === "unavailable" && runtimeState !== "running") return "Khởi động hệ thống rồi kiểm tra lại.";
  if (state === "unknown") {
    if (component === "woocommerce") return "Xử lý lỗi WordPress/CoffeePOS phía trên rồi chọn Kiểm tra lại.";
    return "Xử lý dependency đang lỗi rồi chọn Kiểm tra lại.";
  }
  return "Chọn Kiểm tra lại. Nếu lỗi lặp lại, dùng Khởi động lại trong Runtime trước khi chuyển sang repair.";
}

function renderHealthComponent(component: HealthComponent, info: ComponentHealthInfo, runtimeState: RuntimeInfo["state"]): void {
  const stateNode = element(`health-${component}-state`);
  const summaryNode = element(`health-${component}-summary`);
  const recoveryNode = element(`health-${component}-recovery`);
  setTextIfChanged(stateNode, healthStateLabel(info.state));
  stateNode.dataset.healthState = info.state;
  setTextIfChanged(summaryNode, info.error?.message ?? defaultHealthSummary(component, info.state, runtimeState));
  const recovery = info.error?.recovery ?? defaultHealthRecovery(component, info.state, runtimeState);
  setTextIfChanged(recoveryNode, recovery ? `Hướng xử lý: ${recovery}` : "");
}

function setHealthControls(): void {
  healthRecheck.disabled = diagnosticsBusy
    || repairOperation !== null
    || bootstrapBusy
    || runtimeBusy
    || provisioningBusy
    || backupSystemBusy()
    || currentProvisioning?.state !== "ready"
    || currentRuntime?.state !== "running";
}

function renderHealthDiagnostics(info: HealthDiagnosticsInfo): void {
  currentDiagnostics = info;
  for (const component of healthComponents) renderHealthComponent(component, info[component], info.runtime_state);
  const states = healthComponents.map((component) => info[component].state);
  if (states.includes("unhealthy")) {
    setTextIfChanged(healthSummaryState, "Cần xử lý");
    setTextIfChanged(healthSummary, "Ít nhất một thành phần đã được xác minh là chưa sẵn sàng. Xem đúng dòng lỗi và hướng xử lý bên dưới.");
  } else if (states.includes("unknown")) {
    setTextIfChanged(healthSummaryState, "Chưa xác minh");
    setTextIfChanged(healthSummary, "Một số thành phần chưa thể được xác minh; CoffeePOS không suy đoán dependency lỗi khi chưa có bằng chứng.");
  } else if (states.every((state) => state === "healthy")) {
    setTextIfChanged(healthSummaryState, "Sẵn sàng");
    setTextIfChanged(healthSummary, "Database, PHP, WordPress, WooCommerce và CoffeePOS đều vượt qua health check hiện tại.");
  } else if (info.runtime_state !== "running") {
    setTextIfChanged(healthSummaryState, "Đang dừng");
    setTextIfChanged(healthSummary, "Runtime chưa chạy. Khởi động hệ thống để kiểm tra health đầy đủ.");
  } else {
    setTextIfChanged(healthSummaryState, "Chưa xác minh");
    setTextIfChanged(healthSummary, "Health snapshot hiện tại chưa đủ để kết luận tất cả thành phần.");
  }
  setHealthControls();
}

function renderHealthChecking(
  summaryState = "Đang kiểm tra",
  summaryText = "Đang kiểm tra lần lượt Database, PHP, WordPress và machine-health của CoffeePOS…",
): void {
  currentDiagnostics = null;
  setTextIfChanged(healthSummaryState, summaryState);
  setTextIfChanged(healthSummary, summaryText);
  for (const component of healthComponents) {
    const stateNode = element(`health-${component}-state`);
    stateNode.dataset.healthState = "checking";
    setTextIfChanged(stateNode, "Đang kiểm tra");
    setTextIfChanged(element(`health-${component}-summary`), "Đang chờ kết quả health hiện tại.");
    setTextIfChanged(element(`health-${component}-recovery`), "");
  }
  setHealthControls();
}

function renderHealthCommandError(error: unknown): void {
  currentDiagnostics = null;
  setTextIfChanged(healthSummaryState, "Không thể kiểm tra");
  setTextIfChanged(healthSummary, nativeErrorText(error));
  for (const component of healthComponents) {
    const stateNode = element(`health-${component}-state`);
    stateNode.dataset.healthState = "unknown";
    setTextIfChanged(stateNode, "Chưa xác minh");
    setTextIfChanged(element(`health-${component}-summary`), "Snapshot chẩn đoán chưa hoàn tất nên không gán lỗi cho component này.");
    setTextIfChanged(element(`health-${component}-recovery`), "");
  }
  setHealthControls();
}

async function refreshHealthDiagnostics(): Promise<void> {
  if (!isTauri() || diagnosticsBusy || repairOperation || bootstrapBusy || runtimeBusy || provisioningBusy || backupSystemBusy() || currentProvisioning?.state !== "ready") return;
  diagnosticsBusy = true;
  setRuntimeControls(currentRuntime);
  renderHealthChecking();
  setTextIfChanged(healthCheckStatus, "Đang chạy health diagnostics…");
  try {
    const info = await invoke<HealthDiagnosticsInfo>("get_health_diagnostics");
    renderHealthDiagnostics(info);
    setTextIfChanged(healthCheckStatus, "Đã kiểm tra sức khỏe các thành phần.");
    await refreshRuntime();
  } catch (error) {
    renderHealthCommandError(error);
    setTextIfChanged(healthCheckStatus, nativeErrorText(error));
  } finally {
    diagnosticsBusy = false;
    setHealthControls();
    setRuntimeControls(currentRuntime);
  }
}

function repairClassificationLabel(classification: RepairClassification): string {
  if (classification === "repairable") return "Có thể sửa";
  if (classification === "requires_input") return "Cần xác nhận";
  return "Không thể tự sửa";
}

function setRepairControls(): void {
  const busy = repairOperation !== null || bootstrapBusy || provisioningBusy || runtimeBusy || diagnosticsBusy || backupSystemBusy();
  const eligible = currentProvisioning?.state === "ready" || currentProvisioning?.state === "needs_repair";
  repairInspect.disabled = busy || !eligible;
  repairApply.disabled = busy || !eligible || !currentRepairPlan?.can_apply;
  repairAdminPassword.disabled = busy;
  repairAdminPasswordConfirm.disabled = busy;
  repairOpenDiagnostics.disabled = busy;
}

function clearRepairPasswordFields(): void {
  repairAdminPassword.value = "";
  repairAdminPasswordConfirm.value = "";
  repairAdminPassword.removeAttribute("aria-invalid");
  repairAdminPasswordConfirm.removeAttribute("aria-invalid");
  repairAdminError.textContent = "";
  repairAdminError.hidden = true;
}

function resetRepairInspectionState(): void {
  currentRepairPlan = null;
  repairList.replaceChildren();
  repairAdminInput.hidden = true;
  clearRepairPasswordFields();
  repairError.textContent = "";
  repairError.hidden = true;
  repairOpenDiagnostics.hidden = true;
  setTextIfChanged(repairSummaryState, "Chưa kiểm tra");
  setTextIfChanged(
    repairSummary,
    "Nhấn Kiểm tra để CoffeePOS lập kế hoạch sửa chữa read-only cho trạng thái hiện tại.",
  );
  setTextIfChanged(repairInspect, "Kiểm tra");
  setTextIfChanged(repairStatus, "");
}

function renderRepairItems(items: RepairItem[]): void {
  repairList.replaceChildren();
  for (const item of items) {
    const article = document.createElement("article");
    article.className = "repair-row";
    const heading = document.createElement("div");
    heading.className = "repair-row-heading";
    const target = document.createElement("strong");
    target.textContent = item.target;
    const badge = document.createElement("span");
    badge.className = "state-badge";
    badge.dataset.repairClassification = item.classification;
    badge.textContent = repairClassificationLabel(item.classification);
    heading.append(target, badge);
    const action = document.createElement("p");
    action.className = "repair-action";
    action.textContent = item.action;
    const reason = document.createElement("p");
    reason.className = "repair-summary-text";
    reason.textContent = item.reason;
    const impact = document.createElement("p");
    impact.className = "hint";
    impact.textContent = item.impact;
    article.append(heading, action, reason, impact);
    repairList.append(article);
  }
}

function renderRepairPlan(plan: RepairPlan): void {
  currentRepairPlan = plan;
  setTextIfChanged(repairInspect, "Kiểm tra lại");
  repairError.hidden = true;
  repairError.textContent = "";
  repairOpenDiagnostics.hidden = true;
  setTextIfChanged(repairStatus, "");
  renderRepairItems(plan.items);
  const repairable = plan.items.filter((item) => item.classification === "repairable").length;
  const needsInput = plan.items.filter((item) => item.classification === "requires_input").length;
  const blocked = plan.items.filter((item) => item.classification === "blocked").length;
  const needsAdminPassword = plan.items.some((item) => item.input_kind === "admin_password");
  repairAdminInput.hidden = !needsAdminPassword;
  if (!needsAdminPassword) clearRepairPasswordFields();

  if (plan.items.length === 0) {
    setTextIfChanged(repairSummaryState, "Không cần sửa");
    setTextIfChanged(repairSummary, "CoffeePOS không phát hiện thành phần managed nào cần sửa ở snapshot hiện tại.");
  } else if (repairable + needsInput > 0) {
    setTextIfChanged(repairSummaryState, "Có thể sửa");
    const parts = [`${repairable + needsInput} mục có hành động an toàn`];
    if (blocked > 0) parts.push(`${blocked} mục bị chặn`);
    setTextIfChanged(repairSummary, `${parts.join(" · ")}. Xem phạm vi và ảnh hưởng của từng mục trước khi sửa.`);
  } else {
    setTextIfChanged(repairSummaryState, "Không thể tự sửa");
    setTextIfChanged(repairSummary, `${blocked} mục cần được giữ nguyên vì CoffeePOS chưa có đủ ownership/authority để sửa tự động.`);
  }
  setRepairControls();
}

function renderRepairCommandError(error: unknown): void {
  currentRepairPlan = null;
  setTextIfChanged(repairInspect, "Kiểm tra lại");
  setTextIfChanged(repairSummaryState, "Không thể kiểm tra");
  setTextIfChanged(repairSummary, "CoffeePOS chưa tạo được repair plan an toàn cho store hiện tại.");
  repairList.replaceChildren();
  repairAdminInput.hidden = true;
  clearRepairPasswordFields();
  repairError.textContent = nativeErrorText(error);
  repairError.hidden = false;
  setRepairControls();
}

async function refreshRepairPlan(): Promise<void> {
  if (!isTauri() || repairOperation || bootstrapBusy || provisioningBusy || runtimeBusy || diagnosticsBusy || backupSystemBusy()) return;
  if (currentProvisioning?.state !== "ready" && currentProvisioning?.state !== "needs_repair") return;
  repairOperation = "inspect";
  currentRepairPlan = null;
  setTextIfChanged(repairSummaryState, "Đang kiểm tra");
  setTextIfChanged(repairSummary, "Đang kiểm tra ownership, pinned artifacts và protected credential state…");
  setTextIfChanged(repairStatus, "Đang lập repair plan read-only…");
  repairError.hidden = true;
  repairOpenDiagnostics.hidden = true;
  setRepairControls();
  setRuntimeControls(currentRuntime);
  setHealthControls();
  try {
    renderRepairPlan(await invoke<RepairPlan>("get_repair_plan"));
    setTextIfChanged(repairStatus, "Repair plan đã được tạo từ trạng thái native hiện tại.");
  } catch (error) {
    renderRepairCommandError(error);
    setTextIfChanged(repairStatus, nativeErrorText(error));
  } finally {
    repairOperation = null;
    setRepairControls();
    setRuntimeControls(currentRuntime);
    setHealthControls();
  }
}

function validateRepairAdminPassword(): string | null {
  repairAdminError.hidden = true;
  repairAdminError.textContent = "";
  repairAdminPassword.removeAttribute("aria-invalid");
  repairAdminPasswordConfirm.removeAttribute("aria-invalid");
  const password = repairAdminPassword.value;
  const confirmation = repairAdminPasswordConfirm.value;
  const count = Array.from(password).length;
  if (count < 12 || count > 128 || /[\u0000-\u001f\u007f]/.test(password)) {
    repairAdminPassword.setAttribute("aria-invalid", "true");
    repairAdminError.textContent = "Mật khẩu phải có 12–128 ký tự và không chứa ký tự điều khiển.";
    repairAdminError.hidden = false;
    repairAdminPassword.focus();
    return null;
  }
  if (password !== confirmation) {
    repairAdminPasswordConfirm.setAttribute("aria-invalid", "true");
    repairAdminError.textContent = "Hai lần nhập mật khẩu chưa khớp.";
    repairAdminError.hidden = false;
    repairAdminPasswordConfirm.focus();
    return null;
  }
  return password;
}

function renderRepairResult(result: RepairApplyResult, previousPlan: RepairPlan): void {
  currentRepairPlan = null;
  setTextIfChanged(repairInspect, "Kiểm tra lại");
  repairList.replaceChildren();
  repairAdminInput.hidden = true;
  clearRepairPasswordFields();
  const targets = new Map(previousPlan.items.map((item) => [item.id, item.target]));
  for (const item of result.items) {
    const article = document.createElement("article");
    article.className = "repair-row";
    const heading = document.createElement("div");
    heading.className = "repair-row-heading";
    const target = document.createElement("strong");
    target.textContent = targets.get(item.id) ?? item.id;
    const badge = document.createElement("span");
    badge.className = "state-badge";
    badge.textContent = item.status === "repaired" ? "Đã sửa" : item.status === "blocked" ? "Bị chặn" : "Chưa xử lý";
    heading.append(target, badge);
    const messageNode = document.createElement("p");
    messageNode.className = "repair-summary-text";
    messageNode.textContent = item.message;
    article.append(heading, messageNode);
    repairList.append(article);
  }
  repairError.hidden = !result.last_error;
  repairError.textContent = result.last_error ? structuredErrorText(result.last_error) : "";
  repairOpenDiagnostics.hidden = result.status === "stale";
  if (result.status === "repaired") {
    setTextIfChanged(repairSummaryState, "Đã sửa xong");
    setTextIfChanged(repairSummary, "Các mục repairable đã được khôi phục, verifier đạt và runtime đã được trả về trạng thái vận hành trước khi sửa.");
    setTextIfChanged(repairStatus, "Sửa chữa hoàn tất.");
  } else if (result.status === "stale") {
    setTextIfChanged(repairSummaryState, "Cần kiểm tra lại");
    setTextIfChanged(repairSummary, "Store đã thay đổi sau lần kiểm tra trước. Không có mutation nào được áp dụng từ repair plan cũ.");
    setTextIfChanged(repairStatus, "Kiểm tra lại để lấy repair plan mới.");
  } else {
    setTextIfChanged(repairSummaryState, "Cần xử lý tiếp");
    setTextIfChanged(repairSummary, "Một phần repair đã hoàn tất hoặc còn mục bị chặn/verifier chưa đạt. Dữ liệu store hiện có được giữ nguyên.");
    setTextIfChanged(repairStatus, "Xem lỗi và chạy Kiểm tra lại sau khi xử lý nguyên nhân còn lại.");
  }
  setRepairControls();
}

async function applyRepair(): Promise<void> {
  const plan = currentRepairPlan;
  if (!isTauri() || !plan || !plan.can_apply || repairOperation || bootstrapBusy || provisioningBusy || runtimeBusy || diagnosticsBusy || backupSystemBusy()) return;
  const needsAdminPassword = plan.items.some((item) => item.input_kind === "admin_password");
  let adminPassword: string | null = null;
  if (needsAdminPassword) {
    adminPassword = validateRepairAdminPassword();
    if (adminPassword === null) return;
  }
  repairOperation = "apply";
  setTextIfChanged(repairSummaryState, "Đang sửa chữa");
  setTextIfChanged(repairSummary, "CoffeePOS đang áp dụng repair plan dưới lifecycle lock và sẽ tự kiểm tra lại trước khi kết luận.");
  setTextIfChanged(repairStatus, "Đang sửa chữa hệ thống…");
  repairError.hidden = true;
  setRepairControls();
  setRuntimeControls(currentRuntime);
  setHealthControls();
  const repairPromise = invoke<RepairApplyResult>("apply_repair", {
    planId: plan.plan_id,
    inputs: needsAdminPassword ? { adminPassword } : null,
  });
  clearRepairPasswordFields();
  adminPassword = null;
  try {
    const result = await repairPromise;
    await renderProvisioningWithRepairRouting(result.provisioning_info);
    if (currentView === "diagnostics") selectSystemSection("repair", false, false);
    renderRepairResult(result, plan);
    if (result.health_diagnostics) renderHealthDiagnostics(result.health_diagnostics);
    await refreshRuntime();
  } catch (error) {
    currentRepairPlan = null;
    setTextIfChanged(repairInspect, "Kiểm tra lại");
    repairError.textContent = nativeErrorText(error);
    repairError.hidden = false;
    setTextIfChanged(repairSummaryState, "Sửa chữa chưa hoàn tất");
    setTextIfChanged(repairSummary, "Native repair command chưa hoàn tất. Store được giữ theo repair transaction hiện tại; kiểm tra lại trước khi thử tiếp.");
    setTextIfChanged(repairStatus, nativeErrorText(error));
    await refreshProvisioning();
    await refreshRuntime();
  } finally {
    repairOperation = null;
    setRepairControls();
    setRuntimeControls(currentRuntime);
    setHealthControls();
  }
}

function selectedLogEntry(): LogCatalogEntry | null {
  if (!currentLogCatalog || !currentLogId) return null;
  return currentLogCatalog.logs.find((entry) => entry.id === currentLogId) ?? null;
}

function formatLogBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  if (bytes < 1024) return `${Math.round(bytes)} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(bytes < 10 * 1024 ? 1 : 0)} KiB`;
  return `${(bytes / (1024 * 1024)).toFixed(bytes < 10 * 1024 * 1024 ? 1 : 0)} MiB`;
}

function formatLogTimestamp(value: number | null): string {
  if (!value || !Number.isFinite(value)) return "Chưa có dữ liệu";
  const milliseconds = value < 1_000_000_000_000 ? value * 1000 : value;
  const date = new Date(milliseconds);
  if (Number.isNaN(date.getTime())) return "Không xác định";
  return new Intl.DateTimeFormat("vi-VN", {
    dateStyle: "short",
    timeStyle: "short",
  }).format(date);
}

function logErrorMessage(error: unknown, fallback: string): string {
  if (error && typeof error === "object" && "message" in error) {
    const message = (error as { message?: unknown }).message;
    if (typeof message === "string" && message.trim()) return message;
  }
  return fallback;
}

function setLogControls(): void {
  const busy = logOperation !== null || logExportBusy;
  const entry = selectedLogEntry();
  logsPanel.setAttribute("aria-busy", busy ? "true" : "false");
  logSource.disabled = busy || !currentLogCatalog || currentLogCatalog.logs.length === 0;
  logRefresh.disabled = busy;
  logLoadOlder.disabled = busy || !currentLogHasOlder || !currentLogOlderCursor;
  logExport.disabled = busy;
  logLoadOlder.hidden = !currentLogHasOlder || !currentLogOlderCursor || !entry?.exists;
}

function renderLogMetadata(entry: LogCatalogEntry | null): void {
  if (!entry) {
    setTextIfChanged(logCurrentLabel, "Nhật ký");
    setTextIfChanged(logCurrentMeta, "Chưa có metadata.");
    return;
  }
  setTextIfChanged(logCurrentLabel, entry.label);
  const metadata = entry.exists
    ? `Cập nhật gần nhất: ${formatLogTimestamp(entry.modified_at)} · ${formatLogBytes(entry.size_bytes)}`
    : "Chưa có file nhật ký cho nguồn này.";
  const flags: string[] = [];
  if (currentLogTruncated) flags.push("trang hiện tại đã được giới hạn");
  if (currentLogRedactionCount > 0) flags.push(`đã ẩn ${currentLogRedactionCount} giá trị nhạy cảm`);
  setTextIfChanged(logCurrentMeta, flags.length > 0 ? `${metadata} · ${flags.join(" · ")}` : metadata);
}

function clearLogReadError(): void {
  logReadError.hidden = true;
  logReadErrorDetails.hidden = true;
  logReadErrorDetails.open = false;
  logReadErrorTechnical.textContent = "";
}

function renderLogBody(): void {
  const entry = selectedLogEntry();
  renderLogMetadata(entry);
  const hasLines = currentLogLines.length > 0;
  logContent.hidden = !hasLines;
  logContent.textContent = hasLines ? currentLogLines.join("\n") : "";
  logEmpty.hidden = hasLines || !!entry?.exists || !entry;
  if (!entry) {
    setTextIfChanged(logsReadState, "Chưa tải");
  } else if (!entry.exists) {
    setTextIfChanged(logsReadState, "Chưa có dữ liệu");
  } else if (hasLines) {
    setTextIfChanged(logsReadState, currentLogTruncated ? "Đã giới hạn" : "Đã tải");
  } else {
    setTextIfChanged(logsReadState, "Không có dòng");
    logEmpty.hidden = false;
  }
  setLogControls();
}

function showLogReadError(titleText: string, error: unknown, preserveContent: boolean): void {
  setTextIfChanged(logsReadState, "Không thể đọc");
  setTextIfChanged(logReadErrorTitle, titleText);
  setTextIfChanged(logReadErrorMessage, logErrorMessage(error, "CoffeePOS không thể đọc dữ liệu nhật ký hiện tại."));
  const technical = nativeErrorText(error);
  logReadErrorTechnical.textContent = technical;
  logReadErrorDetails.hidden = technical.length === 0;
  logReadErrorDetails.open = false;
  logReadError.hidden = false;
  if (!preserveContent) {
    currentLogLines = [];
    currentLogOlderCursor = null;
    currentLogHasOlder = false;
    currentLogTruncated = false;
    currentLogRedactionCount = 0;
    logContent.textContent = "";
    logContent.hidden = true;
    logEmpty.hidden = true;
  }
  setLogControls();
}

function renderLogCatalog(catalog: LogCatalog): void {
  const previousId = currentLogId;
  currentLogCatalog = catalog;
  logSource.replaceChildren();
  for (const entry of catalog.logs) {
    const option = document.createElement("option");
    option.value = entry.id;
    option.textContent = entry.exists ? entry.label : `${entry.label} · chưa có dữ liệu`;
    logSource.append(option);
  }

  const nextId = previousId && catalog.logs.some((entry) => entry.id === previousId)
    ? previousId
    : catalog.logs.find((entry) => entry.id === "runtime")?.id ?? catalog.logs[0]?.id ?? null;
  const sourceChanged = nextId !== currentLogId;
  currentLogId = nextId;
  logSource.value = nextId ?? "";
  if (sourceChanged) {
    currentLogLines = [];
    currentLogOlderCursor = null;
    currentLogHasOlder = false;
    currentLogTruncated = false;
    currentLogRedactionCount = 0;
  }

  const available = catalog.logs.filter((entry) => entry.exists).length;
  if (catalog.logs.length === 0) {
    const option = document.createElement("option");
    option.value = "";
    option.textContent = "Không có nguồn nhật ký";
    logSource.append(option);
    setTextIfChanged(logSourceStatus, "Native chưa trả về nguồn nhật ký nào trong allowlist.");
  } else {
    setTextIfChanged(logSourceStatus, `${available}/${catalog.logs.length} nguồn hiện có dữ liệu.`);
  }
  clearLogReadError();
  renderLogBody();
}

async function loadCurrentLogTail(preserveExisting = false): Promise<void> {
  const entry = selectedLogEntry();
  if (!isTauri() || logOperation || logExportBusy || !entry) return;
  if (!entry.exists) {
    currentLogLines = [];
    currentLogOlderCursor = null;
    currentLogHasOlder = false;
    currentLogTruncated = false;
    currentLogRedactionCount = 0;
    clearLogReadError();
    setTextIfChanged(logRefreshStatus, "");
    renderLogBody();
    return;
  }

  logOperation = "read";
  clearLogReadError();
  setTextIfChanged(logsReadState, preserveExisting && currentLogLines.length > 0 ? "Đang làm mới" : "Đang đọc");
  setTextIfChanged(logRefreshStatus, preserveExisting && currentLogLines.length > 0 ? "Đang làm mới…" : "Đang đọc…");
  if (currentLogLines.length === 0) logEmpty.hidden = true;
  setLogControls();
  const requestedLogId = entry.id;
  try {
    const page = await readLogPage(requestedLogId, null, "tail");
    if (page.log_id !== requestedLogId) throw new Error("Native trả về trang nhật ký không khớp nguồn đã chọn.");
    const hitViewerLimit = page.lines.length > LOG_VIEW_MAX_LINES;
    currentLogLines = hitViewerLimit ? page.lines.slice(-LOG_VIEW_MAX_LINES) : page.lines;
    currentLogOlderCursor = page.older_cursor;
    currentLogHasOlder = !hitViewerLimit && page.has_older;
    currentLogTruncated = page.truncated || hitViewerLimit;
    currentLogRedactionCount = page.redaction_count;
    setTextIfChanged(logRefreshStatus, "Đã cập nhật.");
    renderLogBody();
  } catch (error) {
    showLogReadError(`Không thể đọc nhật ký ${entry.label}`, error, preserveExisting && currentLogLines.length > 0);
    setTextIfChanged(logRefreshStatus, "Không thể làm mới.");
  } finally {
    logOperation = null;
    setLogControls();
  }
}

async function refreshLogCatalogAndTail(preserveExisting = true): Promise<void> {
  if (!isTauri() || logOperation || logExportBusy) return;
  logOperation = "catalog";
  clearLogReadError();
  setTextIfChanged(logsReadState, currentLogLines.length > 0 ? "Đang làm mới" : "Đang tải");
  setTextIfChanged(logRefreshStatus, currentLogLines.length > 0 ? "Đang làm mới…" : "Đang tải danh sách…");
  setLogControls();
  let loadTail = false;
  try {
    const catalog = await getLogCatalog();
    renderLogCatalog(catalog);
    loadTail = !!currentLogId;
  } catch (error) {
    showLogReadError("Không thể tải danh sách nhật ký", error, preserveExisting && currentLogLines.length > 0);
    setTextIfChanged(logSourceStatus, "Không thể cập nhật danh sách nguồn nhật ký.");
    setTextIfChanged(logRefreshStatus, "Không thể làm mới.");
  } finally {
    logOperation = null;
    setLogControls();
  }
  if (loadTail) await loadCurrentLogTail(preserveExisting);
}

async function loadOlderLogLines(): Promise<void> {
  const entry = selectedLogEntry();
  const cursor = currentLogOlderCursor;
  if (!isTauri() || logOperation || logExportBusy || !entry?.exists || !cursor || !currentLogHasOlder) return;
  logOperation = "older";
  clearLogReadError();
  setTextIfChanged(logsReadState, "Đang tải thêm");
  setTextIfChanged(logRefreshStatus, "Đang tải dòng cũ hơn…");
  setLogControls();
  const requestedLogId = entry.id;
  try {
    const page = await readLogPage(requestedLogId, cursor, "older");
    if (page.log_id !== requestedLogId) throw new Error("Native trả về trang nhật ký không khớp nguồn đã chọn.");
    const combinedLines = [...page.lines, ...currentLogLines];
    const hitViewerLimit = combinedLines.length > LOG_VIEW_MAX_LINES;
    currentLogLines = hitViewerLimit ? combinedLines.slice(0, LOG_VIEW_MAX_LINES) : combinedLines;
    currentLogOlderCursor = page.older_cursor;
    currentLogHasOlder = !hitViewerLimit && page.has_older;
    currentLogTruncated = currentLogTruncated || page.truncated || hitViewerLimit;
    currentLogRedactionCount += page.redaction_count;
    setTextIfChanged(
      logRefreshStatus,
      hitViewerLimit
        ? "Đã đạt giới hạn 1.000 dòng hiển thị. Chọn Làm mới để quay về phần mới nhất."
        : page.lines.length > 0
          ? "Đã tải thêm dòng cũ."
          : "Không còn dòng cũ hơn.",
    );
    renderLogBody();
  } catch (error) {
    showLogReadError(`Không thể tải dòng cũ của ${entry.label}`, error, true);
    setTextIfChanged(logRefreshStatus, "Không thể tải thêm.");
  } finally {
    logOperation = null;
    setLogControls();
  }
}

async function exportLogsSupportBundle(): Promise<void> {
  if (!isTauri() || logExportBusy || logOperation) return;
  logExportBusy = true;
  setTextIfChanged(logExportStatus, "Đang tạo gói hỗ trợ… CoffeePOS đang thu thập nhật ký và loại bỏ thông tin nhạy cảm.");
  setLogControls();
  try {
    const result = await exportSupportBundle();
    if (result.status === "cancelled") {
      setTextIfChanged(logExportStatus, "");
      return;
    }
    setTextIfChanged(logExportStatus, "Đã xuất gói hỗ trợ. File đã được lưu tại vị trí bạn chọn.");
  } catch (error) {
    const message = logErrorMessage(error, "CoffeePOS chưa thể tạo gói hỗ trợ.");
    setTextIfChanged(logExportStatus, `Không thể xuất gói hỗ trợ. ${message}`);
  } finally {
    logExportBusy = false;
    setLogControls();
  }
}

function setHomeAction(
  kind: "start" | "retry_health" | "refresh" | "open_pos" | null,
  label = "",
  disabled = false,
): void {
  homeActionKind = kind;
  homeAction.hidden = kind === null;
  homeAction.disabled = disabled;
  if (kind !== null) setTextIfChanged(homeAction, label);
}

function applyShellInfo(info: ShellInfo): void {
  element("data-dir").textContent = info.data_dir;
  element("version").textContent = info.version;
  preferredStartupView = info.config.startup_view;
  settingsStartupView.value = preferredStartupView;
}

function setupHeading(step: SetupStep): HTMLElement {
  if (step === "details") return element<HTMLElement>("setup-details-title");
  if (step === "review") return element<HTMLElement>("setup-review-title");
  if (step === "progress") return element<HTMLElement>("setup-progress-title");
  if (step === "complete") return setupCompleteTitle;
  return setupTitle;
}

function selectSetupStep(step: SetupStep, moveFocus = true): void {
  currentSetupStep = step;
  for (const panel of setupPanels) panel.hidden = panel.dataset.setupPanel !== step;
  if (moveFocus) setupHeading(step).focus();
}

function applySetupInfo(info: SetupInfo): void {
  currentSetupInfo = info;
  name.value = info.store_name;
  adminUsername.value = info.admin_username;
  adminEmail.value = info.admin_email;
  adminPassword.required = !info.password_configured;
  adminPasswordConfirm.required = !info.password_configured;
  passwordHint.textContent = info.password_configured
    ? "Mật khẩu đã được lưu bảo mật. Để trống hai ô mật khẩu để giữ nguyên, hoặc nhập mật khẩu mới trước khi bắt đầu cài đặt."
    : "Tối thiểu 12 ký tự. Mật khẩu không được lưu trong draft hoặc URL.";
  name.disabled = !info.editable || setupProfileBusy || provisioningBusy || runtimeBusy || restoreSystemBusy();
  adminUsername.disabled = !info.editable || setupProfileBusy || provisioningBusy || runtimeBusy || restoreSystemBusy();
  adminEmail.disabled = !info.editable || setupProfileBusy || provisioningBusy || runtimeBusy || restoreSystemBusy();
  adminPassword.disabled = !info.editable || setupProfileBusy || provisioningBusy || runtimeBusy || restoreSystemBusy();
  adminPasswordConfirm.disabled = !info.editable || setupProfileBusy || provisioningBusy || runtimeBusy || restoreSystemBusy();
  save.disabled = !info.editable || setupProfileBusy || provisioningBusy || runtimeBusy || restoreSystemBusy();
  provisionWordPress.disabled =
    !info.editable || !info.password_configured || setupProfileBusy || provisioningBusy || runtimeBusy || restoreSystemBusy();
  element("review-store-name").textContent = info.store_name;
  element("review-admin-username").textContent = info.admin_username;
  element("review-admin-email").textContent = info.admin_email;
}

async function refreshSetupInfo(): Promise<SetupInfo | null> {
  try {
    const info = await invoke<SetupInfo>("get_setup_info");
    applySetupInfo(info);
    return info;
  } catch (error) {
    message.textContent = nativeErrorText(error);
    message.hidden = false;
    return null;
  }
}

function clearFieldErrors(): void {
  for (const id of ["store-name", "admin-username", "admin-email", "admin-password", "admin-password-confirm"]) {
    const error = element(`${id}-error`);
    error.textContent = "";
    error.hidden = true;
    element<HTMLInputElement>(id).removeAttribute("aria-invalid");
  }
  message.textContent = "";
  message.hidden = true;
}

function fieldError(input: HTMLInputElement, text: string): false {
  const error = element(`${input.id}-error`);
  error.textContent = text;
  error.hidden = false;
  input.setAttribute("aria-invalid", "true");
  input.focus();
  return false;
}

function validateSetupForm(): boolean {
  clearFieldErrors();
  const storeName = name.value.trim();
  const username = adminUsername.value.trim();
  const email = adminEmail.value.trim();
  const password = adminPassword.value;
  const confirmation = adminPasswordConfirm.value;
  const passwordLength = Array.from(password).length;
  if (!storeName || Array.from(storeName).length > 80 || /[\u0000-\u001f\u007f]/.test(storeName)) {
    return fieldError(name, "Nhập tên cửa hàng từ 1–80 ký tự.");
  }
  if (/[<>]/.test(storeName) || /  /.test(storeName) || /%[0-9A-Fa-f]{2}/.test(storeName)) {
    return fieldError(name, "Tên cửa hàng không được chứa dấu ngoặc HTML, khoảng trắng lặp hoặc chuỗi mã hóa như %20.");
  }
  if (!/^[A-Za-z0-9._-]{3,60}$/.test(username)) {
    return fieldError(adminUsername, "Tên đăng nhập phải có 3–60 ký tự: chữ, số, dấu chấm, gạch dưới hoặc gạch nối.");
  }
  const emailPattern = /^[A-Za-z0-9.+_-]+@[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?(?:\.[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?)+$/;
  const [emailLocal = ""] = email.split("@");
  if (!adminEmail.checkValidity() || email.length > 100 || emailLocal.length > 64 || emailLocal.startsWith(".") || emailLocal.endsWith(".") || emailLocal.includes("..") || !emailPattern.test(email)) {
    return fieldError(adminEmail, "Nhập email quản trị hợp lệ.");
  }
  const passwordRequired = !currentSetupInfo?.password_configured;
  if ((passwordRequired || password.length > 0) && (passwordLength < 12 || passwordLength > 128 || /[\u0000-\u001f\u007f]/.test(password))) {
    return fieldError(adminPassword, "Mật khẩu phải có 12–128 ký tự và không chứa ký tự điều khiển.");
  }
  if (confirmation.length > 0 && password.length === 0) {
    return fieldError(adminPassword, "Nhập mật khẩu mới trước khi nhập lại mật khẩu.");
  }
  if ((passwordRequired || password.length > 0) && password !== confirmation) {
    return fieldError(adminPasswordConfirm, "Hai lần nhập mật khẩu chưa khớp.");
  }
  return true;
}

function viewHeading(view: InstalledView): HTMLElement {
  if (view === "settings") return element<HTMLElement>("settings-title");
  if (view === "diagnostics") return element<HTMLElement>("diagnostics-title");
  return homeTitle;
}

function systemSectionHeading(section: SystemSection): HTMLElement {
  if (section === "repair") return element<HTMLElement>("repair-title");
  if (section === "logs") return element<HTMLElement>("logs-title");
  if (section === "backup") return element<HTMLElement>("backup-title");
  return element<HTMLElement>("health-diagnostics-title");
}

function selectSystemSection(section: SystemSection, moveFocus = true, refresh = true): void {
  if (currentSystemSection === "backup" && section !== "backup" && currentBackupView === "password") {
    clearBackupPasswordFields();
    selectBackupView("landing");
  }
  currentSystemSection = section;
  for (const button of systemSectionButtons) {
    button.setAttribute("aria-current", button.dataset.systemSection === section ? "page" : "false");
  }
  for (const panel of systemSectionPanels) {
    panel.hidden = panel.dataset.systemPanel !== section;
  }
  if (moveFocus) systemSectionHeading(section).focus();
  if (!refresh || currentView !== "diagnostics") return;
  if (section === "logs") void refreshLogCatalogAndTail(true);
  else if (section === "backup") void refreshBackupStatus();
  else if (section === "diagnostics") void refreshHealthDiagnostics();
}

function selectInstalledView(view: InstalledView, moveFocus = true): void {
  if (installedShell.hidden) return;
  currentView = view;
  for (const button of navButtons) {
    button.setAttribute("aria-current", button.dataset.view === view ? "page" : "false");
  }
  for (const panel of viewPanels) {
    panel.hidden = panel.dataset.viewPanel !== view;
  }
  if (view === "diagnostics") {
    selectSystemSection(repairRouteRequired ? "repair" : currentSystemSection, false, false);
  }
  if (moveFocus) viewHeading(view).focus();
  if (view === "diagnostics" && moveFocus) {
    if (currentSystemSection === "logs") void refreshLogCatalogAndTail(true);
    else if (currentSystemSection === "backup") void refreshBackupStatus();
    else if (currentSystemSection === "diagnostics") void refreshHealthDiagnostics();
  }
}

function showBootstrapError(error: unknown): void {
  currentProvisioning = null;
  setup.hidden = true;
  installedShell.hidden = true;
  bootstrapPanel.hidden = false;
  title.textContent = "Không thể đọc trạng thái cửa hàng";
  description.textContent = nativeErrorText(error);
  retry.hidden = false;
}

function applyInstallationLayout(info: ProvisioningInfo): void {
  const repairMode = repairRouteRequired;
  const useInstalledShell = info.state === "ready" || repairMode;
  const enteringInstalledShell = useInstalledShell && installedShell.hidden === true;
  const enteringSetup = !useInstalledShell && setup.hidden === true;
  bootstrapPanel.hidden = true;

  if (!restoreScreen.hidden) {
    setup.hidden = true;
    installedShell.hidden = true;
    return;
  }

  if (useInstalledShell) {
    if (repairMode) {
      completionPending = false;
      setup.hidden = true;
      installedShell.hidden = false;
      selectInstalledView("diagnostics", false);
      selectSystemSection("repair", enteringInstalledShell, enteringInstalledShell);
      return;
    }
    if (completionPending) {
      installedShell.hidden = true;
      setup.hidden = false;
      selectSetupStep("complete", enteringSetup || currentSetupStep !== "complete");
      return;
    }
    setup.hidden = true;
    installedShell.hidden = false;
    if (enteringInstalledShell) selectInstalledView(preferredStartupView, true);
    else selectInstalledView(currentView, false);
    return;
  }

  installedShell.hidden = true;
  setup.hidden = false;
  if (info.state === "not_installed") {
    if (currentSetupStep === "progress" || currentSetupStep === "complete") currentSetupStep = "welcome";
    selectSetupStep(currentSetupStep, enteringSetup);
  } else {
    selectSetupStep("progress", enteringSetup || currentSetupStep !== "progress");
  }
}

function renderHome(): void {
  const payload = currentRuntime?.coffeepos_health.payload;
  const verifiedStoreName = payload?.store.name.trim();
  homeStoreName.textContent = verifiedStoreName || "Cửa hàng CoffeePOS đã cài đặt";
  const posReady = currentRuntime?.state === "running"
    && currentRuntime.wordpress_health === "healthy"
    && currentRuntime.coffeepos_health.state === "healthy";
  if (!posReady && !posOpenBusy) setTextIfChanged(homeOpenStatus, "");
  if (repairRouteRequired) {
    setTextIfChanged(homeState, "Cần sửa chữa");
    setTextIfChanged(homeStatus, "Cửa hàng cần được kiểm tra an toàn");
    setTextIfChanged(homeDetail, "Mở Hệ thống → Sửa chữa để xem repair plan. CoffeePOS sẽ giữ nguyên dữ liệu khi ownership hoặc authority chưa đủ.");
    setTextIfChanged(homeDiagnostics, "Mở Sửa chữa");
    setHomeAction(null);
    return;
  }
  if (restoreSystemBusy()) {
    setTextIfChanged(homeState, restoreNeedsRecovery() ? "Cần xử lý" : "Đang khôi phục");
    setTextIfChanged(homeStatus, restoreNeedsRecovery() ? "Khôi phục cần được kiểm tra" : "CoffeePOS đang khôi phục cửa hàng");
    setTextIfChanged(
      homeDetail,
      restoreNeedsRecovery()
        ? "CoffeePOS đang giữ admission gate để bảo vệ dữ liệu phục hồi. Mở luồng khôi phục để xem trạng thái."
        : "POS và các thao tác thay đổi hệ thống tạm khóa trong khi restore transaction đang hoạt động.",
    );
    setTextIfChanged(homeDiagnostics, "Mở khôi phục");
    setHomeAction(null);
    return;
  }
  if (backupOperation === "create" || backupOperation === "cancel" || backupIsActive()) {
    setTextIfChanged(homeState, "Đang sao lưu");
    setTextIfChanged(homeStatus, "CoffeePOS đang sao lưu cửa hàng");
    setTextIfChanged(homeDetail, "POS tạm dừng trong khi CoffeePOS tạo snapshot nhất quán. Mở Sao lưu và khôi phục để xem tiến độ.");
    setTextIfChanged(homeDiagnostics, "Mở Sao lưu");
    setHomeAction(null);
    return;
  }
  setTextIfChanged(homeDiagnostics, "Mở Hệ thống");

  if (runtimeTransition === "starting") {
    setTextIfChanged(homeState, "Đang khởi động");
    setTextIfChanged(homeStatus, "Đang khởi động cửa hàng…");
    setTextIfChanged(homeDetail, "CoffeePOS đang chuẩn bị hệ thống và kiểm tra cửa hàng trước khi báo sẵn sàng.");
    setHomeAction("start", "Đang khởi động…", true);
    return;
  }

  if (runtimeTransition === "stopping") {
    setTextIfChanged(homeState, "Đang dừng");
    setTextIfChanged(homeStatus, "Đang dừng hệ thống…");
    setTextIfChanged(homeDetail, "Vui lòng chờ thao tác hiện tại hoàn tất.");
    setHomeAction("start", "Đang dừng…", true);
    return;
  }

  if (runtimeTransition === "checking") {
    setTextIfChanged(homeState, "Đang kiểm tra");
    setTextIfChanged(homeStatus, "Đang kiểm tra lại cửa hàng…");
    setTextIfChanged(homeDetail, "CoffeePOS đang kiểm tra lại trạng thái sử dụng hiện tại.");
    setHomeAction("retry_health", "Đang kiểm tra…", true);
    return;
  }

  if (!currentRuntime) {
    if (runtimeLoadError) {
      setTextIfChanged(homeState, "Có lỗi");
      setTextIfChanged(homeStatus, "Không thể đọc trạng thái cửa hàng");
      setTextIfChanged(homeDetail, "Thử đọc lại trạng thái hoặc mở Hệ thống để xem chi tiết.");
      setHomeAction("refresh", "Thử lại");
    } else {
      setTextIfChanged(homeState, "Đang kiểm tra");
      setTextIfChanged(homeStatus, "Đang đọc trạng thái hệ thống…");
      setTextIfChanged(homeDetail, "CoffeePOS đang đọc trạng thái cửa hàng trên máy này.");
      setHomeAction(null);
    }
    return;
  }

  if (currentRuntime.state === "stopped") {
    if (currentRuntime.last_error) {
      setTextIfChanged(homeState, "Có lỗi");
      setTextIfChanged(homeStatus, "Không thể khởi động hệ thống");
      setTextIfChanged(homeDetail, "Lần khởi động gần nhất chưa hoàn tất. Bạn có thể thử lại hoặc xem chi tiết trong Hệ thống.");
      setHomeAction("start", "Thử lại");
    } else {
      setTextIfChanged(homeState, "Đã cài đặt");
      setTextIfChanged(homeStatus, "Hệ thống đang dừng");
      setTextIfChanged(homeDetail, "Cửa hàng đã được cài đặt và chưa chạy trên máy này.");
      setHomeAction("start", "Khởi động");
    }
    return;
  }

  if (currentRuntime.state === "starting" || currentRuntime.state === "installing") {
    setTextIfChanged(homeState, "Đang khởi động");
    setTextIfChanged(homeStatus, "Đang khởi động cửa hàng…");
    setTextIfChanged(homeDetail, "CoffeePOS đang chuẩn bị hệ thống và kiểm tra cửa hàng trước khi báo sẵn sàng.");
    setHomeAction("start", "Đang khởi động…", true);
    return;
  }

  if (currentRuntime.state === "stopping") {
    setTextIfChanged(homeState, "Đang dừng");
    setTextIfChanged(homeStatus, "Đang dừng hệ thống…");
    setTextIfChanged(homeDetail, "Vui lòng chờ thao tác hiện tại hoàn tất.");
    setHomeAction("start", "Đang dừng…", true);
    return;
  }

  if (currentRuntime.state !== "running") {
    setTextIfChanged(homeState, "Có lỗi");
    setTextIfChanged(homeStatus, "Hệ thống chưa sẵn sàng");
    setTextIfChanged(homeDetail, "Mở Hệ thống để xem chi tiết trạng thái hiện tại.");
    setHomeAction(null);
    return;
  }

  if (currentRuntime.wordpress_health === "checking") {
    setTextIfChanged(homeState, "Đang kiểm tra");
    setTextIfChanged(homeStatus, "Đang kiểm tra cửa hàng…");
    setTextIfChanged(homeDetail, "Hệ thống đã chạy và đang hoàn tất kiểm tra sẵn sàng.");
    setHomeAction(null);
    return;
  }

  if (currentRuntime.wordpress_health !== "healthy") {
    setTextIfChanged(homeState, "Có lỗi");
    setTextIfChanged(homeStatus, "Cửa hàng chưa sẵn sàng");
    setTextIfChanged(homeDetail, "Hệ thống đang chạy nhưng kiểm tra cửa hàng chưa đạt. Bạn có thể thử lại mà không cần cài đặt lại.");
    setHomeAction("retry_health", "Thử lại");
    return;
  }

  if (currentRuntime.coffeepos_health.state === "healthy") {
    setTextIfChanged(homeState, "Sẵn sàng");
    setTextIfChanged(homeStatus, "Hệ thống đã sẵn sàng");
    setTextIfChanged(homeDetail, "Cửa hàng đang chạy. Mở POS trong trình duyệt và đăng nhập nếu được yêu cầu.");
    setHomeAction("open_pos", posOpenBusy ? "Đang mở…" : "Mở bán hàng", posOpenBusy);
    return;
  }

  if (currentRuntime.coffeepos_health.state === "degraded") {
    setTextIfChanged(homeState, "Cần kiểm tra");
    setTextIfChanged(homeStatus, "Cửa hàng chưa sẵn sàng");
    setTextIfChanged(homeDetail, "Một thành phần của cửa hàng chưa sẵn sàng. Thử kiểm tra lại hoặc xem chi tiết trong Hệ thống.");
    setHomeAction("retry_health", "Thử lại");
    return;
  }

  if (currentRuntime.coffeepos_health.state === "failed") {
    setTextIfChanged(homeState, "Có lỗi");
    setTextIfChanged(homeStatus, "Không thể kiểm tra cửa hàng");
    setTextIfChanged(homeDetail, "Kiểm tra ứng dụng chưa hoàn tất. Bạn có thể thử lại mà không cần cài đặt lại.");
    setHomeAction("retry_health", "Thử lại");
    return;
  }

  setTextIfChanged(homeState, "Đang kiểm tra");
  setTextIfChanged(homeStatus, "Đang kiểm tra cửa hàng…");
  setTextIfChanged(homeDetail, "Hệ thống đang chạy và đang hoàn tất kiểm tra sẵn sàng.");
  setHomeAction(null);
}

function setRuntimeControls(info: RuntimeInfo | null): void {
  if (provisioningBusy || runtimeBusy || diagnosticsBusy || repairOperation !== null || backupSystemBusy() || repairRouteRequired || !info) {
    runtimeStart.disabled = true;
    runtimeStop.disabled = true;
    runtimeRestart.disabled = true;
    openWordPress.disabled = true;
    return;
  }

  const provisioningReady = currentProvisioning?.state === "ready";
  if (!provisioningReady) {
    runtimeStart.disabled = true;
    runtimeRestart.disabled = true;
    runtimeStop.disabled = info.state !== "running";
    openWordPress.disabled = true;
    return;
  }

  runtimeStart.disabled = info.state === "running" || info.state === "starting" || info.state === "stopping";
  runtimeStop.disabled = info.state !== "running";
  runtimeRestart.disabled = info.state === "starting" || info.state === "stopping" || info.state === "installing";
  openWordPress.disabled = info.state !== "running" || info.wordpress_health !== "healthy" || !info.http_port;
}

function renderRuntime(info: RuntimeInfo): void {
  currentRuntime = info;
  element("runtime-state").textContent = info.state;
  element("runtime-php").textContent = info.php_version ?? "—";
  element("runtime-mariadb").textContent = info.mariadb_version ?? "—";
  element("runtime-http").textContent = info.http_port ? `127.0.0.1:${info.http_port}` : "—";
  element("runtime-database").textContent = info.database_port ? `127.0.0.1:${info.database_port}` : "—";
  wordpressHealth.textContent = info.wordpress_health;
  setHiddenIfChanged(wordpressHealthError, !info.wordpress_error);
  setTextIfChanged(wordpressHealthError, info.wordpress_error ? structuredErrorText(info.wordpress_error) : "");
  coffeeposHealth.textContent = info.coffeepos_health.state;
  setHiddenIfChanged(coffeeposHealthError, !info.coffeepos_health.error);
  setTextIfChanged(
    coffeeposHealthError,
    info.coffeepos_health.error
      ? `${info.coffeepos_health.failure_kind ? `${info.coffeepos_health.failure_kind}: ` : ""}${structuredErrorText(info.coffeepos_health.error)}`
      : "",
  );
  const appHealth = info.coffeepos_health.payload;
  coffeeposHealthDetails.textContent = appHealth
    ? `${appHealth.store.name} · WP ${appHealth.versions.wordpress} [${appHealth.wordpress ? "ready" : "not ready"}] · Woo ${appHealth.versions.woocommerce} [${appHealth.woocommerce ? "ready" : "not ready"}] · CoffeePOS ${appHealth.versions.coffeepos} [${appHealth.coffeepos ? "ready" : "not ready"}] · schema ${appHealth.versions.coffeepos_schema} · DB ${appHealth.database ? "ready" : "not ready"} · POS ${appHealth.pos_path}`
    : "";
  setRuntimeControls(info);
  setHealthControls();

  if (provisioningBusy) {
    setTextIfChanged(runtimeDescription, "Runtime controls tạm khóa trong khi CoffeePOS đang được provision.");
  } else if (runtimeBusy) {
    setTextIfChanged(runtimeDescription, "Đang cập nhật runtime…");
  } else if (info.last_error) {
    setTextIfChanged(runtimeDescription, `${info.last_error.message} ${info.last_error.recovery}`);
  } else if (info.state === "not_installed") {
    setTextIfChanged(runtimeDescription, "Runtime bundle đã sẵn sàng; WordPress/database chưa được provision.");
  } else if (info.state === "running") {
    if (info.wordpress_health === "healthy") {
      if (info.coffeepos_health.state === "healthy") {
        setTextIfChanged(runtimeDescription, "MariaDB, PHP, WordPress và CoffeePOS application health đều healthy.");
      } else if (info.coffeepos_health.state === "degraded") {
        setTextIfChanged(runtimeDescription, "Runtime và WordPress đang chạy, nhưng CoffeePOS báo dependency/application state degraded.");
      } else if (info.coffeepos_health.state === "failed") {
        setTextIfChanged(runtimeDescription, "WordPress đang healthy nhưng CoffeePOS machine-health probe thất bại. Xem phân loại lỗi bên dưới.");
      } else {
        setTextIfChanged(runtimeDescription, "MariaDB, PHP và WordPress đã vượt qua readiness checks; CoffeePOS application health chưa được xác minh.");
      }
    } else if (info.wordpress_health === "unhealthy") {
      setTextIfChanged(runtimeDescription, "MariaDB và PHP đang chạy, nhưng WordPress chưa healthy. Có thể khởi động lại runtime để thử lại; không cần cài lại WordPress.");
    } else {
      setTextIfChanged(runtimeDescription, "MariaDB và PHP đang chạy; đang xác minh WordPress.");
    }
  } else if (info.state === "stopped") {
      setTextIfChanged(runtimeDescription, "Runtime đang dừng. Có thể khởi động lại từ Hệ thống khi cần.");
  } else {
    setTextIfChanged(runtimeDescription, "Runtime manager đã sẵn sàng.");
  }

  renderHome();
  if (
    currentView === "diagnostics"
    && currentDiagnostics?.runtime_state === "running"
    && info.state !== "running"
    && !diagnosticsBusy
  ) {
    void refreshHealthDiagnostics();
  }
}

function renderProvisioning(info: ProvisioningInfo, commandError?: string): void {
  currentProvisioning = info;
  provisioningAction = "provision";
  setup.setAttribute("aria-busy", provisioningBusy ? "true" : "false");
  provisioningState.textContent = info.state;
  provisioningWordPress.textContent = info.wordpress_version || "—";
  provisioningWooCommerce.textContent = info.woocommerce_version
    ? `${info.woocommerce_version} · ${info.woocommerce_active ? "active" : "not active"}`
    : "—";
  provisioningCoffeePos.textContent = info.coffeepos_version
    ? `${info.coffeepos_version} · ${info.coffeepos_active ? "active" : "not active"}`
    : "—";
  provisioningAdmin.textContent = info.admin_username ?? "—";
  settingsAdminUsername.textContent = info.admin_username ?? "—";
  element("complete-admin-username").textContent = info.admin_username ?? "—";
  provisioningDetails.hidden = false;
  provisioningError.hidden = true;
  provisioningError.textContent = "";

  if (commandError) {
    provisioningError.textContent = commandError;
    provisioningError.hidden = false;
  } else if (info.last_error) {
    provisioningError.textContent = structuredErrorText(info.last_error);
    provisioningError.hidden = false;
  }

  if (provisioningBusy || info.state === "installing") {
    provisioningState.textContent = "installing";
    provisioningStatus.textContent = "Đang đảm bảo database, WordPress, WooCommerce và CoffeePOS. Nếu app bị đóng ngoài ý muốn, lần mở lại sẽ đọc provisioning journal và tiếp tục từ mốc an toàn.";
    setupRetry.textContent = "Đang cài đặt…";
    setupRetry.hidden = false;
    setupRetry.disabled = true;
  } else if (info.state === "not_installed") {
    provisioningStatus.textContent = "Cửa hàng chưa được thiết lập trên máy này.";
    setupRetry.hidden = true;
    provisionWordPress.disabled = setupProfileBusy || provisioningBusy || runtimeBusy || restoreSystemBusy() || !currentSetupInfo?.password_configured;
  } else if (info.state === "ready") {
    provisioningStatus.textContent = "CoffeePOS đã được cài đặt và activation baseline đã hoàn tất.";
    setupRetry.hidden = true;
  } else if (info.can_retry) {
    provisioningStatus.textContent = "Thiết lập chưa hoàn tất. Có thể tiếp tục từ checkpoint an toàn mà không xóa dữ liệu hiện có.";
    setupRetry.textContent = "Tiếp tục thiết lập";
    setupRetry.hidden = false;
    setupRetry.disabled = runtimeBusy || provisioningBusy;
    provisioningAction = "provision";
  } else {
    provisioningStatus.textContent = "Cửa hàng cần được kiểm tra trước khi setup có thể tiếp tục an toàn. Dữ liệu hiện có được giữ nguyên.";
    setupRetry.textContent = "Kiểm tra lại trạng thái";
    setupRetry.hidden = false;
    setupRetry.disabled = runtimeBusy || provisioningBusy;
    provisioningAction = "refresh";
  }

  if (currentSetupInfo) applySetupInfo(currentSetupInfo);
  applyInstallationLayout(info);
  setRuntimeControls(currentRuntime);
  renderHome();
}

async function renderProvisioningWithRepairRouting(
  info: ProvisioningInfo,
  commandError?: string,
): Promise<void> {
  const enteringRepairState = info.state === "needs_repair" && currentProvisioning?.state !== "needs_repair";
  repairRouteRequired = info.state === "needs_repair";
  if (enteringRepairState) {
    resetRepairInspectionState();
  } else if (!repairRouteRequired) {
    repairRouteRequired = false;
    currentRepairPlan = null;
  }
  renderProvisioning(info, commandError);
  setRepairControls();
  setBackupControls();
  setRestoreControls();
}

async function refreshProvisioning(commandError?: string): Promise<boolean> {
  try {
    const info = await invoke<ProvisioningInfo>("get_provisioning_info");
    await renderProvisioningWithRepairRouting(info, commandError);
    return true;
  } catch (error) {
    showBootstrapError(commandError ?? error);
    setRuntimeControls(null);
    return false;
  }
}

async function refreshRuntime(): Promise<void> {
  if (runtimeRefreshBusy || backupSystemBusy()) return;
  runtimeRefreshBusy = true;
  try {
    runtimeLoadError = null;
    renderRuntime(await invoke<RuntimeInfo>("get_runtime_info"));
  } catch (error) {
    currentRuntime = null;
    runtimeLoadError = nativeErrorText(error);
    setTextIfChanged(runtimeDescription, runtimeLoadError);
    setRuntimeControls(null);
    renderHome();
  } finally {
    runtimeRefreshBusy = false;
  }
}

async function refreshRuntimeMaintenance(): Promise<void> {
  if (runtimeMaintenanceBusy || runtimeBusy || provisioningBusy || diagnosticsBusy || repairOperation || backupSystemBusy()) return;
  runtimeMaintenanceBusy = true;
  try {
    renderRuntime(await invoke<RuntimeInfo>("refresh_runtime_maintenance"));
  } catch {
    // Normal status polling remains the user-visible fallback if maintenance is temporarily unavailable.
  } finally {
    runtimeMaintenanceBusy = false;
  }
}

async function provision(): Promise<void> {
  if (provisioningBusy || runtimeBusy || backupSystemBusy()) return;
  provisioningBusy = true;
  selectSetupStep("progress", true);
  const installingInfo: ProvisioningInfo = currentProvisioning ?? {
    state: "installing",
    wordpress_version: "",
    woocommerce_version: "",
    woocommerce_active: false,
    coffeepos_version: "",
    coffeepos_active: false,
    admin_username: null,
    can_retry: false,
    last_error: null,
  };
  renderProvisioning({ ...installingInfo, state: "installing", last_error: null });
  if (currentRuntime) renderRuntime(currentRuntime);

  let result: ProvisioningInfo | null = null;
  let failure: string | null = null;
  try {
    result = await invoke<ProvisioningInfo>("provision_wordpress");
  } catch (error) {
    failure = nativeErrorText(error);
  } finally {
    provisioningBusy = false;
  }

  if (result) {
    completionPending = result.state === "ready";
    await renderProvisioningWithRepairRouting(result);
  } else {
    await refreshProvisioning(failure ?? "Provisioning thất bại nhưng native layer không trả chi tiết lỗi.");
  }
  await refreshRuntime();
}

async function copyAdminPassword(status: HTMLElement, button: HTMLButtonElement): Promise<void> {
  if (provisioningBusy || runtimeBusy || repairOperation || backupSystemBusy()) return;
  button.disabled = true;
  setTextIfChanged(status, "Đang sao chép…");
  try {
    await invoke<void>("copy_admin_password");
    setTextIfChanged(status, "Đã sao chép mật khẩu vào clipboard Windows.");
  } catch (error) {
    setTextIfChanged(status, nativeErrorText(error));
  } finally {
    button.disabled = false;
  }
}

async function runtimeAction(command: "start_runtime" | "stop_runtime" | "restart_runtime" | "retry_runtime_health"): Promise<void> {
  if (provisioningBusy || runtimeBusy || diagnosticsBusy || repairOperation || backupSystemBusy() || repairRouteRequired || currentProvisioning?.state !== "ready") return;
  runtimeBusy = true;
  runtimeTransition = command === "stop_runtime" ? "stopping" : command === "retry_runtime_health" ? "checking" : "starting";
  if (currentProvisioning) renderProvisioning(currentProvisioning);
  setRuntimeControls(null);
  if (command !== "retry_runtime_health") element("runtime-state").textContent = runtimeTransition;
  wordpressHealth.textContent = "unavailable";
  setHiddenIfChanged(wordpressHealthError, true);
  setTextIfChanged(wordpressHealthError, "");
  coffeeposHealth.textContent = "unavailable";
  setHiddenIfChanged(coffeeposHealthError, true);
  setTextIfChanged(coffeeposHealthError, "");
  coffeeposHealthDetails.textContent = "";
  openWordPressStatus.textContent = "";
  homeOpenStatus.textContent = "";
  if (currentView === "diagnostics") {
    if (command === "stop_runtime") {
      renderHealthChecking("Đang dừng", "Runtime đang dừng; kết quả health cũ đã được loại khỏi màn hình.");
    } else if (command === "restart_runtime") {
      renderHealthChecking("Đang khởi động lại", "Runtime đang khởi động lại; health sẽ được kiểm tra trên process mới.");
    } else {
      renderHealthChecking();
    }
  }
  runtimeDescription.textContent = command === "stop_runtime"
    ? "Đang dừng PHP và MariaDB…"
    : command === "retry_runtime_health"
      ? "Đang kiểm tra lại WordPress và CoffeePOS health…"
      : "Đang khởi động runtime và kiểm tra WordPress…";
  renderHome();

  try {
    renderRuntime(await invoke<RuntimeInfo>(command));
  } catch (error) {
    runtimeDescription.textContent = nativeErrorText(error);
    await refreshRuntime();
  } finally {
    runtimeBusy = false;
    runtimeTransition = null;
    if (currentProvisioning) renderProvisioning(currentProvisioning);
    if (currentRuntime) renderRuntime(currentRuntime);
  }
  if (currentView === "diagnostics") await refreshHealthDiagnostics();
}

async function runHomeAction(): Promise<void> {
  if (homeAction.disabled || provisioningBusy || runtimeBusy || diagnosticsBusy || repairOperation || backupSystemBusy() || posOpenBusy) return;
  if (homeActionKind === "start") {
    await runtimeAction("start_runtime");
  } else if (homeActionKind === "retry_health") {
    await runtimeAction("retry_runtime_health");
  } else if (homeActionKind === "refresh") {
    homeAction.disabled = true;
    await refreshRuntime();
    renderHome();
  } else if (homeActionKind === "open_pos") {
    await openPos();
  }
}

async function saveAppSettings(): Promise<void> {
  if (settingsBusy || backupSystemBusy()) return;
  settingsBusy = true;
  settingsSave.disabled = true;
  setTextIfChanged(settingsSaveStatus, "Đang lưu…");
  try {
    const info = await invoke<ShellInfo>("save_app_settings", { startupView: settingsStartupView.value });
    applyShellInfo(info);
    setTextIfChanged(settingsSaveStatus, "Đã lưu cấu hình Desktop.");
  } catch (error) {
    setTextIfChanged(settingsSaveStatus, nativeErrorText(error));
  } finally {
    settingsBusy = false;
    settingsSave.disabled = backupSystemBusy();
  }
}

async function openManagedWordPress(): Promise<void> {
  if (provisioningBusy || runtimeBusy || repairOperation || backupSystemBusy() || repairRouteRequired || openWordPress.disabled) return;
  openWordPress.disabled = true;
  openWordPressStatus.textContent = "Đang mở WordPress bằng địa chỉ runtime hiện tại…";
  try {
    const url = await invoke<string>("open_wordpress");
    openWordPressStatus.textContent = `Đã yêu cầu mở ${url}`;
  } catch (error) {
    openWordPressStatus.textContent = nativeErrorText(error);
    await refreshRuntime();
  } finally {
    setRuntimeControls(currentRuntime);
  }
}

async function openPos(): Promise<void> {
  if (provisioningBusy || runtimeBusy || repairOperation || backupSystemBusy() || repairRouteRequired || posOpenBusy || currentProvisioning?.state !== "ready") return;
  posOpenBusy = true;
  renderHome();
  setTextIfChanged(homeOpenStatus, "Đang yêu cầu mở POS trong trình duyệt…");
  let feedback = "";
  try {
    await invoke<string>("open_pos");
    feedback = "Đã yêu cầu mở trình duyệt. Đăng nhập trong CoffeePOS nếu được yêu cầu.";
  } catch (error) {
    feedback = nativeErrorText(error);
    await refreshRuntime();
  } finally {
    posOpenBusy = false;
    renderHome();
    setTextIfChanged(homeOpenStatus, feedback);
  }
}

async function bootstrap(): Promise<void> {
  if (bootstrapBusy) return;
  bootstrapBusy = true;
  setup.hidden = true;
  installedShell.hidden = true;
  bootstrapPanel.hidden = false;
  retry.hidden = true;
  retry.disabled = true;
  title.textContent = "Đang mở cửa hàng…";
  description.textContent = "Đang đọc trạng thái cài đặt.";

  if (!isTauri()) {
    title.textContent = "Bản xem trước giao diện";
    description.textContent = "Chạy npm run dev để mở ứng dụng desktop. Bản xem trước không gọi native provisioning hoặc runtime.";
    bootstrapBusy = false;
    return;
  }

  try {
    try {
      const info = await invoke<ShellInfo>("get_shell_info");
      applyShellInfo(info);
    } catch (error) {
      showBootstrapError(error);
      return;
    }

    // Restore recovery must be read before provisioning/runtime work so a WebView reload cannot
    // race daily startup while a restore journal still owns the external admission gate.
    await refreshRestoreStatus();
    if (restoreIsActive() || restoreNeedsRecovery()) return;

    // Reconnect to an active native backup before provisioning/runtime reads. Backup status and
    // cancellation remain lock-independent so a WebView reload can resume progress immediately.
    await refreshBackupStatus();
    const provisioningLoaded = await refreshProvisioning();
    if (!provisioningLoaded) return;
    if (currentProvisioning?.state === "not_installed") {
      await refreshSetupInfo();
      if (currentProvisioning) renderProvisioning(currentProvisioning);
    }
    if (!backupSystemBusy()) await refreshRuntime();
    if (currentProvisioning?.state === "ready" && !repairRouteRequired && !backupSystemBusy() && currentRuntime?.state === "stopped") {
      await runtimeAction("start_runtime");
    }
  } finally {
    bootstrapBusy = false;
    if (!retry.hidden) retry.disabled = false;
    if (currentView === "diagnostics") {
      if (currentSystemSection === "logs") void refreshLogCatalogAndTail(true);
      else if (currentSystemSection === "backup") void refreshBackupStatus();
      else if (currentSystemSection === "diagnostics") void refreshHealthDiagnostics();
    }
  }
}

retry.addEventListener("click", () => void bootstrap());
setupRetry.addEventListener("click", () => {
  if (provisioningAction === "refresh") void refreshProvisioning();
  else void provision();
});
provisionWordPress.addEventListener("click", () => void provision());
setupBegin.addEventListener("click", () => selectSetupStep("details", true));
setupRestore.addEventListener("click", () => beginRestore("fresh"));
setupBack.addEventListener("click", () => selectSetupStep("welcome", true));
setupEdit.addEventListener("click", () => selectSetupStep("details", true));
completeContinue.addEventListener("click", () => {
  completionPending = false;
  if (currentProvisioning) applyInstallationLayout(currentProvisioning);
});
completeCopyPassword.addEventListener("click", () => void copyAdminPassword(completeCopyStatus, completeCopyPassword));
settingsCopyPassword.addEventListener("click", () => void copyAdminPassword(settingsCopyStatus, settingsCopyPassword));
appSettingsForm.addEventListener("submit", (event) => {
  event.preventDefault();
  void saveAppSettings();
});

for (const button of navButtons) {
  button.addEventListener("click", () => {
    const view = button.dataset.view as InstalledView | undefined;
    if (view) selectInstalledView(view, true);
  });
}

for (const button of systemSectionButtons) {
  button.addEventListener("click", () => {
    const section = button.dataset.systemSection as SystemSection | undefined;
    if (section) selectSystemSection(section, true, true);
  });
}

homeAction.addEventListener("click", () => void runHomeAction());
homeDiagnostics.addEventListener("click", () => {
  selectInstalledView("diagnostics", true);
  if (backupSystemBusy()) selectSystemSection("backup", true, true);
});
runtimeStart.addEventListener("click", () => void runtimeAction("start_runtime"));
runtimeStop.addEventListener("click", () => void runtimeAction("stop_runtime"));
runtimeRestart.addEventListener("click", () => void runtimeAction("restart_runtime"));
healthRecheck.addEventListener("click", () => void refreshHealthDiagnostics());
repairInspect.addEventListener("click", () => void refreshRepairPlan());
repairApply.addEventListener("click", () => void applyRepair());
repairOpenDiagnostics.addEventListener("click", () => selectSystemSection("diagnostics", true, true));
logSource.addEventListener("change", () => {
  if (logOperation || logExportBusy) return;
  currentLogId = logSource.value || null;
  currentLogLines = [];
  currentLogOlderCursor = null;
  currentLogHasOlder = false;
  currentLogTruncated = false;
  currentLogRedactionCount = 0;
  clearLogReadError();
  setTextIfChanged(logRefreshStatus, "");
  renderLogBody();
  void loadCurrentLogTail(false);
});
logRefresh.addEventListener("click", () => void refreshLogCatalogAndTail(true));
logLoadOlder.addEventListener("click", () => void loadOlderLogLines());
logExport.addEventListener("click", () => void exportLogsSupportBundle());
backupCreate.addEventListener("click", () => showBackupPasswordStep());
backupCreateAnother.addEventListener("click", () => showBackupPasswordStep());
backupRetry.addEventListener("click", () => showBackupPasswordStep());
backupPasswordCancel.addEventListener("click", () => {
  clearBackupPasswordFields();
  setTextIfChanged(backupState, "Sẵn sàng");
  selectBackupView("landing", true);
  setBackupControls();
});
backupPasswordForm.addEventListener("submit", (event) => {
  event.preventDefault();
  void createBackup();
});
backupCancel.addEventListener("click", () => void cancelBackup());
backupOpenFolder.addEventListener("click", () => void openBackupFolder());
restoreInstalledStart.addEventListener("click", () => beginRestore("installed"));
restorePasswordForm.addEventListener("submit", (event) => {
  event.preventDefault();
  void inspectRestoreBackup();
});
restorePasswordCancel.addEventListener("click", () => void leaveRestoreFlow(false));
restoreReviewCancel.addEventListener("click", () => void leaveRestoreFlow(false));
restoreApply.addEventListener("click", () => void applyRestore());
restoreCancel.addEventListener("click", () => void cancelRestore());
restoreOpenHome.addEventListener("click", () => void leaveRestoreFlow(true));
restoreFailureBack.addEventListener("click", () => void leaveRestoreFlow(false));
restoreRetry.addEventListener("click", () => beginRestore(restoreEntrySource));
restoreRecoveryRefresh.addEventListener("click", () => void refreshRestoreStatus(true));
openWordPress.addEventListener("click", () => void openManagedWordPress());

setupForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  if (setupProfileBusy || provisioningBusy || runtimeBusy || backupSystemBusy() || currentProvisioning?.state !== "not_installed") return;
  if (!validateSetupForm()) return;
  setupProfileBusy = true;
  save.disabled = true;
  message.hidden = true;
  const password = adminPassword.value;
  try {
    const info = await invoke<SetupInfo>("save_setup_profile", {
      storeName: name.value.trim(),
      adminUsername: adminUsername.value.trim(),
      adminEmail: adminEmail.value.trim(),
      adminPassword: password.length > 0 ? password : null,
    });
    adminPassword.value = "";
    adminPasswordConfirm.value = "";
    applySetupInfo(info);
    selectSetupStep("review", true);
  } catch (error) {
    message.textContent = nativeErrorText(error);
    message.hidden = false;
  } finally {
    setupProfileBusy = false;
    if (currentSetupInfo) applySetupInfo(currentSetupInfo);
  }
});

window.setInterval(() => {
  if (isTauri() && currentProvisioning?.state === "ready" && !bootstrapBusy && !provisioningBusy && !runtimeBusy && !diagnosticsBusy && !repairOperation && !backupSystemBusy()) {
    void refreshRuntime();
  }
}, 2000);

window.setInterval(() => {
  if (isTauri() && currentProvisioning?.state === "ready" && !bootstrapBusy && !repairOperation && !backupSystemBusy()) {
    void refreshRuntimeMaintenance();
  }
}, 5000);

void bootstrap();
