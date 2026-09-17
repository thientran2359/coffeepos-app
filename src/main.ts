import { invoke, isTauri } from "@tauri-apps/api/core";
import "./styles.css";

interface ShellInfo {
  version: string;
  data_dir: string;
  config: { schema_version: number; store_name: string; bind_host: string };
}

interface RuntimeErrorInfo {
  component: string;
  operation: string;
  message: string;
  recovery: string;
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
  last_error: RuntimeErrorInfo | null;
}

type ProvisioningState = "not_installed" | "installing" | "ready" | "needs_repair";

interface ProvisioningInfo {
  state: ProvisioningState;
  wordpress_version: string;
  admin_username: string | null;
  can_retry: boolean;
  last_error: RuntimeErrorInfo | null;
}

function element<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (!node) throw new Error(`Missing UI element: ${id}`);
  return node as T;
}

const title = element("status-title");
const description = element("status-description");
const retry = element<HTMLButtonElement>("retry");
const name = element<HTMLInputElement>("store-name");
const save = element<HTMLButtonElement>("save");
const message = element("save-status");
const setup = element("setup");
const provisioningState = element("provisioning-state");
const provisioningStatus = element("provisioning-status");
const provisioningError = element("provisioning-error");
const provisioningDetails = element("provisioning-details");
const provisioningWordPress = element("provisioning-wordpress");
const provisioningAdmin = element("provisioning-admin");
const provisionWordPress = element<HTMLButtonElement>("provision-wordpress");
const runtimeSection = element("runtime");
const runtimeDescription = element("runtime-description");
const runtimeStart = element<HTMLButtonElement>("runtime-start");
const runtimeStop = element<HTMLButtonElement>("runtime-stop");
const runtimeRestart = element<HTMLButtonElement>("runtime-restart");
const wordpressHealth = element("wordpress-health");
const wordpressHealthError = element("wordpress-health-error");

let provisioningBusy = false;
let runtimeBusy = false;
let currentProvisioning: ProvisioningInfo | null = null;
let currentRuntime: RuntimeInfo | null = null;
let provisioningAction: "provision" | "refresh" = "refresh";

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

function setRuntimeControls(info: RuntimeInfo | null): void {
  if (provisioningBusy || runtimeBusy || !info) {
    runtimeStart.disabled = true;
    runtimeStop.disabled = true;
    runtimeRestart.disabled = true;
    return;
  }
  const provisioningReady = currentProvisioning?.state === "ready";
  if (!provisioningReady) {
    runtimeStart.disabled = true;
    runtimeRestart.disabled = true;
    runtimeStop.disabled = info.state !== "running";
    return;
  }
  runtimeStart.disabled = info.state === "running" || info.state === "starting" || info.state === "stopping";
  runtimeStop.disabled = info.state !== "running";
  runtimeRestart.disabled = info.state === "starting" || info.state === "stopping" || info.state === "installing";
}

function renderRuntime(info: RuntimeInfo): void {
  currentRuntime = info;
  runtimeSection.hidden = false;
  element("runtime-state").textContent = info.state;
  element("runtime-php").textContent = info.php_version ?? "—";
  element("runtime-mariadb").textContent = info.mariadb_version ?? "—";
  element("runtime-http").textContent = info.http_port ? `127.0.0.1:${info.http_port}` : "—";
  element("runtime-database").textContent = info.database_port ? `127.0.0.1:${info.database_port}` : "—";
  wordpressHealth.textContent = info.wordpress_health;
  wordpressHealthError.hidden = !info.wordpress_error;
  wordpressHealthError.textContent = info.wordpress_error ? structuredErrorText(info.wordpress_error) : "";
  setRuntimeControls(info);
  if (provisioningBusy) {
    runtimeDescription.textContent = "Runtime controls tạm khóa trong khi WordPress đang được provision.";
  } else if (runtimeBusy) {
    runtimeDescription.textContent = "Đang cập nhật runtime…";
  } else if (info.last_error) {
    runtimeDescription.textContent = `${info.last_error.message} ${info.last_error.recovery}`;
  } else if (info.state === "not_installed") {
    runtimeDescription.textContent = "Runtime bundle đã sẵn sàng; WordPress/database chưa được provision.";
  } else if (info.state === "running") {
    if (info.wordpress_health === "healthy") {
      runtimeDescription.textContent = "MariaDB, PHP và WordPress đã vượt qua readiness checks.";
    } else if (info.wordpress_health === "unhealthy") {
      runtimeDescription.textContent = "MariaDB và PHP đang chạy, nhưng WordPress chưa healthy. Có thể khởi động lại runtime để thử lại; không cần cài lại WordPress.";
    } else {
      runtimeDescription.textContent = "MariaDB và PHP đang chạy; đang xác minh WordPress.";
    }
  } else {
    runtimeDescription.textContent = "Runtime manager đã sẵn sàng.";
  }
}

