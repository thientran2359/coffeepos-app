import { invoke, isTauri } from "@tauri-apps/api/core";
import {
  applyDocumentTranslations,
  formatBytes,
  formatDateTime,
  formatNumber,
  getLocale,
  hasTranslation,
  isAppLanguage,
  setLocale,
  t,
  type AppLanguage,
} from "./i18n";
import "./styles.css";

interface ShellInfo {
  version: string;
  data_dir: string;
  config: {
    schema_version: number;
    store_name: string;
    bind_host: string;
    startup_view: "home" | "settings" | "diagnostics";
    app_language?: AppLanguage | null;
    setup_admin_username?: string;
    setup_admin_email?: string;
    network_mode: "local_only" | "lan";
    lan_adapter_id?: string | null;
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
  code: string;
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
  network: {
    configured_mode: "local_only" | "lan";
    effective_mode: "local_only" | "lan";
    adapter_id: string | null;
    adapter_name: string | null;
    lan_address: string | null;
    internal_origin: string | null;
    canonical_origin: string | null;
    lan_listener_state: "disabled" | "starting" | "ready" | "error";
    tls_state: "disabled" | "preparing" | "ready" | "error";
    network_profile: "private" | "domain_authenticated" | "public" | "unknown" | null;
    last_error: RuntimeErrorInfo | null;
  };
  last_error: RuntimeErrorInfo | null;
}

type RuntimeStartupStage =
  | "idle"
  | "preparing"
  | "database_starting"
  | "database_ready"
  | "php_starting"
  | "php_ready"
  | "web_server_starting"
  | "web_server_ready"
  | "application_health"
  | "ready";

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
    button.dataset.i18n = "onboarding.restore_backup";
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
    button.dataset.i18n = "onboarding.restore_backup";
    actions.append(button);
    backupLead.insertAdjacentElement("afterend", actions);
    const titleNode = document.getElementById("backup-title");
    if (titleNode) titleNode.dataset.i18n = "backup.title";
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
        <div><span class="section-label" data-i18n="restore.label"></span><h2 id="restore-title" tabindex="-1" data-i18n="restore.title"></h2></div>
        <span id="restore-state" class="state-badge" data-i18n="common.ready"></span>
      </div>

      <div data-restore-view="password">
        <p class="lead" data-i18n="restore.password_lead"></p>
        <form id="restore-password-form" class="backup-password-form" novalidate>
          <div class="field-group">
            <label for="restore-password" data-i18n="restore.password"></label>
            <input id="restore-password" type="password" maxlength="128" autocomplete="current-password" required />
          </div>
          <p class="hint" data-i18n="restore.password_hint"></p>
          <p id="restore-password-error" class="field-error" role="alert" hidden></p>
          <div class="actions split-actions">
            <button id="restore-password-cancel" class="secondary" type="button" data-i18n="common.back"></button>
            <button id="restore-inspect" type="submit" data-i18n="restore.inspect"></button>
          </div>
        </form>
      </div>

      <div data-restore-view="review" hidden>
        <p class="lead" data-i18n="restore.review_lead"></p>
        <dl class="review-list">
          <dt data-i18n="onboarding.review_store"></dt><dd id="restore-review-store">—</dd>
          <dt data-i18n="restore.created_at"></dt><dd id="restore-review-created">—</dd>
          <dt>WordPress</dt><dd id="restore-review-wordpress">—</dd>
          <dt>WooCommerce</dt><dd id="restore-review-woocommerce">—</dd>
          <dt>CoffeePOS</dt><dd id="restore-review-coffeepos">—</dd>
          <dt data-i18n="restore.uploads"></dt><dd id="restore-review-uploads">—</dd>
          <dt data-i18n="restore.compatibility"></dt><dd id="restore-review-compatibility">—</dd>
        </dl>
        <div id="restore-unmanaged-warning" class="backup-result backup-result-error" hidden>
          <strong data-i18n="restore.unmanaged_title"></strong>
          <p data-i18n="restore.unmanaged_description"></p>
        </div>
        <p id="restore-review-impact" class="hint"></p>
        <p id="restore-review-error" class="error" role="alert" hidden></p>
        <div class="actions split-actions">
          <button id="restore-review-cancel" class="secondary" type="button" data-i18n="common.cancel"></button>
          <button id="restore-apply" type="button" data-i18n="restore.apply"></button>
        </div>
      </div>

      <div data-restore-view="progress" hidden>
        <div class="backup-progress-copy">
          <strong data-i18n="restore.progress_title"></strong>
          <p id="restore-progress-stage" role="status" aria-live="polite" data-i18n="restore.progress_prepare"></p>
          <progress id="restore-progress" max="1" data-i18n="common.processing"></progress>
          <p id="restore-progress-detail" class="hint" data-i18n="restore.progress_detail"></p>
        </div>
        <p id="restore-close-guidance" class="hint" data-i18n="restore.close_guidance"></p>
        <div class="actions"><button id="restore-cancel" class="secondary" type="button" data-i18n="restore.cancel"></button></div>
        <p id="restore-progress-status" class="hint" role="status" aria-live="polite"></p>
      </div>

      <div data-restore-view="success" hidden>
        <div class="backup-result">
          <strong data-i18n="restore.success_title"></strong>
          <p data-i18n="restore.success_description"></p>
        </div>
        <div class="actions"><button id="restore-open-home" type="button" data-i18n="restore.open_overview"></button></div>
      </div>

      <div data-restore-view="failure" hidden>
        <div class="backup-result backup-result-error" role="alert">
          <strong id="restore-failure-title" data-i18n="restore.failure_title"></strong>
          <p id="restore-failure-message" data-i18n="restore.failure_message"></p>
          <p id="restore-failure-recovery" class="hint"></p>
        </div>
        <div class="actions split-actions">
          <button id="restore-failure-back" class="secondary" type="button" data-i18n="common.back"></button>
          <button id="restore-retry" type="button" data-i18n="common.retry"></button>
        </div>
      </div>

