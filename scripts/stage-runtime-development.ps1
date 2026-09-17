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
$mariaArchive = Get-PinnedArchive -Component $manifest.mariadb

if (Test-Path -LiteralPath $extractRoot) {
    Remove-Item -LiteralPath $extractRoot -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $extractRoot | Out-Null

$phpExtract = Join-Path $extractRoot 'php'
$mariaExtract = Join-Path $extractRoot 'mariadb'
Expand-Archive -LiteralPath $phpArchive -DestinationPath $phpExtract
Expand-Archive -LiteralPath $mariaArchive -DestinationPath $mariaExtract

$mariaSourceRoot = Join-Path $mariaExtract "mariadb-$($manifest.mariadb.version)-winx64"
if (-not (Test-Path -LiteralPath (Join-Path $phpExtract 'php.exe'))) {
    throw 'PHP archive did not contain php.exe at the expected root.'
}
if (-not (Test-Path -LiteralPath (Join-Path $mariaSourceRoot 'bin/mariadbd.exe'))) {
    throw 'MariaDB archive did not contain the expected winx64 directory layout.'
}

if (Test-Path -LiteralPath $stageRoot) {
    Remove-Item -LiteralPath $stageRoot -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $stageRoot | Out-Null
Move-Item -LiteralPath $phpExtract -Destination (Join-Path $stageRoot 'php')
Move-Item -LiteralPath $mariaSourceRoot -Destination (Join-Path $stageRoot 'mariadb')
$phpExtDir = (Join-Path $stageRoot 'php/ext').Replace('\', '/')
$phpIni = (Get-Content -LiteralPath (Join-Path $templateRoot 'php.ini') -Raw).Replace('__COFFEEPOS_PHP_EXT_DIR__', $phpExtDir)
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[IO.File]::WriteAllText((Join-Path $stageRoot 'php/php.ini'), $phpIni, $utf8NoBom)
Copy-Item -LiteralPath (Join-Path $templateRoot 'fixture') -Destination (Join-Path $stageRoot 'fixture') -Recurse
Copy-Item -LiteralPath $manifestTemplate -Destination (Join-Path $stageRoot 'manifest.json')

Remove-Item -LiteralPath $extractRoot -Recurse -Force

Write-Host "Staged development runtime: $stageRoot"
Write-Host "PHP:     $(Join-Path $stageRoot $manifest.php.executable)"
Write-Host "MariaDB: $(Join-Path $stageRoot $manifest.mariadb.server)"
