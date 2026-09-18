[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$scriptRoot = Split-Path -Parent $PSCommandPath
$projectRoot = Split-Path -Parent $scriptRoot
$runtimeRoot = [IO.Path]::GetFullPath((Join-Path $projectRoot 'runtime/development'))
$artifactRoot = [IO.Path]::GetFullPath((Join-Path $scriptRoot 'coffeepos-development'))
$manifestTemplate = Join-Path $artifactRoot 'coffeepos-1.0.0.manifest.json'
$manifest = Get-Content -LiteralPath $manifestTemplate -Raw | ConvertFrom-Json

if ($manifest.schema_version -ne 1) {
    throw "Unsupported CoffeePOS manifest schema: $($manifest.schema_version)"
}
if ($manifest.target -ne 'x86_64-pc-windows-msvc') {
    throw "Unexpected CoffeePOS target in manifest template: $($manifest.target)"
}

$coffeepos = $manifest.coffeepos
$targetRoot = [IO.Path]::GetFullPath((Join-Path $runtimeRoot $manifest.target))
$stageRoot = [IO.Path]::GetFullPath((Join-Path $targetRoot 'coffeepos'))
$pluginRoot = [IO.Path]::GetFullPath((Join-Path $targetRoot $coffeepos.plugin_root))
$archivePath = [IO.Path]::GetFullPath((Join-Path $artifactRoot $coffeepos.archive))
$extractRoot = [IO.Path]::GetFullPath((Join-Path $runtimeRoot '.extract-coffeepos'))
$runtimePrefix = $runtimeRoot.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
$targetPrefix = $targetRoot.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
$artifactPrefix = $artifactRoot.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar

function Assert-NotReparsePoint {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Label
    )

    if (-not (Test-Path -LiteralPath $Path)) {
        return
    }

    $item = Get-Item -LiteralPath $Path -Force
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Refusing to use $Label through a reparse point: $Path"
    }
}

