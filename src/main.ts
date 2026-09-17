import { invoke, isTauri } from "@tauri-apps/api/core";
import "./styles.css";

interface ShellInfo {
  version: string;
  data_dir: string;
  config: { schema_version: number; store_name: string; bind_host: string };
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
    title.textContent = "Chưa cài đặt môi trường bán hàng";
    description.textContent = "Ứng dụng desktop đã sẵn sàng. PHP, MariaDB và WordPress chưa được cài đặt trong Phase 1.";
  } catch (error) {
    title.textContent = "Không thể mở cấu hình ứng dụng";
    description.textContent = String(error);
    retry.hidden = false;
  }
}

retry.addEventListener("click", () => void bootstrap());
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