      <div data-restore-view="recovery" hidden>
        <div class="backup-result backup-result-error" role="alert">
          <strong data-i18n="restore.recovery_title"></strong>
          <p data-i18n="restore.recovery_description"></p>
        </div>
        <details class="technical-details">
          <summary data-i18n="common.technical_details"></summary>
          <p id="restore-recovery-details" class="hint"></p>
        </details>
        <div class="actions"><button id="restore-recovery-refresh" type="button" data-i18n="restore.refresh_recovery"></button></div>
      </div>
    </div>`;
  main.append(screen);
  applyDocumentTranslations(screen);
}

installRestoreUi();

const bootstrapPanel = element("bootstrap");
const title = element("status-title");
const description = element("status-description");
const retry = element<HTMLButtonElement>("retry");
const languageChooser = element<HTMLElement>("language-chooser");
const languageTitle = element<HTMLElement>("language-title");
const languageStatus = element("language-status");
const languageButtons = Array.from(document.querySelectorAll<HTMLButtonElement>("[data-language]"));
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
const settingsAppLanguage = element<HTMLSelectElement>("app-language");
const languageSettingsStatus = element("language-settings-status");
const settingsStartupView = element<HTMLSelectElement>("startup-view");
const settingsSave = element<HTMLButtonElement>("settings-save");
const settingsSaveStatus = element("settings-save-status");
const settingsNetwork = element("settings-network");
const settingsNetworkState = element("settings-network-state");
const settingsNetworkDetail = element("settings-network-detail");
const settingsNetworkToggle = element<HTMLButtonElement>("settings-network-toggle");
const settingsNetworkConfirmation = element("settings-network-confirmation");
const settingsNetworkConfirm = element<HTMLButtonElement>("settings-network-confirm");
const settingsNetworkCancel = element<HTMLButtonElement>("settings-network-cancel");
const settingsNetworkStatus = element("settings-network-status");

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
let provisioningProgressRefreshBusy = false;
let runtimeBusy = false;
let runtimeStartupProgress: RuntimeStartupStage = "idle";
let runtimeStartupProgressRefreshBusy = false;
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
let networkBusy = false;
let networkConfirmOpen = false;
let networkFeedback = "";
let networkFeedbackError = false;
let languageBusy = false;
let persistedLanguage: AppLanguage | null = null;
let currentShellInfo: ShellInfo | null = null;
let languageResolutionPending = true;
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
let currentRepairResult: RepairApplyResult | null = null;
let currentRepairResultPlan: RepairPlan | null = null;
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

function localizedErrorCode(code: unknown, fallbackKey = "errors.generic"): string {
  if (typeof code === "string" && code.trim()) {
    const key = `errors.${code.trim()}`;
    if (hasTranslation(key)) return t(key);
  }
  return t(fallbackKey);
}

function localizedNativeError(error: unknown, fallbackKey = "errors.generic"): string {
  const value = recordValue(error);
  if (value) {
    if (typeof value.code === "string") return localizedErrorCode(value.code, fallbackKey);
  }
  return t(fallbackKey);
}

function rawNativeError(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  const value = recordValue(error);
  if (value) {
    const message = typeof value.message === "string" ? value.message : "";
    const recovery = typeof value.recovery === "string" ? value.recovery : "";
    return [message, recovery].filter(Boolean).join("\n");
  }
  return "";
}

function applyLocaleStaticText(reveal = true): void {
  applyDocumentTranslations();
  settingsAppLanguage.value = getLocale();
  if (reveal) document.documentElement.dataset.i18nReady = "true";
  else delete document.documentElement.dataset.i18nReady;
}

function setupProfileCommitted(): boolean {
  return Boolean(
    currentShellInfo?.config.setup_admin_username
    || currentShellInfo?.config.setup_admin_email,
  );
}

function freshProfileNeedsLanguageChoice(): boolean {
  return persistedLanguage === null
    && currentProvisioning?.state === "not_installed"
    && !setupProfileCommitted()
    && !restoreSystemBusy();
}

function showLanguageChooser(): void {
  bootstrapPanel.hidden = true;
  setup.hidden = true;
  installedShell.hidden = true;
  restoreScreen.hidden = true;
  languageChooser.hidden = false;
  languageResolutionPending = true;
  setTextIfChanged(languageStatus, "");
  applyLocaleStaticText();
  languageTitle.focus();
}

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
  const code = value && typeof value.code === "string" ? value.code : null;
  return {
    message: localizedErrorCode(code, "errors.restore_failed"),
    recovery: t("errors.recovery.retry"),
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
    const label = backup.can_restore ? t("restore.compatible") : t("restore.incompatible");
    return { compatible: backup.can_restore, label };
  }
  if (typeof inspection.compatible === "boolean") {
    return {
      compatible: inspection.compatible,
      label: inspection.compatible ? t("restore.compatible") : t("restore.incompatible"),
    };
  }
  if (typeof inspection.compatibility === "boolean") {
    return {
      compatible: inspection.compatibility,
      label: inspection.compatibility ? t("restore.compatible") : t("restore.incompatible"),
    };
  }
  if (typeof inspection.compatibility === "string") {
    const value = inspection.compatibility.trim();
    const compatible = !/blocked|incompatible|unsupported|reject/i.test(value);
    return { compatible, label: compatible ? t("restore.compatible_short") : t("restore.incompatible_short") };
  }
  const compatibility = recordValue(inspection.compatibility);
  if (compatibility) {
    const compatibleValue = compatibility.compatible;
    const status = firstString(compatibility.status);
    const compatible = typeof compatibleValue === "boolean"
      ? compatibleValue
      : !/blocked|incompatible|unsupported|reject/i.test(status);
    const label = compatible ? t("restore.compatible") : t("restore.incompatible");
    return { compatible, label };
  }
  return {
    compatible: false,
    label: t("restore.compatibility_unknown"),
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
  return formatDateTime(date);
}

function formatBackupBytes(value: number): string {
  return formatBytes(value);
}

function backupStageLabel(stage: string): string {
  const key = `backup.stage.${stage}`;
  return hasTranslation(key) ? t(key) : t("backup.stage.default");
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
  backupCreate.disabled = !eligible || backupSystemBusy() || networkBusy;
  backupPassword.disabled = mutating || active || restoring || networkBusy;
  backupPasswordConfirm.disabled = mutating || active || restoring || networkBusy;
  backupPasswordCancel.disabled = mutating || active || restoring || networkBusy;
  backupChooseDestination.disabled = !eligible || mutating || active || restoring || networkBusy;
  backupCancel.disabled = !backupCancellationAvailable() || backupOperation === "cancel" || restoring || networkBusy;
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
  settingsSave.disabled = settingsBusy || backupSystemBusy() || networkBusy;
  renderNetworkSettings();
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
      t("backup.progress_files", { processed: formatNumber(processedFiles), estimated: formatNumber(estimatedFiles) }),
    );
  } else if (estimatedBytes > 0) {
    backupProgress.max = estimatedBytes;
    backupProgress.value = Math.min(processedBytes, estimatedBytes);
    setTextIfChanged(
      backupProgressCount,
      t("backup.progress_bytes", { processed: formatBackupBytes(processedBytes), estimated: formatBackupBytes(estimatedBytes) }),
    );
  } else {
    backupProgress.max = 1;
    backupProgress.removeAttribute("value");
    setTextIfChanged(backupProgressCount, t("backup.progress_wait"));
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
    setTextIfChanged(backupState, t("common.completed"));
    setTextIfChanged(backupSuccessStatus, "");
    selectBackupView("success");
  } else if (status.failed) {
    setTextIfChanged(backupState, t("common.error"));
    setTextIfChanged(backupFailureMessage, localizedErrorCode(status.last_error?.code, "errors.backup_failed"));
    setTextIfChanged(backupFailureRecovery, status.last_error ? t("errors.recovery.retry") : "");
    selectBackupView("failure");
  } else if (status.cancelled) {
    setTextIfChanged(backupState, t("common.cancelled"));
    setTextIfChanged(backupLandingStatus, t("backup.cancelled_message"));
    selectBackupView("landing");
  } else if (backupIsActive(status)) {
    setTextIfChanged(backupState, t("backup.state.running"));
    setTextIfChanged(backupProgressTitle, t("backup.progress_title"));
    setTextIfChanged(backupProgressStatus, backupOperation === "cancel" ? t("backup.cancel_requested") : "");
    renderBackupProgress(status);
    selectBackupView("progress");
  } else if (currentBackupView !== "password" || backupOperation === null) {
    setTextIfChanged(backupState, t("common.ready"));
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
  setTextIfChanged(backupState, t("common.error"));
  setTextIfChanged(backupFailureMessage, localizedErrorCode(structured?.code, "errors.backup_failed"));
  setTextIfChanged(backupFailureRecovery, t("errors.recovery.retry"));
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
      setTextIfChanged(backupProgressStatus, t("backup.status_update_failed"));
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
    setTextIfChanged(backupPasswordError, t("backup.validation.password_required"));
    backupPasswordError.hidden = false;
    backupPassword.focus();
    return null;
  }
  if (password !== backupPasswordConfirm.value) {
    backupPasswordConfirm.setAttribute("aria-invalid", "true");
    setTextIfChanged(backupPasswordError, t("backup.validation.password_mismatch"));
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
  setTextIfChanged(backupState, t("backup.state.preparing"));
  setTextIfChanged(backupProgressStage, t("backup.select_destination"));
  setTextIfChanged(backupProgressCount, t("backup.snapshot_after_destination"));
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
      setTextIfChanged(backupState, t("common.ready"));
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
  setTextIfChanged(backupProgressStatus, t("backup.cancelling"));
  syncOperationControls();
  try {
    await invoke<unknown>("cancel_backup", { operationId });
    ensureBackupPolling();
  } catch (error) {
    setTextIfChanged(backupProgressStatus, localizedErrorCode(backupErrorInfo(error)?.code, "errors.backup_failed"));
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
  setTextIfChanged(backupSuccessStatus, t("backup.opening_folder"));
  setBackupControls();
  try {
    await invoke<void>("open_backup_folder", { operationId });
    setTextIfChanged(backupSuccessStatus, t("backup.opened_folder"));
  } catch (error) {
    setTextIfChanged(backupSuccessStatus, localizedErrorCode(backupErrorInfo(error)?.code, "errors.backup_failed"));
  } finally {
    backupOperation = null;
    setBackupControls();
  }
}

function showBackupPasswordStep(): void {
  if (backupSystemBusy() || currentProvisioning?.state !== "ready" || repairRouteRequired) return;
  clearBackupPasswordFields();
  setTextIfChanged(backupLandingStatus, "");
  setTextIfChanged(backupState, t("common.ready"));
  selectBackupView("password", true);
  setBackupControls();
}

function restoreStageLabel(stage: string): string {
  const key = `restore.stage.${stage}`;
  return hasTranslation(key) ? t(key) : t("restore.stage.default");
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
  setupRestore.disabled = !freshEligible || otherMaintenance || restoreSystemBusy() || networkBusy;
  restoreInstalledStart.disabled = !installedEligible || otherMaintenance || restoreSystemBusy() || networkBusy;
  restorePassword.disabled = active || mutating || inspecting || networkBusy;
  restoreInspect.disabled = active || mutating || inspecting || otherMaintenance || networkBusy;
  restorePasswordCancel.disabled = active || mutating || inspecting || networkBusy;
  restoreReviewCancel.disabled = active || mutating || networkBusy;
  restoreApply.disabled = active
    || mutating
    || !restoreCandidateId()
    || compatibility?.compatible === false
    || otherMaintenance
    || networkBusy;
  restoreCancel.disabled = !restoreCancellationAvailable() || restoreOperation === "cancel" || networkBusy;
  restoreFailureBack.disabled = active || mutating || networkBusy;
  restoreRetry.disabled = active || mutating || otherMaintenance || networkBusy;
  restoreRecoveryRefresh.disabled = restoreStatusRefreshBusy || networkBusy;
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
    compatibility.compatible ? "" : t("restore.review_incompatible"),
  );
  setTextIfChanged(
    restoreReviewImpact,
    restoreEntrySource === "fresh"
      ? t("restore.impact.fresh")
      : t("restore.impact.installed"),
  );
  setTextIfChanged(restoreState, compatibility.compatible ? t("restore.state.inspected") : t("restore.state.incompatible"));
  showRestoreScreen("review", true);
  setRestoreControls();
}

function renderRestoreProgress(status: RestoreStatus): void {
  setTextIfChanged(restoreState, t("restore.state.restoring"));
  setTextIfChanged(restoreProgressStage, restoreStageLabel(status.stage));
  const afterCutover = restoreCutoverStarted(status);
  const canCancel = restoreCancellationAvailable(status);
  setTextIfChanged(
    restoreProgressDetail,
    afterCutover
      ? t("restore.after_cutover_detail")
      : t("restore.before_cutover_detail"),
  );
  setTextIfChanged(
    restoreCloseGuidance,
    afterCutover
      ? t("restore.after_cutover_close")
      : t("restore.before_cutover_close"),
  );
  setTextIfChanged(restoreCancel, afterCutover ? t("restore.rollback") : t("restore.cancel"));
  restoreCancel.hidden = !canCancel && !restoreOperation;
  showRestoreScreen("progress");
}

function restoreStatusError(status: RestoreStatus): { message: string; recovery: string } {
  const error = status.last_error;
  if (!error) {
    return {
      message: t("restore.failure_message"),
      recovery: t("restore.generic_recovery"),
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
    setTextIfChanged(restoreState, t("restore.state.action_required"));
    setTextIfChanged(
      restoreRecoveryDetails,
      [error.message, error.recovery].filter(Boolean).join(" ") || t("restore.recovery_gate"),
    );
    showRestoreScreen("recovery");
  } else if (terminal === "success") {
    setTextIfChanged(restoreState, t("restore.state.done"));
    setTextIfChanged(restoreProgressStatus, "");
    clearRestorePassword();
    showRestoreScreen("success");
  } else if (terminal === "rolled_back") {
    const error = restoreStatusError(status);
    setTextIfChanged(restoreState, t("restore.state.rolled_back"));
    setTextIfChanged(restoreFailureTitle, t("restore.failure_title"));
    setTextIfChanged(
      restoreFailureMessage,
      restoreEntrySource === "fresh"
        ? t("restore.rollback_fresh")
        : t("restore.rollback_installed"),
    );
    setTextIfChanged(restoreFailureRecovery, [error.message, error.recovery].filter(Boolean).join(" "));
    clearRestorePassword();
    showRestoreScreen("failure");
  } else if (terminal === "cancelled") {
    setTextIfChanged(restoreState, t("common.cancelled"));
    setTextIfChanged(restoreFailureTitle, t("restore.cancelled_title"));
    setTextIfChanged(
      restoreFailureMessage,
      restoreEntrySource === "fresh"
        ? t("restore.cancelled_fresh")
        : t("restore.cancelled_installed"),
    );
    setTextIfChanged(restoreFailureRecovery, "");
    clearRestorePassword();
    showRestoreScreen("failure");
  } else if (terminal === "failed") {
    const error = restoreStatusError(status);
    setTextIfChanged(restoreState, t("common.error"));
    setTextIfChanged(restoreFailureTitle, t("restore.failure_title"));
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
  setTextIfChanged(restoreState, t("common.error"));
  setTextIfChanged(restoreFailureTitle, t("restore.failure_title"));
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
      setTextIfChanged(restoreProgressStatus, t("restore.progress_update_failed"));
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
  setTextIfChanged(restoreState, t("common.ready"));
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
    setTextIfChanged(restorePasswordError, t("restore.validation.password_required"));
    restorePasswordError.hidden = false;
    restorePassword.focus();
    return;
  }

  restoreOperation = "inspect";
  currentRestoreInspection = null;
  setTextIfChanged(restoreState, t("restore.state.inspecting"));
  setTextIfChanged(restorePasswordError, "");
  setRestoreControls();
  setBackupControls();
  try {
    const inspection = await invoke<RestoreInspection | null>("inspect_restore_backup", { backupPassword: password });
    if (!inspection || !restoreCandidateId(inspection)) {
      clearRestorePassword();
      setTextIfChanged(restoreState, t("common.ready"));
      return;
    }
    renderRestoreInspection(inspection);
  } catch (error) {
    const safe = restoreErrorInfo(error);
    clearRestorePassword();
    setTextIfChanged(restoreState, t("restore.state.inspect_failed"));
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
    setTextIfChanged(restorePasswordError, t("restore.validation.password_again"));
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
      ? t("restore.cancel_requested.after_cutover")
      : t("restore.cancel_requested.before_cutover"),
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
  return localizedErrorCode(error.code, error.component === "provisioning" ? "errors.provisioning" : "errors.runtime");
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
  if (state === "healthy") return t("common.healthy");
  if (state === "unhealthy") return t("common.error");
  if (state === "unknown") return t("common.not_verified");
  return t("common.unavailable");
}

function defaultHealthSummary(component: HealthComponent, state: ComponentHealthState, runtimeState: RuntimeInfo["state"]): string {
  const displayName = healthComponentName(component);
  if (state === "healthy") {
    if (component === "database") return t("diagnostics.healthy.database");
    if (component === "php") return t("diagnostics.healthy.php");
    if (component === "wordpress") return t("diagnostics.healthy.wordpress");
    return t("diagnostics.healthy.generic", { component: displayName });
  }
  if (state === "unavailable") {
    return runtimeState === "running"
      ? t("diagnostics.unavailable.running", { component: displayName })
      : t("diagnostics.unavailable.stopped", { component: displayName });
  }
  if (state === "unknown") return t("diagnostics.unknown", { component: displayName });
  return t("diagnostics.unhealthy", { component: displayName });
}

function defaultHealthRecovery(component: HealthComponent, state: ComponentHealthState, runtimeState: RuntimeInfo["state"]): string {
  if (state === "healthy") return "";
  if (state === "unavailable" && runtimeState !== "running") return t("diagnostics.recovery.start");
  if (state === "unknown") {
    if (component === "woocommerce") return t("diagnostics.recovery.wordpress");
    return t("diagnostics.recovery.dependency");
  }
  return t("diagnostics.recovery.retry");
}

function renderHealthComponent(component: HealthComponent, info: ComponentHealthInfo, runtimeState: RuntimeInfo["state"]): void {
  const stateNode = element(`health-${component}-state`);
  const summaryNode = element(`health-${component}-summary`);
  const recoveryNode = element(`health-${component}-recovery`);
  setTextIfChanged(stateNode, healthStateLabel(info.state));
  stateNode.dataset.healthState = info.state;
  setTextIfChanged(summaryNode, info.error ? structuredErrorText(info.error) : defaultHealthSummary(component, info.state, runtimeState));
  const recovery = defaultHealthRecovery(component, info.state, runtimeState);
  setTextIfChanged(recoveryNode, recovery ? t("diagnostics.recovery_prefix", { recovery }) : "");
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
    setTextIfChanged(healthSummaryState, t("diagnostics.state.action_required"));
    setTextIfChanged(healthSummary, t("diagnostics.summary.unhealthy"));
  } else if (states.includes("unknown")) {
    setTextIfChanged(healthSummaryState, t("common.not_verified"));
    setTextIfChanged(healthSummary, t("diagnostics.summary.unknown"));
  } else if (states.every((state) => state === "healthy")) {
    setTextIfChanged(healthSummaryState, t("common.ready"));
    setTextIfChanged(healthSummary, t("diagnostics.summary.healthy"));
  } else if (info.runtime_state !== "running") {
    setTextIfChanged(healthSummaryState, t("overview.state.stopping"));
    setTextIfChanged(healthSummary, t("diagnostics.summary.stopped"));
  } else {
    setTextIfChanged(healthSummaryState, t("common.not_verified"));
    setTextIfChanged(healthSummary, t("diagnostics.summary.incomplete"));
  }
  setHealthControls();
}

function renderHealthChecking(
  summaryState = t("diagnostics.state.checking"),
  summaryText = t("diagnostics.summary.checking"),
): void {
  currentDiagnostics = null;
  setTextIfChanged(healthSummaryState, summaryState);
  setTextIfChanged(healthSummary, summaryText);
  for (const component of healthComponents) {
    const stateNode = element(`health-${component}-state`);
    stateNode.dataset.healthState = "checking";
    setTextIfChanged(stateNode, t("diagnostics.state.checking"));
    setTextIfChanged(element(`health-${component}-summary`), t("diagnostics.component.waiting"));
    setTextIfChanged(element(`health-${component}-recovery`), "");
  }
  setHealthControls();
}

function renderHealthCommandError(error: unknown): void {
  currentDiagnostics = null;
  setTextIfChanged(healthSummaryState, t("diagnostics.state.check_failed"));
  setTextIfChanged(healthSummary, localizedNativeError(error, "errors.health"));
  for (const component of healthComponents) {
    const stateNode = element(`health-${component}-state`);
    stateNode.dataset.healthState = "unknown";
    setTextIfChanged(stateNode, t("common.not_verified"));
    setTextIfChanged(element(`health-${component}-summary`), t("diagnostics.component.incomplete"));
    setTextIfChanged(element(`health-${component}-recovery`), "");
  }
  setHealthControls();
}

async function refreshHealthDiagnostics(): Promise<void> {
  if (!isTauri() || diagnosticsBusy || repairOperation || bootstrapBusy || runtimeBusy || provisioningBusy || backupSystemBusy() || currentProvisioning?.state !== "ready") return;
  diagnosticsBusy = true;
  setRuntimeControls(currentRuntime);
  renderHealthChecking();
  setTextIfChanged(healthCheckStatus, t("diagnostics.running"));
  try {
    const info = await invoke<HealthDiagnosticsInfo>("get_health_diagnostics");
    renderHealthDiagnostics(info);
    setTextIfChanged(healthCheckStatus, t("diagnostics.checked"));
    await refreshRuntime();
  } catch (error) {
    renderHealthCommandError(error);
    setTextIfChanged(healthCheckStatus, localizedNativeError(error, "errors.health"));
  } finally {
    diagnosticsBusy = false;
    setHealthControls();
    setRuntimeControls(currentRuntime);
  }
}

function repairClassificationLabel(classification: RepairClassification): string {
  if (classification === "repairable") return t("repair.classification.repairable");
  if (classification === "requires_input") return t("repair.classification.requires_input");
  return t("repair.classification.blocked");
}

function setRepairControls(): void {
  const busy = repairOperation !== null || bootstrapBusy || provisioningBusy || runtimeBusy || networkBusy || diagnosticsBusy || backupSystemBusy();
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
  currentRepairResult = null;
  currentRepairResultPlan = null;
  repairList.replaceChildren();
  repairAdminInput.hidden = true;
  clearRepairPasswordFields();
  repairError.textContent = "";
  repairError.hidden = true;
  repairOpenDiagnostics.hidden = true;
  setTextIfChanged(repairSummaryState, t("common.not_checked"));
  setTextIfChanged(
    repairSummary,
    t("repair.initial"),
  );
  setTextIfChanged(repairInspect, t("repair.inspect"));
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
    const targetKey = `repair.item.target.${item.id}`;
    target.textContent = hasTranslation(targetKey) ? t(targetKey) : t("repair.item.target.unknown", { component: item.component });
    const badge = document.createElement("span");
    badge.className = "state-badge";
    badge.dataset.repairClassification = item.classification;
    badge.textContent = repairClassificationLabel(item.classification);
    heading.append(target, badge);
    const action = document.createElement("p");
    action.className = "repair-action";
    action.textContent = t(`repair.item.action.${item.classification}`);
    const reason = document.createElement("p");
    reason.className = "repair-summary-text";
    reason.textContent = t(`repair.item.reason.${item.classification}`);
    const impact = document.createElement("p");
    impact.className = "hint";
    impact.textContent = t(item.requires_runtime_stop ? "repair.item.impact.runtime_stop" : "repair.item.impact.no_runtime_stop");
    article.append(heading, action, reason, impact);
    repairList.append(article);
  }
}

function renderRepairPlan(plan: RepairPlan): void {
  currentRepairPlan = plan;
  currentRepairResult = null;
  currentRepairResultPlan = null;
  setTextIfChanged(repairInspect, t("repair.inspect_again"));
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
    setTextIfChanged(repairSummaryState, t("repair.none"));
    setTextIfChanged(repairSummary, t("repair.none_summary"));
  } else if (repairable + needsInput > 0) {
    setTextIfChanged(repairSummaryState, t("repair.available"));
    const blockedSuffix = blocked > 0 ? t("repair.blocked_suffix", { count: formatNumber(blocked) }) : "";
    setTextIfChanged(repairSummary, t("repair.available_summary", {
      repairable: formatNumber(repairable + needsInput),
      blocked: blockedSuffix,
    }));
  } else {
    setTextIfChanged(repairSummaryState, t("repair.classification.blocked"));
    setTextIfChanged(repairSummary, t("repair.blocked_summary", { count: formatNumber(blocked) }));
  }
  setRepairControls();
}

function renderRepairCommandError(error: unknown): void {
  currentRepairPlan = null;
  setTextIfChanged(repairInspect, t("repair.inspect_again"));
  setTextIfChanged(repairSummaryState, t("repair.inspect_failed"));
  setTextIfChanged(repairSummary, t("repair.inspect_failed_summary"));
  repairList.replaceChildren();
  repairAdminInput.hidden = true;
  clearRepairPasswordFields();
  repairError.textContent = localizedNativeError(error, "errors.repair");
  repairError.hidden = false;
  setRepairControls();
}

async function refreshRepairPlan(): Promise<void> {
  if (!isTauri() || repairOperation || bootstrapBusy || provisioningBusy || runtimeBusy || diagnosticsBusy || backupSystemBusy()) return;
  if (currentProvisioning?.state !== "ready" && currentProvisioning?.state !== "needs_repair") return;
  repairOperation = "inspect";
  currentRepairPlan = null;
  setTextIfChanged(repairSummaryState, t("repair.inspecting"));
  setTextIfChanged(repairSummary, t("repair.inspecting_summary"));
  setTextIfChanged(repairStatus, t("repair.inspecting_status"));
  repairError.hidden = true;
  repairOpenDiagnostics.hidden = true;
  setRepairControls();
  setRuntimeControls(currentRuntime);
  setHealthControls();
  try {
    renderRepairPlan(await invoke<RepairPlan>("get_repair_plan"));
    setTextIfChanged(repairStatus, t("repair.plan_ready"));
  } catch (error) {
    renderRepairCommandError(error);
    setTextIfChanged(repairStatus, localizedNativeError(error, "errors.repair"));
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
    repairAdminError.textContent = t("repair.password_invalid");
    repairAdminError.hidden = false;
    repairAdminPassword.focus();
    return null;
  }
  if (password !== confirmation) {
    repairAdminPasswordConfirm.setAttribute("aria-invalid", "true");
    repairAdminError.textContent = t("repair.password_mismatch");
    repairAdminError.hidden = false;
    repairAdminPasswordConfirm.focus();
    return null;
  }
  return password;
}

function renderRepairResult(result: RepairApplyResult, previousPlan: RepairPlan): void {
  currentRepairPlan = null;
  currentRepairResult = result;
  currentRepairResultPlan = previousPlan;
  setTextIfChanged(repairInspect, t("repair.inspect_again"));
  repairList.replaceChildren();
  repairAdminInput.hidden = true;
  clearRepairPasswordFields();
  for (const item of result.items) {
    const article = document.createElement("article");
    article.className = "repair-row";
    const heading = document.createElement("div");
    heading.className = "repair-row-heading";
    const target = document.createElement("strong");
    const targetKey = `repair.item.target.${item.id}`;
    target.textContent = hasTranslation(targetKey) ? t(targetKey) : t("repair.item.target.unknown", { component: item.id });
    const badge = document.createElement("span");
    badge.className = "state-badge";
    badge.textContent = item.status === "repaired"
      ? t("repair.result.repaired")
      : item.status === "blocked"
        ? t("repair.result.blocked")
        : t("repair.result.skipped");
    heading.append(target, badge);
    const messageNode = document.createElement("p");
    messageNode.className = "repair-summary-text";
    messageNode.textContent = item.status === "repaired"
      ? t("repair.item.reason.repairable")
      : item.status === "blocked"
        ? t("repair.item.reason.blocked")
        : t("repair.item.reason.requires_input");
    article.append(heading, messageNode);
    repairList.append(article);
  }
  repairError.hidden = !result.last_error;
  repairError.textContent = result.last_error ? structuredErrorText(result.last_error) : "";
  repairOpenDiagnostics.hidden = result.status === "stale";
  if (result.status === "repaired") {
    setTextIfChanged(repairSummaryState, t("repair.done"));
    setTextIfChanged(repairSummary, t("repair.done_summary"));
    setTextIfChanged(repairStatus, t("repair.done_status"));
  } else if (result.status === "stale") {
    setTextIfChanged(repairSummaryState, t("repair.stale"));
    setTextIfChanged(repairSummary, t("repair.stale_summary"));
    setTextIfChanged(repairStatus, t("repair.stale_status"));
  } else {
    setTextIfChanged(repairSummaryState, t("repair.partial"));
    setTextIfChanged(repairSummary, t("repair.partial_summary"));
    setTextIfChanged(repairStatus, t("repair.partial_status"));
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
  setTextIfChanged(repairSummaryState, t("repair.applying"));
  setTextIfChanged(repairSummary, t("repair.applying_summary"));
  setTextIfChanged(repairStatus, t("repair.applying_status"));
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
    setTextIfChanged(repairInspect, t("repair.inspect_again"));
    repairError.textContent = localizedNativeError(error, "errors.repair");
    repairError.hidden = false;
    setTextIfChanged(repairSummaryState, t("repair.command_failed"));
    setTextIfChanged(repairSummary, t("repair.command_failed_summary"));
    setTextIfChanged(repairStatus, localizedNativeError(error, "errors.repair"));
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
  return formatBytes(bytes);
}

function formatLogTimestamp(value: number | null): string {
  if (!value || !Number.isFinite(value)) return t("logs.no_data");
  const milliseconds = value < 1_000_000_000_000 ? value * 1000 : value;
  return formatDateTime(milliseconds);
}

function logErrorMessage(error: unknown, fallbackKey = "errors.logs"): string {
  const value = recordValue(error);
  return localizedErrorCode(value?.code, fallbackKey);
}

function logLabel(entry: Pick<LogCatalogEntry, "id" | "label">): string {
  const key = `logs.id.${entry.id}`;
  return hasTranslation(key) ? t(key) : t("logs.source_unknown", { id: entry.id });
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
    setTextIfChanged(logCurrentLabel, t("logs.current"));
    setTextIfChanged(logCurrentMeta, t("logs.no_metadata"));
    return;
  }
  setTextIfChanged(logCurrentLabel, logLabel(entry));
  const metadata = entry.exists
    ? t("logs.meta", { size: formatLogBytes(entry.size_bytes), time: formatLogTimestamp(entry.modified_at) })
    : t("logs.meta_missing");
  const flags: string[] = [];
  if (currentLogTruncated) flags.push(t("logs.meta_truncated"));
  if (currentLogRedactionCount > 0) flags.push(t("logs.meta_redacted", { count: formatNumber(currentLogRedactionCount) }));
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
    setTextIfChanged(logsReadState, t("logs.not_loaded"));
  } else if (!entry.exists) {
    setTextIfChanged(logsReadState, t("logs.no_data"));
  } else if (hasLines) {
    setTextIfChanged(logsReadState, currentLogTruncated ? t("logs.limited") : t("logs.loaded"));
  } else {
    setTextIfChanged(logsReadState, t("logs.no_lines"));
    logEmpty.hidden = false;
  }
  setLogControls();
}

function showLogReadError(titleText: string, error: unknown, preserveContent: boolean): void {
  setTextIfChanged(logsReadState, t("logs.read_failed"));
  setTextIfChanged(logReadErrorTitle, titleText);
  setTextIfChanged(logReadErrorMessage, logErrorMessage(error, "errors.logs"));
  const technical = rawNativeError(error);
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
    const label = logLabel(entry);
    option.textContent = entry.exists ? label : t("logs.source_missing", { label });
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
    option.textContent = t("logs.source_none");
    logSource.append(option);
    setTextIfChanged(logSourceStatus, t("logs.source_none"));
  } else {
    setTextIfChanged(logSourceStatus, t("logs.source_count", { available: formatNumber(available), total: formatNumber(catalog.logs.length) }));
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
  setTextIfChanged(logsReadState, preserveExisting && currentLogLines.length > 0 ? t("logs.refreshing") : t("logs.reading"));
  setTextIfChanged(logRefreshStatus, preserveExisting && currentLogLines.length > 0 ? t("logs.refreshing_status") : t("logs.reading_status"));
  if (currentLogLines.length === 0) logEmpty.hidden = true;
  setLogControls();
  const requestedLogId = entry.id;
  try {
    const page = await readLogPage(requestedLogId, null, "tail");
    if (page.log_id !== requestedLogId) throw new Error("log_page_source_mismatch");
    const hitViewerLimit = page.lines.length > LOG_VIEW_MAX_LINES;
    currentLogLines = hitViewerLimit ? page.lines.slice(-LOG_VIEW_MAX_LINES) : page.lines;
    currentLogOlderCursor = page.older_cursor;
    currentLogHasOlder = !hitViewerLimit && page.has_older;
    currentLogTruncated = page.truncated || hitViewerLimit;
    currentLogRedactionCount = page.redaction_count;
    setTextIfChanged(logRefreshStatus, t("logs.updated"));
    renderLogBody();
  } catch (error) {
    showLogReadError(t("logs.read_failed_source", { label: logLabel(entry) }), error, preserveExisting && currentLogLines.length > 0);
    setTextIfChanged(logRefreshStatus, t("logs.refresh_failed"));
  } finally {
    logOperation = null;
    setLogControls();
  }
}

async function refreshLogCatalogAndTail(preserveExisting = true): Promise<void> {
  if (!isTauri() || logOperation || logExportBusy) return;
  logOperation = "catalog";
  clearLogReadError();
  setTextIfChanged(logsReadState, currentLogLines.length > 0 ? t("logs.refreshing") : t("logs.loading"));
  setTextIfChanged(logRefreshStatus, currentLogLines.length > 0 ? t("logs.refreshing_status") : t("logs.loading_catalog"));
  setLogControls();
  let loadTail = false;
  try {
    const catalog = await getLogCatalog();
    renderLogCatalog(catalog);
    loadTail = !!currentLogId;
  } catch (error) {
    showLogReadError(t("logs.catalog_read_failed"), error, preserveExisting && currentLogLines.length > 0);
    setTextIfChanged(logSourceStatus, t("logs.catalog_failed"));
    setTextIfChanged(logRefreshStatus, t("logs.refresh_failed"));
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
  setTextIfChanged(logsReadState, t("logs.loading_more"));
  setTextIfChanged(logRefreshStatus, t("logs.loading_older"));
  setLogControls();
  const requestedLogId = entry.id;
  try {
    const page = await readLogPage(requestedLogId, cursor, "older");
    if (page.log_id !== requestedLogId) throw new Error("log_page_source_mismatch");
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
        ? t("logs.viewer_limit")
        : page.lines.length > 0
          ? t("logs.loaded_older")
          : t("logs.no_older"),
    );
    renderLogBody();
  } catch (error) {
    showLogReadError(t("logs.older_failed_source", { label: logLabel(entry) }), error, true);
    setTextIfChanged(logRefreshStatus, t("logs.load_more_failed"));
  } finally {
    logOperation = null;
    setLogControls();
  }
}

async function exportLogsSupportBundle(): Promise<void> {
  if (!isTauri() || logExportBusy || logOperation) return;
  logExportBusy = true;
  setTextIfChanged(logExportStatus, t("logs.exporting"));
  setLogControls();
  try {
    const result = await exportSupportBundle();
    if (result.status === "cancelled") {
      setTextIfChanged(logExportStatus, "");
      return;
    }
    setTextIfChanged(logExportStatus, t("logs.exported"));
  } catch (error) {
    setTextIfChanged(logExportStatus, logErrorMessage(error, "logs.export_failed"));
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
  currentShellInfo = info;
  element("data-dir").textContent = info.data_dir;
  element("version").textContent = info.version;
  preferredStartupView = info.config.startup_view;
  settingsStartupView.value = preferredStartupView;
  const configuredLanguage = isAppLanguage(info.config.app_language) ? info.config.app_language : null;
  persistedLanguage = configuredLanguage;
  setLocale(configuredLanguage ?? "vi");
  if (configuredLanguage) languageResolutionPending = false;
  applyLocaleStaticText(configuredLanguage !== null || !languageResolutionPending);
}

function rerenderLocalizedState(): void {
  const focused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  applyLocaleStaticText();
  if (currentSetupInfo) {
    passwordHint.textContent = currentSetupInfo.password_configured
      ? t("onboarding.password_hint_existing")
      : t("onboarding.password_hint_new");
  }
  if (currentProvisioning) renderProvisioning(currentProvisioning);
  if (currentRuntime) renderRuntime(currentRuntime);
  if (currentDiagnostics) renderHealthDiagnostics(currentDiagnostics);
  if (currentRepairPlan) renderRepairPlan(currentRepairPlan);
  else if (currentRepairResult && currentRepairResultPlan) renderRepairResult(currentRepairResult, currentRepairResultPlan);
  if (currentLogCatalog) {
    renderLogCatalog(currentLogCatalog);
    renderLogBody();
  }
  if (currentBackupStatus) renderBackupStatus(currentBackupStatus);
  if (currentRestoreInspection && currentRestoreView === "review") renderRestoreInspection(currentRestoreInspection);
  if (currentRestoreStatus) renderRestoreStatus(currentRestoreStatus);
  renderHome();
  if (focused?.isConnected) focused.focus({ preventScroll: true });
}

async function saveLanguage(nextLanguage: AppLanguage, source: "chooser" | "settings"): Promise<void> {
  if (!isTauri() || languageBusy || nextLanguage === persistedLanguage) {
    if (nextLanguage === persistedLanguage && source === "settings") settingsAppLanguage.value = nextLanguage;
    return;
  }
  const previousPersisted = persistedLanguage;
  const previousLocale = getLocale();
  const startupViewDraft = settingsStartupView.value;
  languageBusy = true;
  languageButtons.forEach((button) => { button.disabled = true; });
  settingsAppLanguage.disabled = true;
  setTextIfChanged(source === "chooser" ? languageStatus : languageSettingsStatus, "");
  try {
    const info = await invoke<ShellInfo>("save_app_language", { appLanguage: nextLanguage });
    applyShellInfo(info);
    settingsStartupView.value = startupViewDraft;
    languageResolutionPending = false;
    rerenderLocalizedState();
    if (source === "chooser") {
      languageChooser.hidden = true;
      if (currentProvisioning?.state === "not_installed") await refreshSetupInfo();
      if (currentProvisioning) renderProvisioning(currentProvisioning);
      if (!backupSystemBusy()) await refreshRuntime();
    } else {
      setTextIfChanged(languageSettingsStatus, t("language.saved"));
    }
  } catch {
    persistedLanguage = previousPersisted;
    setLocale(previousLocale);
    rerenderLocalizedState();
    settingsAppLanguage.value = previousLocale;
    setTextIfChanged(
      source === "chooser" ? languageStatus : languageSettingsStatus,
      t("language.save_failed"),
    );
  } finally {
    languageBusy = false;
    languageButtons.forEach((button) => { button.disabled = false; });
    settingsAppLanguage.disabled = false;
  }
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
    ? t("onboarding.password_hint_existing")
    : t("onboarding.password_hint_new");
  name.disabled = !info.editable || setupProfileBusy || provisioningBusy || runtimeBusy || networkBusy || restoreSystemBusy();
  adminUsername.disabled = !info.editable || setupProfileBusy || provisioningBusy || runtimeBusy || networkBusy || restoreSystemBusy();
  adminEmail.disabled = !info.editable || setupProfileBusy || provisioningBusy || runtimeBusy || networkBusy || restoreSystemBusy();
  adminPassword.disabled = !info.editable || setupProfileBusy || provisioningBusy || runtimeBusy || networkBusy || restoreSystemBusy();
  adminPasswordConfirm.disabled = !info.editable || setupProfileBusy || provisioningBusy || runtimeBusy || networkBusy || restoreSystemBusy();
  save.disabled = !info.editable || setupProfileBusy || provisioningBusy || runtimeBusy || networkBusy || restoreSystemBusy();
  provisionWordPress.disabled =
    !info.editable || !info.password_configured || setupProfileBusy || provisioningBusy || runtimeBusy || networkBusy || restoreSystemBusy();
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
    message.textContent = localizedNativeError(error, "errors.config");
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
    return fieldError(name, t("onboarding.validation.store_length"));
  }
  if (/[<>]/.test(storeName) || /  /.test(storeName) || /%[0-9A-Fa-f]{2}/.test(storeName)) {
    return fieldError(name, t("onboarding.validation.store_unsafe"));
  }
  if (!/^[A-Za-z0-9._-]{3,60}$/.test(username)) {
    return fieldError(adminUsername, t("onboarding.validation.username_invalid"));
  }
  const emailPattern = /^[A-Za-z0-9.+_-]+@[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?(?:\.[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?)+$/;
  const [emailLocal = ""] = email.split("@");
  if (!adminEmail.checkValidity() || email.length > 100 || emailLocal.length > 64 || emailLocal.startsWith(".") || emailLocal.endsWith(".") || emailLocal.includes("..") || !emailPattern.test(email)) {
    return fieldError(adminEmail, t("onboarding.validation.email_required"));
  }
  const passwordRequired = !currentSetupInfo?.password_configured;
  if ((passwordRequired || password.length > 0) && (passwordLength < 12 || passwordLength > 128 || /[\u0000-\u001f\u007f]/.test(password))) {
    return fieldError(adminPassword, t("onboarding.validation.password_invalid"));
  }
  if (confirmation.length > 0 && password.length === 0) {
    return fieldError(adminPassword, t("onboarding.validation.password_new_required"));
  }
  if ((passwordRequired || password.length > 0) && password !== confirmation) {
    return fieldError(adminPasswordConfirm, t("onboarding.validation.password_mismatch"));
  }
  return true;
}

function viewHeading(view: InstalledView): HTMLElement {
  if (view === "settings") return element<HTMLElement>("settings-title");
  if (view === "diagnostics") return element<HTMLElement>("diagnostics-title");
  return element<HTMLElement>("view-home");
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
  if (view === "settings" && moveFocus && isTauri() && !networkBusy && !backupSystemBusy()) {
    void refreshRuntime();
  }
}

function showBootstrapError(error: unknown): void {
  currentProvisioning = null;
  setup.hidden = true;
  installedShell.hidden = true;
  bootstrapPanel.hidden = false;
  languageChooser.hidden = true;
  if (!document.documentElement.dataset.i18nReady) {
    setLocale("vi");
    applyLocaleStaticText();
  }
  title.textContent = t("bootstrap.error_title");
  description.textContent = localizedNativeError(error, "errors.config");
  retry.hidden = false;
}

function applyInstallationLayout(info: ProvisioningInfo): void {
  if (languageResolutionPending) {
    setup.hidden = true;
    installedShell.hidden = true;
    return;
  }
  languageChooser.hidden = true;
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
  homeStoreName.textContent = verifiedStoreName || t("overview.default_store");
  const posReady = currentRuntime?.state === "running"
    && currentRuntime.wordpress_health === "healthy"
    && currentRuntime.coffeepos_health.state === "healthy";
  if (!posReady && !posOpenBusy) setTextIfChanged(homeOpenStatus, "");
  if (repairRouteRequired) {
    setTextIfChanged(homeState, t("overview.state.needs_repair"));
    setTextIfChanged(homeStatus, t("overview.repair_status"));
    setTextIfChanged(homeDetail, t("overview.repair_detail"));
    setTextIfChanged(homeDiagnostics, t("overview.open_repair"));
    setHomeAction(null);
    return;
  }
  if (restoreSystemBusy()) {
    setTextIfChanged(homeState, restoreNeedsRecovery() ? t("overview.state.action_required") : t("overview.state.restoring"));
    setTextIfChanged(homeStatus, restoreNeedsRecovery() ? t("overview.restore_recovery_status") : t("overview.restore_active_status"));
    setTextIfChanged(
      homeDetail,
      restoreNeedsRecovery()
        ? t("overview.restore_recovery_detail")
        : t("overview.restore_active_detail"),
    );
    setTextIfChanged(homeDiagnostics, t("overview.open_restore"));
    setHomeAction(null);
    return;
  }
  if (backupOperation === "create" || backupOperation === "cancel" || backupIsActive()) {
    setTextIfChanged(homeState, t("overview.state.backing_up"));
    setTextIfChanged(homeStatus, t("overview.backup_status"));
    setTextIfChanged(homeDetail, t("overview.backup_detail"));
    setTextIfChanged(homeDiagnostics, t("overview.open_backup"));
    setHomeAction(null);
    return;
  }
  setTextIfChanged(homeDiagnostics, t("overview.open_system"));

  if (runtimeTransition === "starting") {
    setTextIfChanged(homeState, t("overview.state.starting"));
    setTextIfChanged(homeStatus, t("overview.starting_status"));
    setTextIfChanged(
      homeDetail,
      runtimeStartupProgress === "idle"
        ? t("overview.starting_detail")
        : t(`runtime.startup.${runtimeStartupProgress}`),
    );
    setHomeAction("start", t("overview.action.starting"), true);
    return;
  }

  if (runtimeTransition === "stopping") {
    setTextIfChanged(homeState, t("overview.state.stopping"));
    setTextIfChanged(homeStatus, t("overview.stopping_status"));
    setTextIfChanged(homeDetail, t("overview.wait_detail"));
    setHomeAction("start", t("overview.action.stopping"), true);
    return;
  }

  if (runtimeTransition === "checking") {
    setTextIfChanged(homeState, t("overview.state.checking"));
    setTextIfChanged(homeStatus, t("overview.checking_status"));
    setTextIfChanged(homeDetail, t("overview.checking_detail"));
    setHomeAction("retry_health", t("overview.action.checking"), true);
    return;
  }

  if (!currentRuntime) {
    if (runtimeLoadError) {
      setTextIfChanged(homeState, t("common.error"));
      setTextIfChanged(homeStatus, t("overview.read_failed_status"));
      setTextIfChanged(homeDetail, t("overview.read_failed_detail"));
      setHomeAction("refresh", t("common.retry"));
    } else {
      setTextIfChanged(homeState, t("overview.state.checking"));
      setTextIfChanged(homeStatus, t("overview.reading_status"));
      setTextIfChanged(homeDetail, t("overview.reading_detail"));
      setHomeAction(null);
    }
    return;
  }

  if (currentRuntime.state === "stopped") {
    if (currentRuntime.last_error) {
      setTextIfChanged(homeState, t("common.error"));
      setTextIfChanged(homeStatus, t("overview.start_failed_status"));
      setTextIfChanged(homeDetail, t("overview.start_failed_detail"));
      setHomeAction("start", t("common.retry"));
    } else {
      setTextIfChanged(homeState, t("overview.state.installed"));
      setTextIfChanged(homeStatus, t("overview.stopped_status"));
      setTextIfChanged(homeDetail, t("overview.stopped_detail"));
      setHomeAction("start", t("overview.action.start"));
    }
    return;
  }

  if (currentRuntime.state === "starting" || currentRuntime.state === "installing") {
    setTextIfChanged(homeState, t("overview.state.starting"));
    setTextIfChanged(homeStatus, t("overview.starting_status"));
    setTextIfChanged(homeDetail, t("overview.starting_detail"));
    setHomeAction("start", t("overview.action.starting"), true);
    return;
  }

  if (currentRuntime.state === "stopping") {
    setTextIfChanged(homeState, t("overview.state.stopping"));
    setTextIfChanged(homeStatus, t("overview.stopping_status"));
    setTextIfChanged(homeDetail, t("overview.wait_detail"));
    setHomeAction("start", t("overview.action.stopping"), true);
    return;
  }

  if (currentRuntime.state !== "running") {
    setTextIfChanged(homeState, t("common.error"));
    setTextIfChanged(homeStatus, t("overview.not_ready_status"));
    setTextIfChanged(homeDetail, t("overview.not_ready_detail"));
    setHomeAction(null);
    return;
  }

  if (currentRuntime.wordpress_health === "checking") {
    setTextIfChanged(homeState, t("overview.state.checking"));
    setTextIfChanged(homeStatus, t("overview.health_check_status"));
    setTextIfChanged(homeDetail, t("overview.health_check_detail"));
    setHomeAction(null);
    return;
  }

  if (currentRuntime.wordpress_health !== "healthy") {
    setTextIfChanged(homeState, t("common.error"));
    setTextIfChanged(homeStatus, t("overview.store_not_ready"));
    setTextIfChanged(homeDetail, t("overview.store_not_ready_detail"));
    setHomeAction("retry_health", t("common.retry"));
    return;
  }

  if (currentRuntime.coffeepos_health.state === "healthy") {
    setTextIfChanged(homeState, t("common.ready"));
    setTextIfChanged(homeStatus, t("overview.ready_status"));
    setTextIfChanged(homeDetail, t("overview.ready_detail"));
    setHomeAction("open_pos", posOpenBusy ? t("overview.action.opening") : t("overview.action.open_pos"), posOpenBusy);
    return;
  }

  if (currentRuntime.coffeepos_health.state === "degraded") {
    setTextIfChanged(homeState, t("overview.state.needs_check"));
    setTextIfChanged(homeStatus, t("overview.store_not_ready"));
    setTextIfChanged(homeDetail, t("overview.degraded_detail"));
    setHomeAction("retry_health", t("common.retry"));
    return;
  }

  if (currentRuntime.coffeepos_health.state === "failed") {
    setTextIfChanged(homeState, t("common.error"));
    setTextIfChanged(homeStatus, t("overview.check_failed_status"));
    setTextIfChanged(homeDetail, t("overview.check_failed_detail"));
    setHomeAction("retry_health", t("common.retry"));
    return;
  }

  setTextIfChanged(homeState, t("overview.state.checking"));
  setTextIfChanged(homeStatus, t("overview.health_check_status"));
  setTextIfChanged(homeDetail, t("overview.health_check_detail"));
  setHomeAction(null);
}

function setRuntimeControls(info: RuntimeInfo | null): void {
  if (provisioningBusy || runtimeBusy || networkBusy || diagnosticsBusy || repairOperation !== null || backupSystemBusy() || repairRouteRequired || !info) {
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

function renderNetworkSettings(): void {
  const network = currentRuntime?.network ?? null;
  const blocked = networkBusy
    || provisioningBusy
    || runtimeBusy
    || repairOperation !== null
    || backupSystemBusy()
    || repairRouteRequired
    || currentProvisioning?.state !== "ready"
    || !isTauri();

  settingsNetwork.setAttribute("aria-busy", networkBusy ? "true" : "false");
  settingsNetworkConfirmation.hidden = !networkConfirmOpen || networkBusy;
  settingsNetworkConfirm.disabled = blocked;
  settingsNetworkCancel.disabled = networkBusy;

  if (networkBusy) {
    setTextIfChanged(settingsNetworkState, t("settings.network_state.applying"));
  } else if (!network) {
    setTextIfChanged(settingsNetworkState, t("settings.network_state.unavailable"));
  } else if (network.configured_mode === "lan" && network.effective_mode === "lan") {
    setTextIfChanged(settingsNetworkState, t("settings.network_state.lan"));
  } else if (network.configured_mode === "lan") {
    setTextIfChanged(settingsNetworkState, t("settings.network_state.configured"));
  } else {
    setTextIfChanged(settingsNetworkState, t("settings.network_state.local_only"));
  }

  if (!network) {
    setTextIfChanged(settingsNetworkDetail, t("settings.network_detail.unavailable"));
  } else if (network.configured_mode === "lan" && network.effective_mode === "lan") {
    setTextIfChanged(
      settingsNetworkDetail,
      t("settings.network_detail.lan", { adapter: network.adapter_name ?? t("settings.network_adapter_unknown") }),
    );
  } else if (network.configured_mode === "lan") {
    setTextIfChanged(
      settingsNetworkDetail,
      t("settings.network_detail.configured", { adapter: network.adapter_name ?? t("settings.network_adapter_unknown") }),
    );
  } else {
    setTextIfChanged(settingsNetworkDetail, t("settings.network_detail.local_only"));
  }

  const configuredLan = network?.configured_mode === "lan";
  setTextIfChanged(settingsNetworkToggle, t(configuredLan ? "settings.network_disable" : "settings.network_enable"));
  settingsNetworkToggle.disabled = blocked || !network;

  const nativeNetworkError = network?.last_error
    ? localizedErrorCode(network.last_error.code, "errors.network")
    : "";
  const statusText = networkFeedback || nativeNetworkError;
  setTextIfChanged(settingsNetworkStatus, statusText);
  settingsNetworkStatus.classList.toggle("error", Boolean(statusText) && (networkFeedbackError || (!networkFeedback && Boolean(nativeNetworkError))));
}

function renderRuntime(info: RuntimeInfo): void {
  currentRuntime = info;
  element("runtime-state").textContent = t(`runtime.state.${info.state}`);
  element("runtime-php").textContent = info.php_version ?? "—";
  element("runtime-mariadb").textContent = info.mariadb_version ?? "—";
  element("runtime-http").textContent = info.http_port ? `127.0.0.1:${info.http_port}` : "—";
  element("runtime-database").textContent = info.database_port ? `127.0.0.1:${info.database_port}` : "—";
  wordpressHealth.textContent = t(`runtime.health.${info.wordpress_health}`);
  setHiddenIfChanged(wordpressHealthError, !info.wordpress_error);
  setTextIfChanged(wordpressHealthError, info.wordpress_error ? structuredErrorText(info.wordpress_error) : "");
  coffeeposHealth.textContent = t(`runtime.health.${info.coffeepos_health.state}`);
  setHiddenIfChanged(coffeeposHealthError, !info.coffeepos_health.error);
  setTextIfChanged(
    coffeeposHealthError,
    info.coffeepos_health.error ? structuredErrorText(info.coffeepos_health.error) : "",
  );
  const appHealth = info.coffeepos_health.payload;
  coffeeposHealthDetails.textContent = appHealth
    ? t("runtime.health_details", {
      store: appHealth.store.name,
      wordpressVersion: appHealth.versions.wordpress,
      wordpressState: t(appHealth.wordpress ? "common.ready_lower" : "common.not_ready_lower"),
      woocommerceVersion: appHealth.versions.woocommerce,
      woocommerceState: t(appHealth.woocommerce ? "common.ready_lower" : "common.not_ready_lower"),
      coffeeposVersion: appHealth.versions.coffeepos,
      coffeeposState: t(appHealth.coffeepos ? "common.ready_lower" : "common.not_ready_lower"),
      schema: appHealth.versions.coffeepos_schema,
      databaseState: t(appHealth.database ? "common.ready_lower" : "common.not_ready_lower"),
      posPath: appHealth.pos_path,
    })
    : "";
  setRuntimeControls(info);
  renderNetworkSettings();
  setHealthControls();

  if (provisioningBusy) {
    setTextIfChanged(runtimeDescription, t("runtime.description.provisioning"));
  } else if (runtimeBusy) {
    setTextIfChanged(
      runtimeDescription,
      runtimeTransition === "starting"
        ? t(`runtime.startup.${runtimeStartupProgress}`)
        : t("runtime.description.updating"),
    );
  } else if (info.last_error) {
    setTextIfChanged(runtimeDescription, structuredErrorText(info.last_error));
  } else if (info.state === "not_installed") {
    setTextIfChanged(runtimeDescription, t("runtime.description.not_installed"));
  } else if (info.state === "running") {
    if (info.wordpress_health === "healthy") {
      if (info.coffeepos_health.state === "healthy") {
        setTextIfChanged(runtimeDescription, t("runtime.description.healthy"));
      } else if (info.coffeepos_health.state === "degraded") {
        setTextIfChanged(runtimeDescription, t("runtime.description.degraded"));
      } else if (info.coffeepos_health.state === "failed") {
        setTextIfChanged(runtimeDescription, t("runtime.description.failed"));
      } else {
        setTextIfChanged(runtimeDescription, t("runtime.description.pending_app"));
      }
    } else if (info.wordpress_health === "unhealthy") {
      setTextIfChanged(runtimeDescription, t("runtime.description.wordpress_unhealthy"));
    } else {
      setTextIfChanged(runtimeDescription, t("runtime.description.wordpress_checking"));
    }
  } else if (info.state === "stopped") {
      setTextIfChanged(runtimeDescription, t("runtime.description.stopped"));
  } else {
    setTextIfChanged(runtimeDescription, t("runtime.description.ready"));
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
  provisioningState.textContent = info.state === "not_installed"
    ? t("runtime.state.not_installed")
    : info.state === "installing"
      ? t("runtime.state.installing")
      : info.state === "ready"
        ? t("common.ready")
        : t("overview.state.needs_repair");
  const installing = provisioningBusy || info.state === "installing";
  const wordpressInstalled = Boolean(info.admin_username);
  provisioningWordPress.textContent = info.wordpress_version
    ? installing
      ? `${info.wordpress_version} · ${t(wordpressInstalled ? "common.completed" : "common.processing")}`
      : info.wordpress_version
    : "—";
  provisioningWooCommerce.textContent = info.woocommerce_version
    ? `${info.woocommerce_version} · ${t(info.woocommerce_active ? "common.active" : installing ? "common.installed" : "common.inactive")}`
    : installing
      ? t(wordpressInstalled ? "common.processing" : "common.not_checked")
      : "—";
  provisioningCoffeePos.textContent = info.coffeepos_version
    ? `${info.coffeepos_version} · ${t(info.coffeepos_active ? "common.active" : installing ? "common.installed" : "common.inactive")}`
    : installing
      ? t(info.woocommerce_active ? "common.processing" : "common.not_checked")
      : "—";
  provisioningAdmin.textContent = info.admin_username
    ? installing
      ? `${info.admin_username} · ${t("common.completed")}`
      : info.admin_username
    : installing
      ? t("common.not_checked")
      : "—";
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

  if (installing) {
    provisioningState.textContent = t("runtime.state.installing");
    provisioningStatus.textContent = !wordpressInstalled
      ? t("onboarding.progress.wordpress")
      : !info.woocommerce_version
        ? t("onboarding.progress.woocommerce_install")
        : !info.woocommerce_active
          ? t("onboarding.progress.woocommerce_activate")
          : !info.coffeepos_version
            ? t("onboarding.progress.coffeepos_install")
            : !info.coffeepos_active
              ? t("onboarding.progress.coffeepos_activate")
              : t("onboarding.progress.finalizing");
    setupRetry.textContent = t("onboarding.installing_button");
    setupRetry.hidden = false;
    setupRetry.disabled = true;
  } else if (info.state === "not_installed") {
    provisioningStatus.textContent = t("onboarding.not_installed");
    setupRetry.hidden = true;
    provisionWordPress.disabled = setupProfileBusy || provisioningBusy || runtimeBusy || restoreSystemBusy() || !currentSetupInfo?.password_configured;
  } else if (info.state === "ready") {
    provisioningStatus.textContent = t("onboarding.ready");
    setupRetry.hidden = true;
  } else if (info.can_retry) {
    provisioningStatus.textContent = t("onboarding.retryable");
    setupRetry.textContent = t("onboarding.continue_setup");
    setupRetry.hidden = false;
    setupRetry.disabled = runtimeBusy || provisioningBusy;
    provisioningAction = "provision";
  } else {
    provisioningStatus.textContent = t("onboarding.needs_repair");
    setupRetry.textContent = t("restore.refresh_recovery");
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

async function refreshProvisioningProgress(): Promise<void> {
  if (!provisioningBusy || provisioningProgressRefreshBusy || !isTauri()) return;
  provisioningProgressRefreshBusy = true;
  try {
    const info = await invoke<ProvisioningInfo>("get_provisioning_info");
    await renderProvisioningWithRepairRouting(info);
  } catch {
    // The foreground provisioning command remains authoritative. A transient progress read
    // should not replace the install surface with an error while the worker is still running.
  } finally {
    provisioningProgressRefreshBusy = false;
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
    runtimeLoadError = localizedNativeError(error, "errors.runtime");
    setTextIfChanged(runtimeDescription, runtimeLoadError);
    setRuntimeControls(null);
    renderNetworkSettings();
    renderHome();
  } finally {
    runtimeRefreshBusy = false;
  }
}

async function refreshRuntimeStartupProgress(): Promise<void> {
  if (
    !isTauri()
    || !runtimeBusy
    || runtimeTransition !== "starting"
    || runtimeStartupProgressRefreshBusy
  ) return;
  runtimeStartupProgressRefreshBusy = true;
  try {
    runtimeStartupProgress = await invoke<RuntimeStartupStage>("get_runtime_startup_progress");
    setTextIfChanged(runtimeDescription, t(`runtime.startup.${runtimeStartupProgress}`));
    renderHome();
  } catch {
    // The foreground lifecycle command remains authoritative; keep the last known stage if this
    // lightweight progress read is temporarily unavailable.
  } finally {
    runtimeStartupProgressRefreshBusy = false;
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
  const installingInfo: ProvisioningInfo = {
    state: "installing",
    wordpress_version: currentProvisioning?.wordpress_version ?? "",
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
    failure = localizedNativeError(error, "errors.provisioning");
  } finally {
    provisioningBusy = false;
  }

  if (result) {
    completionPending = result.state === "ready";
    await renderProvisioningWithRepairRouting(result);
  } else {
    await refreshProvisioning(failure ?? t("errors.provisioning"));
  }
  await refreshRuntime();
}

async function copyAdminPassword(status: HTMLElement, button: HTMLButtonElement): Promise<void> {
  if (provisioningBusy || runtimeBusy || repairOperation || backupSystemBusy()) return;
  button.disabled = true;
  setTextIfChanged(status, t("settings.copying"));
  try {
    await invoke<void>("copy_admin_password");
    setTextIfChanged(status, t("settings.copied"));
  } catch (error) {
    setTextIfChanged(status, localizedNativeError(error, "errors.config"));
  } finally {
    button.disabled = false;
  }
}

async function runtimeAction(command: "start_runtime" | "stop_runtime" | "restart_runtime" | "retry_runtime_health"): Promise<void> {
  if (provisioningBusy || runtimeBusy || networkBusy || diagnosticsBusy || repairOperation || backupSystemBusy() || repairRouteRequired || currentProvisioning?.state !== "ready") return;
  runtimeBusy = true;
  runtimeTransition = command === "stop_runtime" ? "stopping" : command === "retry_runtime_health" ? "checking" : "starting";
  if (runtimeTransition === "starting") runtimeStartupProgress = "preparing";
  if (currentProvisioning) renderProvisioning(currentProvisioning);
  setRuntimeControls(null);
  if (command !== "retry_runtime_health") element("runtime-state").textContent = t(`runtime.state.${runtimeTransition}`);
  wordpressHealth.textContent = t("runtime.health.unavailable");
  setHiddenIfChanged(wordpressHealthError, true);
  setTextIfChanged(wordpressHealthError, "");
  coffeeposHealth.textContent = t("runtime.health.unavailable");
  setHiddenIfChanged(coffeeposHealthError, true);
  setTextIfChanged(coffeeposHealthError, "");
  coffeeposHealthDetails.textContent = "";
  openWordPressStatus.textContent = "";
  homeOpenStatus.textContent = "";
  if (currentView === "diagnostics") {
    if (command === "stop_runtime") {
      renderHealthChecking(t("runtime.health_stale.stop_title"), t("runtime.health_stale.stop"));
    } else if (command === "restart_runtime") {
      renderHealthChecking(t("runtime.health_stale.restart_title"), t("runtime.health_stale.restart"));
    } else {
      renderHealthChecking();
    }
  }
  runtimeDescription.textContent = command === "stop_runtime"
    ? t("runtime.transition.stop")
    : command === "retry_runtime_health"
      ? t("runtime.transition.health")
      : t(`runtime.startup.${runtimeStartupProgress}`);
  renderHome();

  try {
    renderRuntime(await invoke<RuntimeInfo>(command));
  } catch (error) {
    runtimeDescription.textContent = localizedNativeError(error, "errors.runtime");
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
  if (homeAction.disabled || provisioningBusy || runtimeBusy || networkBusy || diagnosticsBusy || repairOperation || backupSystemBusy() || posOpenBusy) return;
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
  if (settingsBusy || networkBusy || backupSystemBusy()) return;
  settingsBusy = true;
  settingsSave.disabled = true;
  setTextIfChanged(settingsSaveStatus, t("settings.saving"));
  try {
    const info = await invoke<ShellInfo>("save_app_settings", { startupView: settingsStartupView.value });
    applyShellInfo(info);
    setTextIfChanged(settingsSaveStatus, t("settings.saved"));
  } catch (error) {
    setTextIfChanged(settingsSaveStatus, localizedNativeError(error, "errors.config"));
  } finally {
    settingsBusy = false;
    settingsSave.disabled = backupSystemBusy() || networkBusy;
  }
}

async function applyNetworkMode(mode: "local_only" | "lan"): Promise<void> {
  if (
    networkBusy
    || provisioningBusy
    || runtimeBusy
    || repairOperation
    || backupSystemBusy()
    || repairRouteRequired
    || currentProvisioning?.state !== "ready"
    || !isTauri()
  ) return;

  networkBusy = true;
  networkConfirmOpen = false;
  networkFeedback = t(mode === "lan" ? "settings.network_enabling" : "settings.network_disabling");
  networkFeedbackError = false;
  syncOperationControls();
  renderNetworkSettings();
  try {
    const info = await invoke<RuntimeInfo>("set_network_mode", { networkMode: mode });
    renderRuntime(info);
    networkFeedback = t(mode === "lan" ? "settings.network_enabled" : "settings.network_disabled");
  } catch (error) {
    networkFeedback = localizedNativeError(error, "errors.network");
    networkFeedbackError = true;
  } finally {
    networkBusy = false;
    await refreshRuntime();
    syncOperationControls();
    renderNetworkSettings();
  }
}

async function openManagedWordPress(): Promise<void> {
  if (provisioningBusy || runtimeBusy || networkBusy || repairOperation || backupSystemBusy() || repairRouteRequired || openWordPress.disabled) return;
  openWordPress.disabled = true;
  openWordPressStatus.textContent = t("runtime.opening_wordpress");
  try {
    const url = await invoke<string>("open_wordpress");
    openWordPressStatus.textContent = t("runtime.opened_url", { url });
  } catch (error) {
    openWordPressStatus.textContent = localizedNativeError(error, "errors.runtime");
    await refreshRuntime();
  } finally {
    setRuntimeControls(currentRuntime);
  }
}

async function openPos(): Promise<void> {
  if (provisioningBusy || runtimeBusy || networkBusy || repairOperation || backupSystemBusy() || repairRouteRequired || posOpenBusy || currentProvisioning?.state !== "ready") return;
  posOpenBusy = true;
  renderHome();
  setTextIfChanged(homeOpenStatus, t("runtime.opening_pos"));
  let feedback = "";
  try {
    await invoke<string>("open_pos");
    feedback = t("runtime.opened_pos");
  } catch (error) {
    feedback = localizedNativeError(error, "errors.runtime");
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
  languageResolutionPending = true;
  delete document.documentElement.dataset.i18nReady;
  setup.hidden = true;
  installedShell.hidden = true;
  languageChooser.hidden = true;
  bootstrapPanel.hidden = false;
  retry.hidden = true;
  retry.disabled = true;
  title.textContent = t("bootstrap.opening_title");
  description.textContent = t("bootstrap.opening_description");

  if (!isTauri()) {
    setLocale("vi");
    languageResolutionPending = false;
    applyLocaleStaticText();
    title.textContent = t("bootstrap.preview_title");
    description.textContent = t("bootstrap.preview_description");
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
    if (restoreIsActive() || restoreNeedsRecovery()) {
      languageResolutionPending = false;
      applyLocaleStaticText();
      return;
    }

    // Reconnect to an active native backup before provisioning/runtime reads. Backup status and
    // cancellation remain lock-independent so a WebView reload can resume progress immediately.
    await refreshBackupStatus();
    const provisioningLoaded = await refreshProvisioning();
    if (!provisioningLoaded) return;
    if (freshProfileNeedsLanguageChoice()) {
      showLanguageChooser();
      return;
    }
    languageResolutionPending = false;
    applyLocaleStaticText();
    if (currentProvisioning) applyInstallationLayout(currentProvisioning);
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
for (const button of languageButtons) {
  button.addEventListener("click", () => {
    const language = button.dataset.language;
    if (isAppLanguage(language)) void saveLanguage(language, "chooser");
  });
}
settingsAppLanguage.addEventListener("change", () => {
  const language = settingsAppLanguage.value;
  if (isAppLanguage(language)) void saveLanguage(language, "settings");
});
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
settingsNetworkToggle.addEventListener("click", () => {
  if (settingsNetworkToggle.disabled || networkBusy) return;
  if (currentRuntime?.network.configured_mode === "lan") {
    void applyNetworkMode("local_only");
    return;
  }
  networkConfirmOpen = true;
  networkFeedback = "";
  networkFeedbackError = false;
  renderNetworkSettings();
  settingsNetworkConfirm.focus();
});
settingsNetworkConfirm.addEventListener("click", () => void applyNetworkMode("lan"));
settingsNetworkCancel.addEventListener("click", () => {
  networkConfirmOpen = false;
  renderNetworkSettings();
  settingsNetworkToggle.focus();
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
  setTextIfChanged(backupState, t("common.ready"));
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
    message.textContent = localizedNativeError(error, "errors.config");
    message.hidden = false;
  } finally {
    setupProfileBusy = false;
    if (currentSetupInfo) applySetupInfo(currentSetupInfo);
  }
});

window.setInterval(() => {
  if (isTauri() && currentProvisioning?.state === "ready" && !bootstrapBusy && !provisioningBusy && !runtimeBusy && !networkBusy && !diagnosticsBusy && !repairOperation && !backupSystemBusy()) {
    void refreshRuntime();
  }
}, 2000);

window.setInterval(() => {
  if (isTauri() && provisioningBusy) void refreshProvisioningProgress();
}, 750);

window.setInterval(() => {
  if (isTauri() && runtimeBusy && runtimeTransition === "starting") void refreshRuntimeStartupProgress();
}, 500);

window.setInterval(() => {
  if (isTauri() && currentProvisioning?.state === "ready" && !bootstrapBusy && !networkBusy && !repairOperation && !backupSystemBusy()) {
    void refreshRuntimeMaintenance();
  }
}, 5000);

void bootstrap();
