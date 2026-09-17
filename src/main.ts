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
const runtimeSection = element("runtime");
const runtimeDescription = element("runtime-description");
const runtimeStart = element<HTMLButtonElement>("runtime-start");
const runtimeStop = element<HTMLButtonElement>("runtime-stop");
const runtimeRestart = element<HTMLButtonElement>("runtime-restart");

function renderRuntime(info: RuntimeInfo): void {
  runtimeSection.hidden = false;
  element("runtime-state").textContent = info.state;
  element("runtime-php").textContent = info.php_version ?? "—";
  element("runtime-mariadb").textContent = info.mariadb_version ?? "—";
  element("runtime-http").textContent = info.http_port ? `127.0.0.1:${info.http_port}` : "—";
  element("runtime-database").textContent = info.database_port ? `127.0.0.1:${info.database_port}` : "—";
  runtimeStart.disabled = info.state === "running" || info.state === "starting" || info.state === "stopping";
  runtimeStop.disabled = info.state !== "running";
  runtimeRestart.disabled = info.state === "starting" || info.state === "stopping" || info.state === "installing";
  if (info.last_error) {
    runtimeDescription.textContent = `${info.last_error.message} ${info.last_error.recovery}`;
  } else if (info.state === "not_installed") {
    runtimeDescription.textContent = "Runtime bundle đã sẵn sàng, nhưng dữ liệu MariaDB chưa được provision. Phase 2 không tự khởi tạo hoặc ghi đè database.";
  } else if (info.state === "running") {
    runtimeDescription.textContent = "MariaDB và PHP đã vượt qua readiness checks.";
  } else {
    runtimeDescription.textContent = "Runtime manager đã sẵn sàng.";
  }
}

async function refreshRuntime(): Promise<void> {
  try {
    renderRuntime(await invoke<RuntimeInfo>("get_runtime_info"));
  } catch (error) {
    runtimeSection.hidden = false;
    runtimeDescription.textContent = String(error);
    runtimeStart.disabled = true;
    runtimeStop.disabled = true;
    runtimeRestart.disabled = true;
  }
}

async function runtimeAction(command: "start_runtime" | "stop_runtime" | "restart_runtime"): Promise<void> {
  runtimeStart.disabled = true;
  runtimeStop.disabled = true;
  runtimeRestart.disabled = true;
  runtimeDescription.textContent = "Đang cập nhật runtime…";
  try {
    renderRuntime(await invoke<RuntimeInfo>(command));
  } catch (error) {
    runtimeDescription.textContent = String(error);
    await refreshRuntime();
  }
}

async function bootstrap(): Promise<void> {
  retry.hidden = true;
  title.textContent = "Đang đọc cấu hình…";
  if (!isTauri()) {
    title.textContent = "Bản xem trước giao diện";
    description.textContent = "Chạy npm run dev để mở ứng dụng desktop. Bản xem trước không đọc hoặc ghi dữ liệu trên máy.";
    return;
  }
  try {
    const info = await invoke<ShellInfo>("get_shell_info");
    name.value = info.config.store_name;
    element("data-dir").textContent = info.data_dir;
    element("version").textContent = info.version;
    element("settings").hidden = false;
    element("setup").hidden = false;
    title.textContent = "Desktop shell đã sẵn sàng";
    description.textContent = "Phase 2 quản lý PHP và MariaDB. WordPress provisioning sẽ được thêm ở phase tiếp theo.";
    await refreshRuntime();
  } catch (error) {
    title.textContent = "Không thể mở cấu hình ứng dụng";
    description.textContent = String(error);
    retry.hidden = false;
  }
}

retry.addEventListener("click", () => void bootstrap());
runtimeStart.addEventListener("click", () => void runtimeAction("start_runtime"));
runtimeStop.addEventListener("click", () => void runtimeAction("stop_runtime"));
runtimeRestart.addEventListener("click", () => void runtimeAction("restart_runtime"));
element<HTMLFormElement>("settings-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  save.disabled = true;
  message.textContent = "Đang lưu…";
  try {
    const info = await invoke<ShellInfo>("save_store_name", { storeName: name.value });
    name.value = info.config.store_name;
    message.textContent = "Đã lưu cấu hình.";
  } catch (error) {
    message.textContent = String(error);
  } finally {
    save.disabled = false;
  }
});
void bootstrap();
