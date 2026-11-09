#Requires -Version 7.0
<#
.SYNOPSIS
    Builds the RNotepad workspace crates and assembles a runnable bundle in output\.

.DESCRIPTION
Builds every workspace member with Cargo, then copies the application executable
and its runtime dependencies (third_party\Scintilla.dll and third_party\License.txt)
into output\.  The Scintilla DLL is a binary asset produced by
scripts\build-scintilla.ps1 and must exist before this script runs, because
cargo build stays pure Rust and never touches the C++ toolchain.

.PARAMETER Profile
    Cargo build profile: Release (default) or Debug.

.PARAMETER Clean
    Runs `cargo clean` for the selected profile before building.

.NOTES
    Every failure throws, so the script exits with a non-zero status.
#>
[CmdletBinding()]
param(
    [ValidateSet('Release', 'Debug')]
    [string]$Profile = 'Release',
    [switch]$Clean
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$PSNativeCommandUseErrorActionPreference = $false

$RepoRoot = Split-Path -Parent $PSScriptRoot
$ThirdPartyDir = Join-Path $RepoRoot 'third_party'
$OutputDir = Join-Path $RepoRoot 'output'
$AppExeName = 'rnotepad-app.exe'
$RuntimeAssets = @('Scintilla.dll', 'License.txt')
$CargoProfiles = @{
    Release = 'release'
    Debug   = 'dev'
}
$ProfileDirectories = @{
    Release = 'release'
    Debug   = 'debug'
}

function Resolve-TargetDirectory {
    $metadataLines = & cargo metadata --format-version 1 --no-deps 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "cargo metadata failed with exit code $LASTEXITCODE. Is the Rust toolchain installed?"
    }
    $metadata = ($metadataLines -join "`n") | ConvertFrom-Json
    $targetDirectory = $metadata.target_directory
    if (-not $targetDirectory -or -not (Test-Path -LiteralPath $targetDirectory -PathType Container)) {
        throw "Cargo target directory is missing: $targetDirectory"
    }
    return $targetDirectory
}

function Assert-RuntimeAssets {
    foreach ($asset in $RuntimeAssets) {
        $assetPath = Join-Path $ThirdPartyDir $asset
        if (-not (Test-Path -LiteralPath $assetPath -PathType Leaf)) {
            throw "Runtime asset is missing: $assetPath. Run scripts\build-scintilla.ps1 first."
        }
    }
}

$cargoProfile = $CargoProfiles[$Profile]
$profileDirectoryName = $ProfileDirectories[$Profile]

$targetDirectory = Resolve-TargetDirectory
Assert-RuntimeAssets

if ($Clean) {
    Write-Host "Cleaning the $cargoProfile profile ..."
    & cargo clean --profile $cargoProfile
    if ($LASTEXITCODE -ne 0) {
        throw "cargo clean failed with exit code $LASTEXITCODE."
    }
}

Write-Host "Building the workspace crates ($cargoProfile) ..."
& cargo build --workspace --profile $cargoProfile
$buildExitCode = $LASTEXITCODE
if ($buildExitCode -ne 0) {
    throw "cargo build failed with exit code $buildExitCode. See the build output above."
}

$profileDirectory = Join-Path $targetDirectory $profileDirectoryName
$builtExe = Join-Path $profileDirectory $AppExeName
if (-not (Test-Path -LiteralPath $builtExe -PathType Leaf)) {
    throw "Build reported success but the executable is missing: $builtExe"
}

New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null
Copy-Item -LiteralPath $builtExe -Destination (Join-Path $OutputDir $AppExeName) -Force
foreach ($asset in $RuntimeAssets) {
    Copy-Item -LiteralPath (Join-Path $ThirdPartyDir $asset) -Destination (Join-Path $OutputDir $asset) -Force
}

$exeSize = (Get-Item -LiteralPath (Join-Path $OutputDir $AppExeName)).Length
Write-Host ("Output directory: {0}" -f $OutputDir)
Write-Host ("Executable:       {0} ({1:N1} KB)" -f $AppExeName, ($exeSize / 1KB))
foreach ($asset in $RuntimeAssets) {
    $assetSize = (Get-Item -LiteralPath (Join-Path $OutputDir $asset)).Length
    Write-Host ("Runtime asset:    {0} ({1:N1} KB)" -f $asset, ($assetSize / 1KB))
}
