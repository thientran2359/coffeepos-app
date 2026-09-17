[CmdletBinding()]
param(
    [switch]$ForceDownload
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$scriptRoot = Split-Path -Parent $PSCommandPath
$projectRoot = Split-Path -Parent $scriptRoot
$runtimeRoot = [IO.Path]::GetFullPath((Join-Path $projectRoot 'runtime/development'))
$manifestTemplate = Join-Path $scriptRoot 'wordpress-development/wordpress-7.1.manifest.json'
$manifest = Get-Content -LiteralPath $manifestTemplate -Raw | ConvertFrom-Json

if ($manifest.target -ne 'x86_64-pc-windows-msvc') {
    throw "Unexpected WordPress target in manifest template: $($manifest.target)"
}

$wordpress = $manifest.wordpress
$targetRoot = [IO.Path]::GetFullPath((Join-Path $runtimeRoot $manifest.target))
$stageRoot = [IO.Path]::GetFullPath((Join-Path $targetRoot 'wordpress'))
$coreRoot = [IO.Path]::GetFullPath((Join-Path $targetRoot $wordpress.core_root))
$runtimePrefix = $runtimeRoot.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
if (-not $targetRoot.StartsWith($runtimePrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to stage outside runtime/development: $targetRoot"
}
$targetPrefix = $targetRoot.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
if (-not $coreRoot.StartsWith($targetPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "WordPress core_root escapes the development target: $($wordpress.core_root)"
}
if ($wordpress.core_root -ne 'wordpress/wordpress') {
    throw "Unexpected WordPress core_root in manifest template: $($wordpress.core_root)"
}

$downloadRoot = Join-Path $runtimeRoot '.downloads'
$extractRoot = Join-Path $runtimeRoot '.extract-wordpress'
New-Item -ItemType Directory -Force -Path $runtimeRoot, $downloadRoot | Out-Null

function Test-PinnedArchive {
    param(
        [Parameter(Mandatory = $true)][string]$ArchivePath
    )

    if (-not (Test-Path -LiteralPath $ArchivePath)) {
        return $false
    }

    $sha256 = (Get-FileHash -LiteralPath $ArchivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    $sha1 = (Get-FileHash -LiteralPath $ArchivePath -Algorithm SHA1).Hash.ToLowerInvariant()
    return $sha256 -eq $wordpress.archive_sha256.ToLowerInvariant() -and
        $sha1 -eq $wordpress.official_sha1.ToLowerInvariant()
}

$archivePath = Join-Path $downloadRoot $wordpress.archive
if ($ForceDownload -or -not (Test-PinnedArchive -ArchivePath $archivePath)) {
    if ((Test-Path -LiteralPath $archivePath) -and -not $ForceDownload) {
        Write-Warning 'Cached WordPress archive checksum mismatch; downloading the pinned artifact again.'
    }
    Write-Host "Downloading $($wordpress.source)"
    Invoke-WebRequest -UseBasicParsing -Uri $wordpress.source -OutFile $archivePath
}

$actualSha256 = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
$actualSha1 = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA1).Hash.ToLowerInvariant()
if ($actualSha256 -ne $wordpress.archive_sha256.ToLowerInvariant()) {
    Remove-Item -LiteralPath $archivePath -Force
    throw "WordPress SHA256 mismatch. Expected $($wordpress.archive_sha256), got $actualSha256. The archive was deleted."
}
if ($actualSha1 -ne $wordpress.official_sha1.ToLowerInvariant()) {
    Remove-Item -LiteralPath $archivePath -Force
    throw "WordPress SHA1 mismatch. Expected official SHA1 $($wordpress.official_sha1), got $actualSha1. The archive was deleted."
}
Write-Host "Verified WordPress SHA256 $actualSha256"
Write-Host "Verified official WordPress SHA1 $actualSha1"

if (Test-Path -LiteralPath $extractRoot) {
    Remove-Item -LiteralPath $extractRoot -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $extractRoot | Out-Null
Expand-Archive -LiteralPath $archivePath -DestinationPath $extractRoot

$sourceRoot = Join-Path $extractRoot 'wordpress'
$versionFile = Join-Path $sourceRoot 'wp-includes/version.php'
foreach ($required in @('index.php', 'license.txt', 'readme.html', 'wp-admin', 'wp-content', 'wp-includes', 'wp-includes/version.php')) {
    if (-not (Test-Path -LiteralPath (Join-Path $sourceRoot $required))) {
        throw "WordPress archive is missing required path: $required"
    }
}

$versionText = Get-Content -LiteralPath $versionFile -Raw
$versionMatch = [regex]::Match($versionText, '\$wp_version\s*=\s*''([^'']+)'';')
if (-not $versionMatch.Success) {
    throw 'Could not read $wp_version from the staged WordPress core.'
}
$actualVersion = $versionMatch.Groups[1].Value
if ($actualVersion -ne $wordpress.version) {
    throw "WordPress core version mismatch. Expected $($wordpress.version), got $actualVersion."
}

if (Test-Path -LiteralPath $stageRoot) {
    Remove-Item -LiteralPath $stageRoot -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $stageRoot | Out-Null
Move-Item -LiteralPath $sourceRoot -Destination $coreRoot
$staleNestedManifest = Join-Path $stageRoot 'wordpress-manifest.json'
if (Test-Path -LiteralPath $staleNestedManifest) {
    Remove-Item -LiteralPath $staleNestedManifest -Force
}
Copy-Item -LiteralPath $manifestTemplate -Destination (Join-Path $targetRoot 'wordpress-manifest.json')
Remove-Item -LiteralPath $extractRoot -Recurse -Force

Write-Host "Staged WordPress ${actualVersion}: $stageRoot"
Write-Host "Manifest: $(Join-Path $targetRoot 'wordpress-manifest.json')"
Write-Host "Core root: $coreRoot"
