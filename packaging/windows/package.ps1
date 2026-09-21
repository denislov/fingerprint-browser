<#
.SYNOPSIS
    Builds the Windows installer.

.DESCRIPTION
    The release binary, then Inno Setup over packaging\windows\fingerprint-browser.iss,
    with the version read out of it: the installer's name, the version in
    Add/Remove Programs and the tag a release is cut from all come from the one
    number in Cargo.toml.

    Checks the executable's own resource section first. The icon and the product
    version in it are written by crates\app\build.rs, which degrades to a warning
    when a machine has no resource compiler - so an installer built on such a
    machine would look right and install an executable with no icon, which is
    exactly the kind of thing a release script is for finding.

.EXAMPLE
    pwsh -File packaging\windows\package.ps1
#>
[CmdletBinding()]
param(
    # The version to build the installer as. The release workflow passes the one
    # it read out of `scripts/version.sh`, which is the only place the number is
    # parsed; without it the version the executable reports is used, so a local
    # build does not need an argument.
    [string]$Version,

    # Skip cargo build and package what is already in target\release.
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

$root = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
Set-Location $root

if (-not $SkipBuild) {
    cargo build --release -p app
    if ($LASTEXITCODE -ne 0) { throw "cargo build --release failed" }
}

$exe = Join-Path $root "target\release\fingerprint-browser.exe"
if (-not (Test-Path $exe)) { throw "$exe is not there: build it first" }

# The resource section, which is where the version comes from when the caller did
# not name one. Both halves are checked: the icon and the version are separate
# resources, and the icon is the one that goes missing without an error.
$info = (Get-Item $exe).VersionInfo
if ([string]::IsNullOrWhiteSpace($Version)) {
    $Version = $info.ProductVersion
}
if ($info.ProductVersion -ne $Version) {
    throw "the executable says '$($info.ProductVersion)' and this build is '$Version' - a stale target\release, or the wrong -Version"
}
if ($info.ProductName -ne "Fingerprint Browser") {
    throw "the executable has no product resource: the resource compiler did not run, so the icon is missing too"
}
Write-Host "resource section: $($info.ProductName) $($info.ProductVersion)"

# Inno Setup, wherever the runner put it. GitHub's Windows image has it; a
# machine that does not is told what to install rather than failing obscurely.
$iscc = @(
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) {
    $found = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    if ($found) { $iscc = $found.Source }
}
if (-not $iscc) {
    throw "Inno Setup 6 was not found. Install it (choco install innosetup) and run this again."
}

New-Item -ItemType Directory -Force (Join-Path $root "dist") | Out-Null

& $iscc "/DAppVersion=$Version" "/DSourceRoot=$root" `
    (Join-Path $root "packaging\windows\fingerprint-browser.iss")
if ($LASTEXITCODE -ne 0) { throw "ISCC failed" }

$installer = Join-Path $root "dist\fingerprint-browser-$Version-windows-x86_64-setup.exe"
if (-not (Test-Path $installer)) { throw "$installer was not produced" }

# The checksum file, in the same one-line form the Linux archive gets.
$hash = (Get-FileHash -Algorithm SHA256 $installer).Hash.ToLower()
"$hash  $(Split-Path -Leaf $installer)" |
    Set-Content -Path "$installer.sha256" -Encoding ascii

Write-Host "dist\$(Split-Path -Leaf $installer)"
