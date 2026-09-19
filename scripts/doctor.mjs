import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { join } from "node:path";

console.log(`CoffeePOS developer environment: ${process.platform}/${process.arch}`);
console.log(`Node: ${process.version}`);
let missing = false;
for (const command of ["cargo", "rustc"]) {
  const result = spawnSync(command, ["--version"], { encoding: "utf8", shell: false });
  console.log(`${command}: ${result.status === 0 ? result.stdout.trim() : "MISSING (install Rust via rustup, then reopen terminal)"}`);
  missing ||= result.status !== 0;
}
if (process.platform === "win32") {
  const vswhere = join(process.env["ProgramFiles(x86)"] ?? "", "Microsoft Visual Studio", "Installer", "vswhere.exe");
  const result = existsSync(vswhere) ? spawnSync(vswhere, ["-latest", "-products", "*", "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-property", "installationPath"], { encoding: "utf8" }) : null;
  const available = result?.status === 0 && Boolean(result.stdout.trim());
  console.log(`MSVC: ${available ? result.stdout.trim() : "NOT DETECTED (install Visual Studio C++ Build Tools + Windows SDK)"}`);
  console.log("WebView2: verify with npm run tauri -- info; needed to launch the Windows shell.");
  missing ||= !available;
} else if (process.platform === "darwin") {
  const result = spawnSync("xcode-select", ["-p"], { encoding: "utf8" });
  console.log(`Xcode tools: ${result.status === 0 ? result.stdout.trim() : "MISSING (run xcode-select --install)"}`);
  missing ||= result.status !== 0;
} else {
  console.log("This project targets Windows and macOS; other platforms are not qualified.");
  missing = true;
}
const stagedRuntime = join(process.cwd(), "runtime", "development", "x86_64-pc-windows-msvc");
const stagedPhp = join(stagedRuntime, "php", "php.exe");
const stagedPhpCgi = join(stagedRuntime, "php", "php-cgi.exe");
const stagedCaddy = join(stagedRuntime, "caddy", "caddy.exe");
const stagedIni = join(stagedRuntime, "php", "php.ini");
if (existsSync(stagedPhp) && existsSync(stagedIni)) {
  const modules = spawnSync(stagedPhp, ["-c", stagedIni, "-m"], { encoding: "utf8", shell: false });
  const opcache = modules.status === 0 && modules.stdout.includes("Zend OPcache");
  const fastCgi = existsSync(stagedPhpCgi);
  const caddy = existsSync(stagedCaddy);
  console.log(`Managed runtime: FastCGI=${fastCgi ? "OK" : "MISSING"}, OPcache=${opcache ? "OK" : "MISSING"}, Caddy=${caddy ? "OK" : "MISSING"}`);
  missing ||= !fastCgi || !opcache || !caddy;
} else {
  console.log("Managed runtime: not staged (run scripts/stage-runtime-development.ps1 when runtime work is needed).");
}
console.log("PHP/MariaDB/Caddy are never resolved from PATH by the app.");
process.exitCode = missing ? 1 : 0;
