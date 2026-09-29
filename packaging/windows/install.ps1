<#
.SYNOPSIS
    Installs Workbench from a release archive for the current user.

.DESCRIPTION
    Copies workbench.exe, with workbenchw.exe, conpty.dll, OpenConsole.exe and the documents
    beside it, into %LOCALAPPDATA%\Programs\Workbench (or -Prefix) and adds that folder to your
    PATH. No administrator rights are needed. Run it from the unpacked archive:

        powershell -ExecutionPolicy Bypass -File .\install.ps1

    Windows PowerShell runs no scripts under its default policy on Windows 10 and 11
    (Restricted), and under RemoteSigned it refuses an unsigned script that was downloaded (the
    Mark of the Web). -ExecutionPolicy Bypass applies to this run only. Unblocking the archive
    before unpacking it removes the mark (Unblock-File .\workbench-<version>-...-msvc.zip).

    A running Workbench keeps running: Windows cannot replace a running program, so its files
    are renamed aside (*.old, removed by the next install) and the new ones take their names.
    Restart Workbench to use the new version.

.PARAMETER Prefix
    The folder to install into. Default: %LOCALAPPDATA%\Programs\Workbench.
#>
[CmdletBinding()]
param(
    [string]$Prefix
)

# Windows PowerShell 5.1 runs this file: keep it ASCII (it has no byte order mark) and 5.1
# syntax.
Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

function Get-Reason($ErrorRecord) {
    $e = $ErrorRecord.Exception
    # .NET calls wrap the real error ("Exception calling ... with 2 argument(s)").
    if ($e.InnerException) { $e = $e.InnerException }
    $e.Message
}

function Fail([string]$Message) {
    [Console]::Error.WriteLine("install.ps1: $Message")
    exit 1
}

trap { Fail (Get-Reason $_) }

# What a release archive holds besides this script; only workbench.exe is required.
$Payload = @('workbench.exe', 'workbenchw.exe', 'conpty.dll', 'OpenConsole.exe',
    'LICENSE', 'README.md', 'CHANGELOG.md', 'THIRD_PARTY_NOTICES.md')

