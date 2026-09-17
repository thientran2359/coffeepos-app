# Dot-source from PowerShell: . ./scripts/use-local-rust.ps1
$taskRoot = Split-Path -Parent $PSScriptRoot
$taskCargo = Join-Path $taskRoot '.tools/cargo'
$taskRustup = Join-Path $taskRoot '.tools/rustup'
if (-not (Test-Path (Join-Path $taskCargo 'bin/cargo.exe'))) {
    throw 'No workspace-local Rust toolchain found. Install Rust from rustup.rs, then reopen your terminal.'
}
$env:CARGO_HOME = $taskCargo
$env:RUSTUP_HOME = $taskRustup
$env:PATH = (Join-Path $taskCargo 'bin') + [IO.Path]::PathSeparator + $env:PATH
Write-Host 'Workspace Rust enabled for this PowerShell session only. MSVC Build Tools are still required.'
