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

    A folder it creates admits only you, SYSTEM and Administrators, so nobody else can replace
    the programs your PATH starts. It warns when an existing folder lets others change it.

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
    'LICENSE', 'README.md', 'CHANGELOG.md', 'THIRD_PARTY_NOTICES.md', 'CONPTY_NOTICE.md')

# The accounts an install folder may let change it: you, SYSTEM, Administrators, and the
# CREATOR OWNER and TrustedInstaller entries Windows puts on its own folders.
$Me = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
$Trusted = @($Me.Value, 'S-1-5-18', 'S-1-5-32-544', 'S-1-3-0',
    'S-1-5-80-956008885-3425870976-1789322516-4210395290-2271478464')
# Rights that let an account change a folder's files: create files or folders (append),
# delete, change permissions, take ownership, and the generic write and all.
$WriteRights = 0x2 -bor 0x4 -bor 0x40 -bor 0x10000 -bor 0x40000 -bor 0x80000 -bor 0x10000000 -bor 0x40000000

# A folder's access list. Windows PowerShell's .NET reads and writes it itself, so a
# Get-Acl that fails to load (a Windows PowerShell started from PowerShell 7 can find the
# latter's modules first) does not matter; PowerShell 7's .NET has no such methods.
function Get-FolderAcl([string]$Dir) {
    if ($PSVersionTable.PSEdition -eq 'Desktop') { return [System.IO.Directory]::GetAccessControl($Dir) }
    Get-Acl -LiteralPath $Dir
}

function Set-FolderAcl([string]$Dir, $Acl) {
    if ($PSVersionTable.PSEdition -eq 'Desktop') {
        [System.IO.Directory]::SetAccessControl($Dir, $Acl)
    } else {
        Set-Acl -LiteralPath $Dir -AclObject $Acl
    }
}