# Adds $Dir to the user's PATH (HKCU\Environment) unless it is there; $true when added.
function Add-UserPath([string]$Dir) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
    try {
        # The raw value: expanding it would turn entries like %USERPROFILE%\bin into fixed paths.
        $raw = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        foreach ($entry in $raw.Split(';')) {
            if ([Environment]::ExpandEnvironmentVariables($entry.Trim()).TrimEnd('\') -ieq $Dir) { return $false }
        }
        $kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
        if ($raw -and ($key.GetValueKind('Path') -eq [Microsoft.Win32.RegistryValueKind]::String)) {
            $kind = [Microsoft.Win32.RegistryValueKind]::String
        }
        $rest = $raw.TrimEnd(';')
        if ($rest) { $value = "$rest;$Dir" } else { $value = $Dir }
        $key.SetValue('Path', $value, $kind)
        return $true
    } finally {
        $key.Close()
    }
}

# Tells Explorer that the user environment changed, so what it starts from now on (a new
# terminal) gets the new PATH.
function Send-SettingChange {
    Add-Type -Namespace WorkbenchInstall -Name NativeMethods -MemberDefinition @'
[DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);
'@
    $result = [UIntPtr]::Zero
    # HWND_BROADCAST, WM_SETTINGCHANGE, SMTO_ABORTIFHUNG, at most 5 s per window.
    [void][WorkbenchInstall.NativeMethods]::SendMessageTimeout([IntPtr]0xffff, 0x1A, [UIntPtr]::Zero, 'Environment', 2, 5000, [ref]$result)
}

$here = $PSScriptRoot
if (-not $here) {
    Fail 'run install.ps1 as a file, from the unpacked archive'
}
if (-not (Test-Path -LiteralPath (Join-Path $here 'workbench.exe') -PathType Leaf)) {
    Fail "no workbench.exe next to this script ($here)"
}
if (-not [Environment]::Is64BitOperatingSystem) {
    Fail 'Workbench needs 64-bit Windows'
}
if ([Environment]::OSVersion.Version -lt [Version]'10.0.17763') {
    Write-Warning 'Workbench needs Windows 10 version 1809 or newer, or Windows 11.'
}

if (-not $Prefix) {
    if (-not $env:LOCALAPPDATA) { Fail 'LOCALAPPDATA is not set: pass -Prefix <folder>' }
    $Prefix = Join-Path $env:LOCALAPPDATA 'Programs\Workbench'
}
# Relative to the current location, like any PowerShell path; no trailing separator.
$Prefix = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Prefix)
if ($Prefix.Length -gt 3) { $Prefix = $Prefix.TrimEnd('\') }
if ($Prefix -ieq $here.TrimEnd('\')) {
    Fail "run install.ps1 from the unpacked archive, not from $Prefix"
}
[void][System.IO.Directory]::CreateDirectory($Prefix)

# Files an earlier install renamed aside while Workbench was running.
foreach ($file in @(Get-ChildItem -LiteralPath $Prefix -File -Force)) {
    foreach ($name in $Payload) {
        if ($file.Name -like "$name.*.old") {
            try { [System.IO.File]::Delete($file.FullName) } catch { }
            break
        }
    }
}

$names = @($Payload | Where-Object { Test-Path -LiteralPath (Join-Path $here $_) -PathType Leaf })
if ($names -notcontains 'conpty.dll' -or $names -notcontains 'OpenConsole.exe') {
    Write-Warning 'conpty.dll or OpenConsole.exe is missing: terminals will use the console host built into Windows.'
}

# Copy everything first, next to its destination, so a failed copy changes nothing.
foreach ($name in $names) {
    $new = Join-Path $Prefix "$name.new"
    [System.IO.File]::Copy((Join-Path $here $name), $new, $true)
    # A copy keeps the download's Mark of the Web, with which Windows stops the installed
    # programs with a SmartScreen warning when they start from Explorer or the Start menu.
    try { Unblock-File -LiteralPath $new } catch { }
}

# Then swap each file in. A running exe or a loaded DLL cannot be overwritten or deleted,
# but it can be renamed: the old file moves aside and is deleted unless something uses it.
$inUse = @()
foreach ($name in $names) {
    $dest = Join-Path $Prefix $name
    $aside = $null
    if (Test-Path -LiteralPath $dest) {
        $aside = '{0}.{1}.old' -f $dest, [guid]::NewGuid().ToString('N').Substring(0, 8)
        try {
            [System.IO.File]::Move($dest, $aside)
        } catch {
            Fail "cannot replace ${dest}: $(Get-Reason $_)"
        }
    }
    try {
        [System.IO.File]::Move("$dest.new", $dest)
    } catch {
        $reason = Get-Reason $_
        if ($aside) {
            try { [System.IO.File]::Move($aside, $dest) } catch { }
        }
        Fail "cannot install ${dest}: $reason"
    }
    if ($aside) {
        try { [System.IO.File]::Delete($aside) } catch { $inUse += $name }
    }
}

$exe = Join-Path $Prefix 'workbench.exe'
$version = & $exe --version
if ($LASTEXITCODE -ne 0) { Fail "$exe --version failed (exit code $LASTEXITCODE)" }
Write-Host "Installed $version to $exe"

if (Add-UserPath $Prefix) {
    try {
        Send-SettingChange
    } catch {
        Write-Warning "could not announce the new PATH ($(Get-Reason $_)): sign out and in again."
    }
    Write-Host "Added $Prefix to your PATH: open a new terminal to use it."
}
if (-not @($env:Path.Split(';') | Where-Object { $_.TrimEnd('\') -ieq $Prefix }).Length) {
    $env:Path = "$env:Path;$Prefix"
}

if ($inUse.Length) {
    Write-Host 'Workbench is running and keeps its old files until it restarts. Restart it to use the new version:'
    Write-Host '  stop the running workbench serve (Ctrl+C) and start it again, or, when it starts with'
    Write-Host '  your session, run  workbench service stop  and start it from the Start menu (Workbench).'
} else {
    Write-Host 'Start it with:  workbench serve --open'
    Write-Host 'Or with your session:  workbench service install --enable'
}
