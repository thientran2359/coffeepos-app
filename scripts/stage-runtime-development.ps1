[CmdletBinding()]
param(
    [switch]$ForceDownload
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$scriptRoot = Split-Path -Parent $PSCommandPath
$projectRoot = Split-Path -Parent $scriptRoot
$runtimeRoot = [IO.Path]::GetFullPath((Join-Path $projectRoot 'runtime/development'))
$templateRoot = Join-Path $scriptRoot 'runtime-development'
$manifestTemplate = Join-Path $templateRoot 'x86_64-pc-windows-msvc.manifest.json'
$manifest = Get-Content -LiteralPath $manifestTemplate -Raw | ConvertFrom-Json

if ($manifest.target -ne 'x86_64-pc-windows-msvc') {
    throw "Unexpected runtime target in manifest template: $($manifest.target)"
}

$stageRoot = [IO.Path]::GetFullPath((Join-Path $runtimeRoot $manifest.target))
$runtimePrefix = $runtimeRoot.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
if (-not $stageRoot.StartsWith($runtimePrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to stage outside runtime/development: $stageRoot"
}

$downloadRoot = Join-Path $runtimeRoot '.downloads'
$extractRoot = Join-Path $runtimeRoot '.extract'
New-Item -ItemType Directory -Force -Path $runtimeRoot, $downloadRoot | Out-Null

function Get-PinnedArchive {
    param(
        [Parameter(Mandatory = $true)]$Component
    )

    $archivePath = Join-Path $downloadRoot $Component.archive
    $expectedHash = $Component.archive_sha256.ToLowerInvariant()

    if ((Test-Path -LiteralPath $archivePath) -and -not $ForceDownload) {
        $existingHash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($existingHash -eq $expectedHash) {
            Write-Host "Using verified cache: $($Component.archive)"
            return $archivePath
        }
        Write-Warning "Cached archive hash mismatch; downloading the pinned artifact again."
    }

    Write-Host "Downloading $($Component.source)"
    Invoke-WebRequest -UseBasicParsing -Uri $Component.source -OutFile $archivePath
    $actualHash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualHash -ne $expectedHash) {
        Remove-Item -LiteralPath $archivePath -Force
        throw "SHA256 mismatch for $($Component.archive). Expected $expectedHash, got $actualHash. The archive was deleted."
    }

    Write-Host "Verified SHA256 $actualHash"
    return $archivePath
}

$phpArchive = Get-PinnedArchive -Component $manifest.php
$caddyArchive = Get-PinnedArchive -Component $manifest.web_server
$mariaArchive = Get-PinnedArchive -Component $manifest.mariadb

if (Test-Path -LiteralPath $extractRoot) {
    Remove-Item -LiteralPath $extractRoot -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $extractRoot | Out-Null

$phpExtract = Join-Path $extractRoot 'php'
$caddyExtract = Join-Path $extractRoot 'caddy'
$mariaExtract = Join-Path $extractRoot 'mariadb'
Expand-Archive -LiteralPath $phpArchive -DestinationPath $phpExtract
Expand-Archive -LiteralPath $caddyArchive -DestinationPath $caddyExtract
Expand-Archive -LiteralPath $mariaArchive -DestinationPath $mariaExtract

$mariaSourceRoot = Join-Path $mariaExtract "mariadb-$($manifest.mariadb.version)-winx64"
if (-not (Test-Path -LiteralPath (Join-Path $phpExtract 'php.exe'))) {
    throw 'PHP archive did not contain php.exe at the expected root.'
}
if (-not (Test-Path -LiteralPath (Join-Path $phpExtract 'php-cgi.exe'))) {
    throw 'PHP archive did not contain php-cgi.exe at the expected root.'
}
if (-not (Test-Path -LiteralPath (Join-Path $phpExtract 'ext/php_opcache.dll'))) {
    throw 'PHP archive did not contain ext/php_opcache.dll.'
}
if (-not (Test-Path -LiteralPath (Join-Path $caddyExtract 'caddy.exe'))) {
    throw 'Caddy archive did not contain caddy.exe at the expected root.'
}
if (-not (Test-Path -LiteralPath (Join-Path $caddyExtract 'LICENSE'))) {
    throw 'Caddy archive did not contain LICENSE at the expected root.'
}
if (-not (Test-Path -LiteralPath (Join-Path $mariaSourceRoot 'bin/mariadbd.exe'))) {
    throw 'MariaDB archive did not contain the expected winx64 directory layout.'
}
if (-not (Test-Path -LiteralPath (Join-Path $mariaSourceRoot 'bin/mariadb.exe'))) {
    throw 'MariaDB archive did not contain mariadb.exe required for managed import/verification.'
}
if (-not (Test-Path -LiteralPath (Join-Path $mariaSourceRoot 'bin/mariadb-dump.exe'))) {
    throw 'MariaDB archive did not contain mariadb-dump.exe required for logical backup.'
}

if (Test-Path -LiteralPath $stageRoot) {
    Remove-Item -LiteralPath $stageRoot -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $stageRoot | Out-Null
Move-Item -LiteralPath $phpExtract -Destination (Join-Path $stageRoot 'php')
Move-Item -LiteralPath $caddyExtract -Destination (Join-Path $stageRoot 'caddy')
Move-Item -LiteralPath $mariaSourceRoot -Destination (Join-Path $stageRoot 'mariadb')
$phpExtDir = (Join-Path $stageRoot 'php/ext').Replace('\', '/')
$phpIni = (Get-Content -LiteralPath (Join-Path $templateRoot 'php.ini') -Raw).Replace('__COFFEEPOS_PHP_EXT_DIR__', $phpExtDir)
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[IO.File]::WriteAllText((Join-Path $stageRoot 'php/php.ini'), $phpIni, $utf8NoBom)
$phpExecutable = Join-Path $stageRoot 'php/php.exe'
$phpIniPath = Join-Path $stageRoot 'php/php.ini'
$phpModules = & $phpExecutable -c $phpIniPath -m
if ($LASTEXITCODE -ne 0 -or -not ($phpModules -match 'Zend OPcache')) {
    throw 'Staged PHP did not load Zend OPcache from the managed php.ini.'
}
Copy-Item -LiteralPath (Join-Path $templateRoot 'fixture') -Destination (Join-Path $stageRoot 'fixture') -Recurse
Copy-Item -LiteralPath $manifestTemplate -Destination (Join-Path $stageRoot 'manifest.json')

Remove-Item -LiteralPath $extractRoot -Recurse -Force

Write-Host "Staged development runtime: $stageRoot"
Write-Host "PHP:     $(Join-Path $stageRoot $manifest.php.executable)"
Write-Host "PHP CGI: $(Join-Path $stageRoot $manifest.php.cgi)"
Write-Host "Caddy:   $(Join-Path $stageRoot $manifest.web_server.executable)"
Write-Host "MariaDB: $(Join-Path $stageRoot $manifest.mariadb.server)"
Write-Host "MariaDB dump: $(Join-Path $stageRoot $manifest.mariadb.dump)"