function renderProvisioning(info: ProvisioningInfo, commandError?: string): void {
  currentProvisioning = info;
  provisioningAction = "provision";
  setup.hidden = false;
  setup.setAttribute("aria-busy", provisioningBusy ? "true" : "false");
  provisioningState.textContent = info.state;
  provisioningWordPress.textContent = info.wordpress_version || "—";
  provisioningAdmin.textContent = info.admin_username ?? "—";
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
    provisioningStatus.textContent = "Đang tạo database và cài WordPress. Không đóng app hoặc điều khiển runtime trong lúc này.";
    provisionWordPress.textContent = "Đang cài đặt…";
    provisionWordPress.hidden = false;
    provisionWordPress.disabled = true;
  } else if (info.state === "not_installed") {
    provisioningStatus.textContent = "WordPress chưa được cài. Thao tác này sẽ tạo database và WordPress store cục bộ trên máy này.";
    provisionWordPress.textContent = "Cài đặt WordPress";
    provisionWordPress.hidden = false;
    provisionWordPress.disabled = false;
  } else if (info.state === "ready") {
    provisioningStatus.textContent = `WordPress ${info.wordpress_version} đã được cài và native provisioning báo Ready.`;
    provisionWordPress.hidden = true;
    provisionWordPress.disabled = true;
  } else if (info.can_retry) {
    provisioningStatus.textContent = "Store chưa ở trạng thái hoàn chỉnh. Có thể thử lại provisioning; dữ liệu hiện có sẽ không bị frontend tự xóa.";
    provisionWordPress.textContent = "Thử lại provisioning";
    provisionWordPress.hidden = false;
    provisionWordPress.disabled = runtimeBusy;
  } else {
    provisioningStatus.textContent = "Store cần repair trước khi provisioning có thể tiếp tục an toàn. Hãy làm theo hướng dẫn lỗi bên dưới.";
    provisionWordPress.textContent = "Kiểm tra lại trạng thái";
    provisionWordPress.hidden = false;
    provisionWordPress.disabled = runtimeBusy;
    provisioningAction = "refresh";
  }

  if (info.state === "not_installed") {
    provisionWordPress.disabled = runtimeBusy;
  }
  name.disabled = provisioningBusy || runtimeBusy;
  save.disabled = provisioningBusy || runtimeBusy;
  setRuntimeControls(currentRuntime);
}

function renderProvisioningCommandFailure(error: unknown): void {
  const text = nativeErrorText(error);
  provisioningAction = "refresh";
  setup.hidden = false;
  setup.setAttribute("aria-busy", "false");
  provisioningState.textContent = "error";
  provisioningStatus.textContent = "Không thể đọc trạng thái provisioning từ native layer.";
  provisioningError.textContent = text;
  provisioningError.hidden = false;
  provisioningDetails.hidden = true;
  provisionWordPress.textContent = "Kiểm tra lại trạng thái";
  provisionWordPress.hidden = false;
  provisionWordPress.disabled = false;
  setRuntimeControls(currentRuntime);
}

async function refreshProvisioning(commandError?: string): Promise<void> {
  try {
    const info = await invoke<ProvisioningInfo>("get_provisioning_info");
    renderProvisioning(info, commandError);
  } catch (error) {
    currentProvisioning = null;
    renderProvisioningCommandFailure(commandError ?? error);
  }
}