# Gives the folder install.ps1 just created an access list of its own: full control for you,
# SYSTEM and Administrators (what folders in your profile inherit), passed on to what it holds.
function Protect-Folder([string]$Dir) {
    $acl = Get-FolderAcl $Dir
    $acl.SetAccessRuleProtection($true, $false)
    $inherit = [System.Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit'
    foreach ($sid in @($Me.Value, 'S-1-5-18', 'S-1-5-32-544')) {
        $acl.AddAccessRule([System.Security.AccessControl.FileSystemAccessRule]::new(
            [System.Security.Principal.SecurityIdentifier]::new($sid),
            [System.Security.AccessControl.FileSystemRights]::FullControl, $inherit,
            [System.Security.AccessControl.PropagationFlags]::None,
            [System.Security.AccessControl.AccessControlType]::Allow))
    }
    Set-FolderAcl $Dir $acl
    # Something another account put there before the access list applied would stay theirs.
    if (@(Get-ChildItem -LiteralPath $Dir -Force).Length) {
        Fail "$Dir changed while it was being created: remove it and run install.ps1 again"
    }
}

# The other accounts that can change what is in $Dir: its owner, or an allow entry with one of
# $WriteRights.
function Get-OtherWriters([string]$Dir) {
    $sidType = [System.Security.Principal.SecurityIdentifier]
    $acl = Get-FolderAcl $Dir
    $sids = @()
    $owner = $acl.GetOwner($sidType)
    if ($owner -and $Trusted -notcontains $owner.Value) { $sids += $owner }
    foreach ($rule in $acl.GetAccessRules($true, $true, $sidType)) {
        if ($rule.AccessControlType -ne [System.Security.AccessControl.AccessControlType]::Allow) { continue }
        if ($Trusted -contains $rule.IdentityReference.Value) { continue }
        if ([int]$rule.FileSystemRights -band $WriteRights) { $sids += $rule.IdentityReference }
    }
    $names = @()
    foreach ($sid in $sids) {
        try { $name = $sid.Translate([System.Security.Principal.NTAccount]).Value } catch { $name = $sid.Value }
        if ($names -notcontains $name) { $names += $name }
    }
    $names
}

# Renames $From to $To. Another program can hold either file for a moment without letting it
# be renamed (an antivirus scanning the new file, Explorer's preview), so a sharing violation
# is retried for about a second.
function Move-File([string]$From, [string]$To) {
    $attempt = 0
    while ($true) {
        $attempt++
        try {
            [System.IO.File]::Move($From, $To)
            return
        } catch {
            $e = $_.Exception
            if ($e.InnerException) { $e = $e.InnerException }
            $busy = ($e -is [System.UnauthorizedAccessException]) -or (($e -is [System.IO.IOException]) -and
                ($e -isnot [System.IO.FileNotFoundException]) -and ($e -isnot [System.IO.DirectoryNotFoundException]))
            if ($attempt -ge 10 -or -not $busy) { throw }
            Start-Sleep -Milliseconds 100
        }
    }
}

# Removes the copies staged next to their destinations (<name>.new).
function Remove-Staged {
    foreach ($name in $Payload) {
        try { [System.IO.File]::Delete((Join-Path $Prefix "$name.new")) } catch { }
    }
}

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
# PATH separates folders with ; and expands %NAME%.
if ($Prefix.Contains(';') -or $Prefix.Contains('%')) {
    Fail "the install folder goes on PATH, so its name cannot contain ; or %: $Prefix"
}
if (Test-Path -LiteralPath $Prefix -PathType Leaf) {
    Fail "$Prefix is a file, not a folder"
} elseif (Test-Path -LiteralPath $Prefix) {
    try {
        $others = @(Get-OtherWriters $Prefix)
    } catch {
        $others = @()
        Write-Warning "could not read who can change ${Prefix}: $(Get-Reason $_)"
    }
    if ($others.Length) {
        Write-Warning ("$Prefix lets $($others -join ', ') change its files, and so the programs " +
            'you start from it. Install into a folder only you can change, or remove their access.')
    }
} else {
    [void][System.IO.Directory]::CreateDirectory($Prefix)
    Protect-Folder $Prefix
}

# What an earlier install left: files it renamed aside while Workbench was running, and copies
# of an install that stopped half-way.
foreach ($file in @(Get-ChildItem -LiteralPath $Prefix -File -Force)) {
    foreach ($name in $Payload) {
        if ($file.Name -like "$name.*.old" -or $file.Name -eq "$name.new") {
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
    try {
        [System.IO.File]::Copy((Join-Path $here $name), $new, $true)
    } catch {
        $reason = Get-Reason $_
        Remove-Staged
        Fail "cannot copy $name to ${Prefix}: $reason"
    }
    # A copy keeps the download's Mark of the Web, with which Windows stops the installed
    # programs with a SmartScreen warning when they start from Explorer or the Start menu.
    try { Unblock-File -LiteralPath $new } catch { }
}

# Then swap each file in. A running exe or a loaded DLL cannot be overwritten or deleted,
# but it can be renamed: the old file moves aside and is deleted unless something uses it.
$inUse = @()
$done = @()
foreach ($name in $names) {
    $dest = Join-Path $Prefix $name
    $aside = $null
    $failure = $null
    if (Test-Path -LiteralPath $dest) {
        $aside = '{0}.{1}.old' -f $dest, [guid]::NewGuid().ToString('N').Substring(0, 8)
        try {
            Move-File $dest $aside
        } catch {
            $failure = "cannot replace ${dest}: $(Get-Reason $_)"
            $aside = $null
        }
    }
    if (-not $failure) {
        try {
            Move-File "$dest.new" $dest
        } catch {
            $failure = "cannot install ${dest}: $(Get-Reason $_)"
            if ($aside) {
                try { Move-File $aside $dest } catch { }
            }
        }
    }
    if ($failure) {
        Remove-Staged
        if ($done.Length) {
            $failure += " (already updated: $($done -join ', ')). Run install.ps1 again to finish the update."
        }
        Fail $failure
    }
    $done += $name
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
