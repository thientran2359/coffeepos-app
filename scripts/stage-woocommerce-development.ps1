[CmdletBinding()]
param(
    [switch]$ForceDownload
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$scriptRoot = Split-Path -Parent $PSCommandPath
$projectRoot = Split-Path -Parent $scriptRoot
$runtimeRoot = [IO.Path]::GetFullPath((Join-Path $projectRoot 'runtime/development'))
$manifestTemplate = Join-Path $scriptRoot 'woocommerce-development/woocommerce-11.1.0.manifest.json'
$manifest = Get-Content -LiteralPath $manifestTemplate -Raw | ConvertFrom-Json

if ($manifest.schema_version -ne 1) {
    throw "Unsupported WooCommerce manifest schema: $($manifest.schema_version)"
}
if ($manifest.target -ne 'x86_64-pc-windows-msvc') {
    throw "Unexpected WooCommerce target in manifest template: $($manifest.target)"
}

$woocommerce = $manifest.woocommerce
$targetRoot = [IO.Path]::GetFullPath((Join-Path $runtimeRoot $manifest.target))
$stageRoot = [IO.Path]::GetFullPath((Join-Path $targetRoot 'woocommerce'))
$pluginRoot = [IO.Path]::GetFullPath((Join-Path $targetRoot $woocommerce.plugin_root))
$runtimePrefix = $runtimeRoot.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
$targetPrefix = $targetRoot.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar

if (-not $targetRoot.StartsWith($runtimePrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to stage outside runtime/development: $targetRoot"
}
if (-not $stageRoot.StartsWith($targetPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "WooCommerce stage root escapes the development target: $stageRoot"
}
if (-not $pluginRoot.StartsWith($targetPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "WooCommerce plugin_root escapes the development target: $($woocommerce.plugin_root)"
}
if ($woocommerce.plugin_root -ne 'woocommerce/woocommerce') {
    throw "Unexpected WooCommerce plugin_root in manifest template: $($woocommerce.plugin_root)"
}

$downloadRoot = Join-Path $runtimeRoot '.downloads'
$extractRoot = Join-Path $runtimeRoot '.extract-woocommerce'
New-Item -ItemType Directory -Force -Path $runtimeRoot, $downloadRoot, $targetRoot | Out-Null

function Test-PinnedArchive {
    param(
        [Parameter(Mandatory = $true)][string]$ArchivePath
    )

    if (-not (Test-Path -LiteralPath $ArchivePath)) {
        return $false
    }
    $sha256 = (Get-FileHash -LiteralPath $ArchivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    return $sha256 -eq $woocommerce.archive_sha256.ToLowerInvariant()
}

function Assert-PluginMetadata {
    param(
        [Parameter(Mandatory = $true)][string]$SourceRoot
    )

    foreach ($required in @('woocommerce.php', 'license.txt', 'readme.txt', 'includes', 'src', 'vendor')) {
        if (-not (Test-Path -LiteralPath (Join-Path $SourceRoot $required))) {
            throw "WooCommerce archive is missing required path: $required"
        }
    }

    $entryText = Get-Content -LiteralPath (Join-Path $SourceRoot 'woocommerce.php') -Raw
    $versionMatch = [regex]::Match($entryText, '(?m)^\s*\*\s*Version:\s*([^\r\n]+)$')
    $wordpressMatch = [regex]::Match($entryText, '(?m)^\s*\*\s*Requires at least:\s*([^\r\n]+)$')
    $phpMatch = [regex]::Match($entryText, '(?m)^\s*\*\s*Requires PHP:\s*([^\r\n]+)$')
    if (-not $versionMatch.Success -or $versionMatch.Groups[1].Value.Trim() -ne $woocommerce.version) {
        throw "WooCommerce plugin header version mismatch. Expected $($woocommerce.version)."
    }
    if (-not $wordpressMatch.Success -or $wordpressMatch.Groups[1].Value.Trim() -ne $manifest.compatibility.minimum_wordpress) {
        throw "WooCommerce WordPress requirement mismatch. Expected $($manifest.compatibility.minimum_wordpress)."
    }
    if (-not $phpMatch.Success -or $phpMatch.Groups[1].Value.Trim() -ne $manifest.compatibility.minimum_php) {
        throw "WooCommerce PHP requirement mismatch. Expected $($manifest.compatibility.minimum_php)."
    }

    $readmeText = Get-Content -LiteralPath (Join-Path $SourceRoot 'readme.txt') -Raw
    $testedMatch = [regex]::Match($readmeText, '(?m)^Tested up to:\s*([^\r\n]+)$')
    if (-not $testedMatch.Success -or $testedMatch.Groups[1].Value.Trim() -ne $manifest.compatibility.tested_wordpress) {
        throw "WooCommerce tested WordPress version mismatch. Expected $($manifest.compatibility.tested_wordpress)."
    }
    if ($readmeText -notmatch "(?m)^= $([regex]::Escape($woocommerce.version)) 2026-09-03 =$") {
        throw "WooCommerce readme does not contain the pinned $($woocommerce.version) changelog entry."
    }
}

$archivePath = Join-Path $downloadRoot $woocommerce.archive
if ($ForceDownload -or -not (Test-PinnedArchive -ArchivePath $archivePath)) {
    if ((Test-Path -LiteralPath $archivePath) -and -not $ForceDownload) {
        Write-Warning 'Cached WooCommerce archive checksum mismatch; downloading the pinned artifact again.'
    }
    Write-Host "Downloading $($woocommerce.source)"
    Invoke-WebRequest -UseBasicParsing -Uri $woocommerce.source -OutFile $archivePath
}

$actualSha256 = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actualSha256 -ne $woocommerce.archive_sha256.ToLowerInvariant()) {
    Remove-Item -LiteralPath $archivePath -Force
    throw "WooCommerce SHA256 mismatch. Expected $($woocommerce.archive_sha256), got $actualSha256. The archive was deleted."
}
Write-Host "Verified WooCommerce SHA256 $actualSha256"

if (Test-Path -LiteralPath $extractRoot) {
    Remove-Item -LiteralPath $extractRoot -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $extractRoot | Out-Null
try {
    Expand-Archive -LiteralPath $archivePath -DestinationPath $extractRoot
    $sourceRoot = Join-Path $extractRoot 'woocommerce'
    Assert-PluginMetadata -SourceRoot $sourceRoot

    if (Test-Path -LiteralPath $stageRoot) {
        Remove-Item -LiteralPath $stageRoot -Recurse -Force
    }
    New-Item -ItemType Directory -Force -Path $stageRoot | Out-Null
    Move-Item -LiteralPath $sourceRoot -Destination $pluginRoot
    Copy-Item -LiteralPath $manifestTemplate -Destination (Join-Path $targetRoot 'woocommerce-manifest.json')
} finally {
    if (Test-Path -LiteralPath $extractRoot) {
        Remove-Item -LiteralPath $extractRoot -Recurse -Force
    }
}

Assert-PluginMetadata -SourceRoot $pluginRoot
Write-Host "Staged WooCommerce $($woocommerce.version): $stageRoot"
Write-Host "Manifest: $(Join-Path $targetRoot 'woocommerce-manifest.json')"
Write-Host "Plugin root: $pluginRoot"