async function refreshRuntime(): Promise<void> {
  try {
    renderRuntime(await invoke<RuntimeInfo>("get_runtime_info"));
  } catch (error) {
    currentRuntime = null;
    runtimeSection.hidden = false;
    runtimeDescription.textContent = nativeErrorText(error);
    setRuntimeControls(null);
  }
}

async function provision(): Promise<void> {
  if (provisioningBusy || runtimeBusy) return;
  provisioningBusy = true;
  const installingInfo: ProvisioningInfo = currentProvisioning ?? {
    state: "installing",
    wordpress_version: "",
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
    renderProvisioning(result);
  } else {
    await refreshProvisioning(failure ?? "Provisioning thất bại nhưng native layer không trả chi tiết lỗi.");
  }
  await refreshRuntime();
}

async function runtimeAction(command: "start_runtime" | "stop_runtime" | "restart_runtime"): Promise<void> {
  if (provisioningBusy || runtimeBusy) return;
  runtimeBusy = true;
  if (currentProvisioning) renderProvisioning(currentProvisioning);
  setRuntimeControls(null);
  const transition = command === "stop_runtime" ? "stopping" : "starting";
  element("runtime-state").textContent = transition;
  wordpressHealth.textContent = "unavailable";
  wordpressHealthError.hidden = true;
  wordpressHealthError.textContent = "";
  runtimeDescription.textContent = command === "stop_runtime" ? "Đang dừng PHP và MariaDB…" : "Đang khởi động runtime và kiểm tra WordPress…";
  try {
    renderRuntime(await invoke<RuntimeInfo>(command));
  } catch (error) {
    runtimeDescription.textContent = nativeErrorText(error);
    await refreshRuntime();
  } finally {
    runtimeBusy = false;
    if (currentProvisioning) renderProvisioning(currentProvisioning);
    if (currentRuntime) renderRuntime(currentRuntime);
  }
}

async function bootstrap(): Promise<void> {
  retry.hidden = true;
  title.textContent = "Đang đọc cấu hình…";
  if (!isTauri()) {
    title.textContent = "Bản xem trước giao diện";
    description.textContent = "Chạy npm run dev để mở ứng dụng desktop. Bản xem trước không gọi native provisioning.";
    return;
  }
  try {
    const info = await invoke<ShellInfo>("get_shell_info");
    name.value = info.config.store_name;
    element("data-dir").textContent = info.data_dir;
    element("version").textContent = info.version;
    element("settings").hidden = false;
    title.textContent = "Desktop shell đã sẵn sàng";
    description.textContent = "Phase 4.2 theo dõi riêng trạng thái cài đặt, runtime readiness và WordPress health.";
    await refreshProvisioning();
    await refreshRuntime();
  } catch (error) {
    title.textContent = "Không thể mở cấu hình ứng dụng";
    description.textContent = nativeErrorText(error);
    retry.hidden = false;
  }
}

retry.addEventListener("click", () => void bootstrap());
provisionWordPress.addEventListener("click", () => {
  if (provisioningAction === "refresh") {
    void refreshProvisioning();
  } else {
    void provision();
  }
});
runtimeStart.addEventListener("click", () => void runtimeAction("start_runtime"));
runtimeStop.addEventListener("click", () => void runtimeAction("stop_runtime"));
runtimeRestart.addEventListener("click", () => void runtimeAction("restart_runtime"));
element<HTMLFormElement>("settings-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  if (provisioningBusy || runtimeBusy) return;
  save.disabled = true;
  message.textContent = "Đang lưu…";
  try {
    const info = await invoke<ShellInfo>("save_store_name", { storeName: name.value });
    name.value = info.config.store_name;
    message.textContent = "Đã lưu cấu hình.";
  } catch (error) {
    message.textContent = nativeErrorText(error);
  } finally {
    save.disabled = false;
  }
});
window.setInterval(() => {
  if (isTauri() && !provisioningBusy && !runtimeBusy) void refreshRuntime();
}, 2000);
void bootstrap();
