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
  mariadb_version: string | null;
  database_port: number | null;
  http_port: number | null;
  database_pid: number | null;
  php_pid: number | null;
  wordpress_health: "unavailable" | "checking" | "healthy" | "unhealthy";
  wordpress_error: RuntimeErrorInfo | null;
  coffeepos_health: CoffeePosHealthInfo;
  last_error: RuntimeErrorInfo | null;
}

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
type SetupStep = "welcome" | "details" | "review" | "progress" | "complete";

function element<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (!node) throw new Error(`Missing UI element: ${id}`);
  return node as T;
}

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
const homeSettings = element<HTMLButtonElement>("home-settings");
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

function nativeErrorText(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  try {
    return JSON.stringify(error);
  } catch {
    return String(error);
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
  name.disabled = !info.editable || setupProfileBusy || provisioningBusy || runtimeBusy;
  adminUsername.disabled = !info.editable || setupProfileBusy || provisioningBusy || runtimeBusy;
  adminEmail.disabled = !info.editable || setupProfileBusy || provisioningBusy || runtimeBusy;
  adminPassword.disabled = !info.editable || setupProfileBusy || provisioningBusy || runtimeBusy;
  adminPasswordConfirm.disabled = !info.editable || setupProfileBusy || provisioningBusy || runtimeBusy;
  save.disabled = !info.editable || setupProfileBusy || provisioningBusy || runtimeBusy;
  provisionWordPress.disabled =
    !info.editable || !info.password_configured || setupProfileBusy || provisioningBusy || runtimeBusy;
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

function selectInstalledView(view: InstalledView, moveFocus = true): void {
  if (installedShell.hidden) return;
  currentView = view;
  for (const button of navButtons) {
    button.setAttribute("aria-current", button.dataset.view === view ? "page" : "false");
  }
  for (const panel of viewPanels) {
    panel.hidden = panel.dataset.viewPanel !== view;
  }
  if (moveFocus) viewHeading(view).focus();
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
  const enteringInstalledShell = info.state === "ready" && installedShell.hidden === true;
  const enteringSetup = info.state !== "ready" && setup.hidden === true;
  bootstrapPanel.hidden = true;

  if (info.state === "ready") {
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
      setTextIfChanged(homeDetail, "Thử đọc lại trạng thái hoặc mở Chẩn đoán để xem chi tiết.");
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
      setTextIfChanged(homeDetail, "Lần khởi động gần nhất chưa hoàn tất. Bạn có thể thử lại hoặc xem chi tiết trong Chẩn đoán.");
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
    setTextIfChanged(homeDetail, "Mở Chẩn đoán để xem chi tiết trạng thái hiện tại.");
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
    setTextIfChanged(homeDetail, "Một thành phần của cửa hàng chưa sẵn sàng. Thử kiểm tra lại hoặc xem chi tiết trong Chẩn đoán.");
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
  if (provisioningBusy || runtimeBusy || !info) {
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
    setTextIfChanged(runtimeDescription, "Runtime đang dừng. Có thể khởi động lại từ Chẩn đoán khi cần.");
  } else {
    setTextIfChanged(runtimeDescription, "Runtime manager đã sẵn sàng.");
  }

  renderHome();
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
    provisionWordPress.disabled = setupProfileBusy || provisioningBusy || runtimeBusy || !currentSetupInfo?.password_configured;
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

async function refreshProvisioning(commandError?: string): Promise<boolean> {
  try {
    const info = await invoke<ProvisioningInfo>("get_provisioning_info");
    renderProvisioning(info, commandError);
    return true;
  } catch (error) {
    showBootstrapError(commandError ?? error);
    setRuntimeControls(null);
    return false;
  }
}

async function refreshRuntime(): Promise<void> {
  try {
    runtimeLoadError = null;
    renderRuntime(await invoke<RuntimeInfo>("get_runtime_info"));
  } catch (error) {
    currentRuntime = null;
    runtimeLoadError = nativeErrorText(error);
    setTextIfChanged(runtimeDescription, runtimeLoadError);
    setRuntimeControls(null);
    renderHome();
  }
}

async function provision(): Promise<void> {
  if (provisioningBusy || runtimeBusy) return;
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
    renderProvisioning(result);
  } else {
    await refreshProvisioning(failure ?? "Provisioning thất bại nhưng native layer không trả chi tiết lỗi.");
  }
  await refreshRuntime();
}

async function copyAdminPassword(status: HTMLElement, button: HTMLButtonElement): Promise<void> {
  if (provisioningBusy || runtimeBusy) return;
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
  if (provisioningBusy || runtimeBusy || currentProvisioning?.state !== "ready") return;
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
}

async function runHomeAction(): Promise<void> {
  if (homeAction.disabled || provisioningBusy || runtimeBusy || posOpenBusy) return;
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
  if (settingsBusy) return;
  settingsBusy = true;
  settingsSave.disabled = true;
  setTextIfChanged(settingsSaveStatus, "Đang lưu…");
  try {
    const info = await invoke<ShellInfo>("save_app_settings", { startupView: settingsStartupView.value });
    applyShellInfo(info);
    setTextIfChanged(settingsSaveStatus, "Đã lưu cài đặt Desktop.");
  } catch (error) {
    setTextIfChanged(settingsSaveStatus, nativeErrorText(error));
  } finally {
    settingsBusy = false;
    settingsSave.disabled = false;
  }
}

async function openManagedWordPress(): Promise<void> {
  if (provisioningBusy || runtimeBusy || openWordPress.disabled) return;
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
  if (provisioningBusy || runtimeBusy || posOpenBusy || currentProvisioning?.state !== "ready") return;
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

    const provisioningLoaded = await refreshProvisioning();
    if (!provisioningLoaded) return;
    if (currentProvisioning?.state === "not_installed") {
      await refreshSetupInfo();
      if (currentProvisioning) renderProvisioning(currentProvisioning);
    }
    await refreshRuntime();
    if (currentProvisioning?.state === "ready" && currentRuntime?.state === "stopped") {
      await runtimeAction("start_runtime");
    }
  } finally {
    bootstrapBusy = false;
    if (!retry.hidden) retry.disabled = false;
  }
}

retry.addEventListener("click", () => void bootstrap());
setupRetry.addEventListener("click", () => {
  if (provisioningAction === "refresh") void refreshProvisioning();
  else void provision();
});
provisionWordPress.addEventListener("click", () => void provision());
setupBegin.addEventListener("click", () => selectSetupStep("details", true));
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

homeAction.addEventListener("click", () => void runHomeAction());
homeSettings.addEventListener("click", () => selectInstalledView("settings", true));
homeDiagnostics.addEventListener("click", () => selectInstalledView("diagnostics", true));
runtimeStart.addEventListener("click", () => void runtimeAction("start_runtime"));
runtimeStop.addEventListener("click", () => void runtimeAction("stop_runtime"));
runtimeRestart.addEventListener("click", () => void runtimeAction("restart_runtime"));
openWordPress.addEventListener("click", () => void openManagedWordPress());

setupForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  if (setupProfileBusy || provisioningBusy || runtimeBusy || currentProvisioning?.state !== "not_installed") return;
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
  if (isTauri() && currentProvisioning?.state === "ready" && !bootstrapBusy && !provisioningBusy && !runtimeBusy) {
    void refreshRuntime();
  }
}, 2000);

void bootstrap();