if (-not $targetRoot.StartsWith($runtimePrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to stage outside runtime/development: $targetRoot"
}
if (-not $stageRoot.StartsWith($targetPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "CoffeePOS stage root escapes the development target: $stageRoot"
}
if ($stageRoot -ne [IO.Path]::GetFullPath((Join-Path $targetRoot 'coffeepos'))) {
    throw "Unexpected CoffeePOS stage root: $stageRoot"
}
if (-not $pluginRoot.StartsWith($targetPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "CoffeePOS plugin_root escapes the development target: $($coffeepos.plugin_root)"
}
if (-not $extractRoot.StartsWith($runtimePrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "CoffeePOS extract root escapes runtime/development: $extractRoot"
}
if ($extractRoot -ne [IO.Path]::GetFullPath((Join-Path $runtimeRoot '.extract-coffeepos'))) {
    throw "Unexpected CoffeePOS extract root: $extractRoot"
}
if (-not $archivePath.StartsWith($artifactPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "CoffeePOS archive escapes the checked-in artifact directory: $($coffeepos.archive)"
}
if ($coffeepos.plugin_root -ne 'coffeepos/coffeepos') {
    throw "Unexpected CoffeePOS plugin_root in manifest template: $($coffeepos.plugin_root)"
}
if (-not (Test-Path -LiteralPath $archivePath -PathType Leaf)) {
    throw "Pinned CoffeePOS archive is missing: $archivePath"
}

Assert-NotReparsePoint -Path $runtimeRoot -Label 'runtime root'
Assert-NotReparsePoint -Path $targetRoot -Label 'development target'
Assert-NotReparsePoint -Path $artifactRoot -Label 'artifact root'
Assert-NotReparsePoint -Path $stageRoot -Label 'CoffeePOS stage root'
Assert-NotReparsePoint -Path $extractRoot -Label 'CoffeePOS extract root'

function Assert-PluginMetadata {
    param(
        [Parameter(Mandatory = $true)][string]$SourceRoot
    )

    foreach ($required in @('coffeepos.php', 'readme.txt', 'LICENSE', 'assets', 'includes', 'languages', 'templates', 'vendor', 'vendor/autoload.php')) {
        if (-not (Test-Path -LiteralPath (Join-Path $SourceRoot $required))) {
            throw "CoffeePOS artifact is missing required path: $required"
        }
    }

    $entryText = Get-Content -LiteralPath (Join-Path $SourceRoot 'coffeepos.php') -Raw
    $versionMatch = [regex]::Match($entryText, '(?m)^\s*\*\s*Version:\s*([^\r\n]+)\r?$')
    $wordpressMatch = [regex]::Match($entryText, '(?m)^\s*\*\s*Requires at least:\s*([^\r\n]+)\r?$')
    $phpMatch = [regex]::Match($entryText, '(?m)^\s*\*\s*Requires PHP:\s*([^\r\n]+)\r?$')
    $requiresPluginsMatch = [regex]::Match($entryText, '(?m)^\s*\*\s*Requires Plugins:\s*([^\r\n]+)\r?$')
    $constantMatch = [regex]::Match($entryText, "define\('COFFEEPOS_VERSION',\s*'([^']+)'\)")

    if (-not $versionMatch.Success -or $versionMatch.Groups[1].Value.Trim() -ne $coffeepos.version) {
        throw "CoffeePOS plugin header version mismatch. Expected $($coffeepos.version)."
    }
    if (-not $constantMatch.Success -or $constantMatch.Groups[1].Value.Trim() -ne $coffeepos.version) {
        throw "CoffeePOS version constant mismatch. Expected $($coffeepos.version)."
    }
    if (-not $wordpressMatch.Success -or $wordpressMatch.Groups[1].Value.Trim() -ne $manifest.compatibility.minimum_wordpress) {
        throw "CoffeePOS WordPress requirement mismatch. Expected $($manifest.compatibility.minimum_wordpress)."
    }
    if (-not $phpMatch.Success -or $phpMatch.Groups[1].Value.Trim() -ne $manifest.compatibility.minimum_php) {
        throw "CoffeePOS PHP requirement mismatch. Expected $($manifest.compatibility.minimum_php)."
    }
    if (-not $requiresPluginsMatch.Success) {
        throw 'CoffeePOS plugin header is missing Requires Plugins metadata.'
    }
    $requiredPlugins = @($requiresPluginsMatch.Groups[1].Value.Split(',') | ForEach-Object { $_.Trim().ToLowerInvariant() })
    if ($requiredPlugins -notcontains $coffeepos.requires_plugin.ToLowerInvariant()) {
        throw "CoffeePOS dependency mismatch. Expected Requires Plugins to include $($coffeepos.requires_plugin)."
    }

    $readmeText = Get-Content -LiteralPath (Join-Path $SourceRoot 'readme.txt') -Raw
    $stableMatch = [regex]::Match($readmeText, '(?m)^Stable tag:\s*([^\r\n]+)\r?$')
    $testedMatch = [regex]::Match($readmeText, '(?m)^Tested up to:\s*([^\r\n]+)\r?$')
    if (-not $stableMatch.Success -or $stableMatch.Groups[1].Value.Trim() -ne $coffeepos.version) {
        throw "CoffeePOS Stable tag mismatch. Expected $($coffeepos.version)."
    }
    if (-not $testedMatch.Success -or $testedMatch.Groups[1].Value.Trim() -ne $manifest.compatibility.tested_wordpress) {
        throw "CoffeePOS tested WordPress version mismatch. Expected $($manifest.compatibility.tested_wordpress)."
    }
}

function Assert-ArchiveEntries {
    param(
        [Parameter(Mandatory = $true)][string]$Path
    )

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [IO.Compression.ZipFile]::OpenRead($Path)
    try {
        if ($zip.Entries.Count -eq 0) {
            throw 'CoffeePOS archive is empty.'
        }

        foreach ($entry in $zip.Entries) {
            $name = $entry.FullName.Replace([char]92, [char]47)
            if ([string]::IsNullOrWhiteSpace($name)) {
                continue
            }
            if ($name.StartsWith('/') -or $name -match '(^|/)\.\.(/|$)') {
                throw "CoffeePOS archive contains unsafe path: $name"
            }
            if (-not $name.StartsWith('coffeepos/', [StringComparison]::Ordinal)) {
                throw "CoffeePOS archive entry is outside the expected coffeepos root: $name"
            }
        }
    } finally {
        $zip.Dispose()
    }
}

$actualSha256 = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actualSha256 -ne $coffeepos.archive_sha256.ToLowerInvariant()) {
    throw "CoffeePOS SHA256 mismatch. Expected $($coffeepos.archive_sha256), got $actualSha256."
}
Write-Host "Verified CoffeePOS SHA256 $actualSha256"
Assert-ArchiveEntries -Path $archivePath

New-Item -ItemType Directory -Force -Path $runtimeRoot, $targetRoot | Out-Null
if (Test-Path -LiteralPath $extractRoot) {
    Assert-NotReparsePoint -Path $extractRoot -Label 'CoffeePOS extract root'
    Remove-Item -LiteralPath $extractRoot -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $extractRoot | Out-Null

try {
    Expand-Archive -LiteralPath $archivePath -DestinationPath $extractRoot
    $sourceRoot = Join-Path $extractRoot 'coffeepos'
    Assert-PluginMetadata -SourceRoot $sourceRoot

    if (Test-Path -LiteralPath $stageRoot) {
        Assert-NotReparsePoint -Path $stageRoot -Label 'CoffeePOS stage root'
        Remove-Item -LiteralPath $stageRoot -Recurse -Force
    }
    New-Item -ItemType Directory -Force -Path $stageRoot | Out-Null
    Move-Item -LiteralPath $sourceRoot -Destination $pluginRoot
    Copy-Item -LiteralPath $manifestTemplate -Destination (Join-Path $targetRoot 'coffeepos-manifest.json')
} finally {
    if (Test-Path -LiteralPath $extractRoot) {
        Assert-NotReparsePoint -Path $extractRoot -Label 'CoffeePOS extract root'
        Remove-Item -LiteralPath $extractRoot -Recurse -Force
    }
}

Assert-PluginMetadata -SourceRoot $pluginRoot
Write-Host "Staged CoffeePOS $($coffeepos.version): $stageRoot"
Write-Host "Manifest: $(Join-Path $targetRoot 'coffeepos-manifest.json')"
Write-Host "Plugin root: $pluginRoot"
