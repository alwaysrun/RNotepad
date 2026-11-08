#Requires -Version 7.0
<#
.SYNOPSIS
    Builds the vendored Scintilla 5.6.9 sources into third_party\Scintilla.dll.

.DESCRIPTION
Locates the newest Visual Studio installation that provides the x64 C++
toolset, imports its environment through vcvars64.bat, and runs
`nmake -f scintilla.mak` in vendor\scintilla-5.6.9\win32.  The resulting
bin\Scintilla.dll and License.txt are copied into third_party\.

.PARAMETER Clean
    Runs `nmake -f scintilla.mak clean` before the build.

.NOTES
    Every failure throws, so the script exits with a non-zero status.
#>
[CmdletBinding()]
param(
    [switch]$Clean
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$PSNativeCommandUseErrorActionPreference = $false

$RepoRoot = Split-Path -Parent $PSScriptRoot
$VendorRoot = Join-Path $RepoRoot 'vendor\scintilla-5.6.9'
$Win32Dir = Join-Path $VendorRoot 'win32'
$Makefile = 'scintilla.mak'
$BuiltDll = Join-Path $VendorRoot 'bin\Scintilla.dll'
$BuiltLicense = Join-Path $VendorRoot 'License.txt'
$OutDir = Join-Path $RepoRoot 'third_party'
$OutDll = Join-Path $OutDir 'Scintilla.dll'
$OutLicense = Join-Path $OutDir 'License.txt'

function Find-VsWhere {
    $candidates = @()
    if (${env:ProgramFiles(x86)}) {
        $candidates += Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    }
    if ($env:ProgramFiles) {
        $candidates += Join-Path $env:ProgramFiles 'Microsoft Visual Studio\Installer\vswhere.exe'
    }
    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return $candidate
        }
    }
    throw 'vswhere.exe was not found. Install Visual Studio with the C++ toolset.'
}

function Resolve-VcVars64 {
    param([Parameter(Mandatory)][string]$VsWhere)

    $vcvars = @(& $VsWhere -latest -products '*' `
            -requires 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64' `
            -find 'VC\Auxiliary\Build\vcvars64.bat') | Select-Object -First 1
    if (-not $vcvars) {
        # Older vswhere builds may not support -find; fall back to the install path.
        $installPath = @(& $VsWhere -latest -products '*' `
                -requires 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64' `
                -property installationPath) | Select-Object -First 1
        if ($installPath) {
            $vcvars = Join-Path $installPath 'VC\Auxiliary\Build\vcvars64.bat'
        }
    }
    if (-not $vcvars -or -not (Test-Path -LiteralPath $vcvars -PathType Leaf)) {
        throw 'No Visual Studio installation with the x64 C++ toolset (vcvars64.bat) was found.'
    }
    return $vcvars
}

function Resolve-MsvcTool {
    param(
        [Parameter(Mandatory)][string]$VsInstallRoot,
        [Parameter(Mandatory)][string]$RelativePath
    )

    $toolsets = Get-ChildItem -LiteralPath (Join-Path $VsInstallRoot 'VC\Tools\MSVC') -Directory |
        Sort-Object { [version]$_.Name } -Descending
    if (-not $toolsets) {
        throw "No MSVC toolset found under $VsInstallRoot\VC\Tools\MSVC."
    }
    $toolPath = Join-Path $toolsets[0].FullName $RelativePath
    if (-not (Test-Path -LiteralPath $toolPath -PathType Leaf)) {
        throw "Missing compiler tool: $toolPath"
    }
    return $toolPath
}

if (-not (Test-Path -LiteralPath $Win32Dir -PathType Container)) {
    throw "Scintilla win32 directory not found: $Win32Dir"
}
if (-not (Test-Path -LiteralPath (Join-Path $Win32Dir $Makefile) -PathType Leaf)) {
    throw "Makefile not found: $(Join-Path $Win32Dir $Makefile)"
}

$vsWhere = Find-VsWhere
$vcvars = Resolve-VcVars64 -VsWhere $vsWhere

$vcDir = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $vcvars))
$vsInstallRoot = Split-Path -Parent $vcDir
$clPath = Resolve-MsvcTool -VsInstallRoot $vsInstallRoot -RelativePath 'bin\Hostx64\x64\cl.exe'
$nmakePath = Resolve-MsvcTool -VsInstallRoot $vsInstallRoot -RelativePath 'bin\Hostx64\x64\nmake.exe'

Write-Host "vswhere:    $vsWhere"
Write-Host "vcvars64:   $vcvars"
Write-Host "compiler:   $clPath"
Write-Host "nmake:      $nmakePath"

$batchPath = Join-Path ([System.IO.Path]::GetTempPath()) "rnotepad-build-scintilla-$([Guid]::NewGuid()).cmd"
$batchLines = [System.Collections.Generic.List[string]]::new()
$batchLines.Add('@echo off')
$batchLines.Add("call `"$vcvars`"")
$batchLines.Add('if errorlevel 1 exit /b 1')
$batchLines.Add("cd /d `"$Win32Dir`"")
$batchLines.Add('if errorlevel 1 exit /b 1')
if ($Clean) {
    $batchLines.Add("nmake -f $Makefile clean")
    $batchLines.Add('if errorlevel 1 exit /b 1')
}
$batchLines.Add("nmake -f $Makefile")
$batchLines.Add('exit /b %errorlevel%')
[System.IO.File]::WriteAllText($batchPath, ($batchLines -join "`r`n"), [System.Text.UTF8Encoding]::new($false))

try {
    Write-Host "Building Scintilla in $Win32Dir ..."
    $log = & cmd.exe /d /c $batchPath 2>&1
    $exitCode = $LASTEXITCODE
    foreach ($entry in $log) {
        Write-Host $entry
    }
    if ($exitCode -ne 0) {
        throw "nmake failed with exit code $exitCode. See the build output above."
    }
}
finally {
    Remove-Item -LiteralPath $batchPath -Force -ErrorAction SilentlyContinue
}

if (-not (Test-Path -LiteralPath $BuiltDll -PathType Leaf)) {
    throw "Build reported success but the DLL is missing: $BuiltDll"
}
if (-not (Test-Path -LiteralPath $BuiltLicense -PathType Leaf)) {
    throw "Scintilla license file is missing: $BuiltLicense"
}

New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
Copy-Item -LiteralPath $BuiltDll -Destination $OutDll -Force
Copy-Item -LiteralPath $BuiltLicense -Destination $OutLicense -Force

$dllSize = (Get-Item -LiteralPath $OutDll).Length
Write-Host ("Output DLL: {0} ({1:N1} KB)" -f $OutDll, ($dllSize / 1KB))
Write-Host ("License:    {0}" -f $OutLicense)
