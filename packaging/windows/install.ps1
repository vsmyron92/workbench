<#
.SYNOPSIS
    Installs Workbench from a release archive for the current user, or uninstalls it.

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

    With -Uninstall it removes Workbench from that folder instead:

        powershell -ExecutionPolicy Bypass -File .\install.ps1 -Uninstall

    It runs workbench service uninstall for each service (sign-in entry, Start Menu shortcut)
    that starts the folder's programs, removes the files install.ps1 put in the folder, the
    folder's entry in your PATH, then the folder once nothing else is in it. Your
    configuration (%APPDATA%\workbench) and Workbench's state (%LOCALAPPDATA%\workbench) stay.
    It changes nothing while a program from the folder runs, or when it cannot tell: stop
    Workbench first, and run it from a terminal outside Workbench.

.PARAMETER Prefix
    The folder to install into, or to uninstall from. Default: %LOCALAPPDATA%\Programs\Workbench.

.PARAMETER Uninstall
    Remove Workbench from the folder instead of installing it.
#>
[CmdletBinding()]
param(
    [string]$Prefix,
    [switch]$Uninstall
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
# The programs among them.
$Programs = @($Payload | Where-Object { $_ -like '*.exe' })

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

# Whether a rename or a delete failed because another program holds the file for a moment
# without letting it be renamed or deleted (an antivirus scanning it, Explorer's preview, a
# program that has just ended), which is worth retrying.
function Test-Busy($ErrorRecord) {
    $e = $ErrorRecord.Exception
    if ($e.InnerException) { $e = $e.InnerException }
    ($e -is [System.UnauthorizedAccessException]) -or (($e -is [System.IO.IOException]) -and
        ($e -isnot [System.IO.FileNotFoundException]) -and ($e -isnot [System.IO.DirectoryNotFoundException]))
}

# Renames $From to $To. Another program can hold either file for a moment, so a sharing
# violation is retried for about a second.
function Move-File([string]$From, [string]$To) {
    $attempt = 0
    while ($true) {
        $attempt++
        try {
            [System.IO.File]::Move($From, $To)
            return
        } catch {
            if ($attempt -ge 10 -or -not (Test-Busy $_)) { throw }
            Start-Sleep -Milliseconds 100
        }
    }
}

# Deletes $Path (nothing when it is gone), retrying a sharing violation as Move-File does.
function Remove-File([string]$Path) {
    $attempt = 0
    while ($true) {
        $attempt++
        try {
            [System.IO.File]::Delete($Path)
            return
        } catch {
            if ($attempt -ge 10 -or -not (Test-Busy $_)) { throw }
            Start-Sleep -Milliseconds 100
        }
    }
}

# Deletes $Path, a file install.ps1 put in $Prefix, for -Uninstall.
function Remove-InstalledFile([string]$Path) {
    try {
        Remove-File $Path
    } catch {
        Fail "cannot remove ${Path}: $(Get-Reason $_) (is a program from $Prefix running?)"
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

# Whether the user's PATH has the entry Add-UserPath writes for $Dir.
function Test-UserPath([string]$Dir) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment')
    if (-not $key) { return $false }
    try {
        $raw = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        return @($raw.Split(';') | Where-Object { $_ -ieq $Dir }).Length -gt 0
    } finally {
        $key.Close()
    }
}

# Removes the entry Add-UserPath wrote for $Dir from the user's PATH, keeping the value's
# type (REG_EXPAND_SZ or REG_SZ) and every other entry as they are; the value goes when no
# entry is left. $true when there was one.
function Remove-UserPath([string]$Dir) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
    if (-not $key) { return $false }
    try {
        $raw = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        $entries = $raw.Split(';')
        $rest = @($entries | Where-Object { $_ -ine $Dir })
        if ($rest.Length -eq $entries.Length) { return $false }
        $value = $rest -join ';'
        if ($value.Trim(';')) {
            $key.SetValue('Path', $value, $key.GetValueKind('Path'))
        } else {
            $key.DeleteValue('Path')
        }
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

# Whether $Path is $Prefix or something in it.
function Test-Inside([string]$Path) {
    ($Path.TrimEnd('\') + '\').StartsWith($Prefix.TrimEnd('\') + '\', [System.StringComparison]::OrdinalIgnoreCase)
}

# Whether install.ps1 puts a file of this name in the install folder: the payload, a file it
# renamed aside (<name>.<id>.old) or a copy it staged (<name>.new).
function Test-Installed([string]$Name) {
    foreach ($p in $Payload) {
        if ($Name -eq $p -or $Name -eq "$p.new" -or $Name -like "$p.*.old") { return $true }
    }
    $false
}

# The programs that may run from $Prefix (the server, its supervisor, the console hosts of its
# terminals, a workbench command, a *.old still running), as "name (pid N)"; throws when the
# programs cannot be listed. Each program's path comes from QueryFullProcessImageNameW, which
# needs no WMI, and whose PROCESS_QUERY_LIMITED_INFORMATION an elevated program's integrity
# level does not block. A program of this session named like one in $Programs whose path
# cannot be read counts, marked as such; another account's programs in other sessions do not
# show.
function Get-FolderProcesses {
    Add-Type -Namespace WorkbenchInstall -Name ProcessImage -MemberDefinition @'
[DllImport("kernel32.dll", SetLastError = true)]
static extern IntPtr OpenProcess(uint dwDesiredAccess, bool bInheritHandle, uint dwProcessId);
[DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
static extern bool QueryFullProcessImageNameW(IntPtr hProcess, uint dwFlags, System.Text.StringBuilder lpExeName, ref uint lpdwSize);
[DllImport("kernel32.dll")]
static extern bool CloseHandle(IntPtr hObject);

// The path of the program process id runs, or null when it cannot be read.
public static string PathOf(int id) {
    // PROCESS_QUERY_LIMITED_INFORMATION
    IntPtr process = OpenProcess(0x1000, false, (uint)id);
    if (process == IntPtr.Zero) return null;
    try {
        System.Text.StringBuilder path = new System.Text.StringBuilder(32768);
        uint size = (uint)path.Capacity;
        return QueryFullProcessImageNameW(process, 0, path, ref size) ? path.ToString() : null;
    } finally {
        CloseHandle(process);
    }
}
'@
    $session = [System.Diagnostics.Process]::GetCurrentProcess().SessionId
    foreach ($p in [System.Diagnostics.Process]::GetProcesses()) {
        $path = [WorkbenchInstall.ProcessImage]::PathOf($p.Id)
        if ($path) {
            if (Test-Inside $path) { "$([System.IO.Path]::GetFileName($path)) (pid $($p.Id))" }
        } elseif ($p.SessionId -eq $session -and $Programs -contains "$($p.ProcessName).exe") {
            "$($p.ProcessName).exe (pid $($p.Id), whose path cannot be read)"
        }
    }
}

# The services of workbench service install [--name N] that start $Launcher, by their entry
# name (Workbench or Workbench-N): a value of HKCU's Run key or a Start Menu shortcut of that
# name. The services of Workbench in another folder are left alone.
function Get-FolderServices([string]$Launcher) {
    $pattern = '^Workbench(-[A-Za-z0-9_-]{1,64})?$'
    $entries = @()
    $run = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Software\Microsoft\Windows\CurrentVersion\Run')
    if ($run) {
        try {
            foreach ($value in $run.GetValueNames()) {
                if ($value -notmatch $pattern) { continue }
                # "<folder>\workbenchw.exe" [--name N]: the program first, quoted.
                if ([string]$run.GetValue($value) -match '^\s*"([^"]*)"' -and $Matches[1] -ieq $Launcher) { $entries += $value }
            }
        } finally {
            $run.Close()
        }
    }
    $programs = [Environment]::GetFolderPath('Programs')
    if ($programs -and (Test-Path -LiteralPath $programs -PathType Container)) {
        $links = @(Get-ChildItem -LiteralPath $programs -Filter 'Workbench*.lnk' -File -Force |
            Where-Object { $_.BaseName -match $pattern -and $entries -notcontains $_.BaseName })
        if ($links.Length) {
            $shell = New-Object -ComObject WScript.Shell
            foreach ($link in $links) {
                # A shortcut that cannot be read is not one of ours.
                try { $target = $shell.CreateShortcut($link.FullName).TargetPath } catch { $target = $null }
                if ($target -ieq $Launcher) { $entries += $link.BaseName }
            }
        }
    }
    $entries
}

# Removes Workbench from $Prefix: the services that start its programs, the files install.ps1
# put there, its PATH entry, then workbench.exe and the folder once nothing else is in it. The
# configuration and the state stay.
function Uninstall-Workbench {
    $exe = Join-Path $Prefix 'workbench.exe'
    $launcher = Join-Path $Prefix 'workbenchw.exe'
    $installed = Test-Path -LiteralPath $exe -PathType Leaf
    if (-not $installed -and (Test-Path -LiteralPath $Prefix)) {
        # Not an install folder, or one an earlier -Uninstall kept for the other files in it
        # (which removed its PATH entry before workbench.exe).
        if (Test-UserPath $Prefix) {
            Fail ("no workbench.exe in ${Prefix}, which is on your PATH: is it the folder Workbench was installed into? " +
                'Nothing was changed. If it is, remove it from Path in your user variables by hand.')
        }
        Write-Host "Workbench is not installed in $Prefix."
        return
    }
    if ($installed) {
        # Windows keeps the files of a running program, and workbench service uninstall stops a
        # server this very script may run in (a Workbench terminal).
        try {
            $running = @(Get-FolderProcesses)
        } catch {
            Fail "cannot list the running programs, to tell whether Workbench runs from ${Prefix}: $(Get-Reason $_). Nothing was changed."
        }
        if ($running.Length) {
            Fail ("Workbench is running from ${Prefix}: $($running -join ', '). Stop it first (workbench service stop, " +
                'or Ctrl+C where workbench serve runs), then run install.ps1 -Uninstall again from a terminal outside ' +
                'Workbench. Nothing was changed.')
        }
        # A process's current folder cannot be removed.
        if (Test-Inside ([Environment]::CurrentDirectory)) {
            Fail "the current folder is in ${Prefix}: change to another one (cd ~) and run install.ps1 -Uninstall again. Nothing was changed."
        }
    }
    try {
        $services = @(Get-FolderServices $launcher)
    } catch {
        Fail "cannot read the sign-in entries and Start Menu shortcuts: $(Get-Reason $_). Nothing was changed."
    }
    if ($installed) {
        foreach ($entry in $services) {
            $arguments = @('service', 'uninstall')
            if ($entry -ne 'Workbench') { $arguments += @('--name', $entry.Substring('Workbench-'.Length)) }
            & $exe @arguments
            if ($LASTEXITCODE -ne 0) { Fail "workbench $($arguments -join ' ') failed (exit code $LASTEXITCODE)" }
        }
        # What install.ps1 put there but workbench.exe, which goes after the PATH entry: as long
        # as it is there, running this again finishes a run stopped half-way.
        foreach ($file in @(Get-ChildItem -LiteralPath $Prefix -File -Force)) {
            if ($file.Name -ne 'workbench.exe' -and (Test-Installed $file.Name)) { Remove-InstalledFile $file.FullName }
        }
    } else {
        foreach ($entry in $services) {
            Write-Warning "$entry still starts $launcher, which is gone: turn it off in Task Manager > Startup apps and delete its Start Menu shortcut."
        }
    }
    $unlisted = Remove-UserPath $Prefix
    if ($unlisted) {
        try {
            Send-SettingChange
        } catch {
            Write-Warning "could not announce the new PATH ($(Get-Reason $_)): sign out and in again."
        }
        Write-Host "Removed $Prefix from your PATH: terminals opened from now on do not have it."
    }
    if ($installed) {
        Remove-InstalledFile $exe
        $left = @(Get-ChildItem -LiteralPath $Prefix -Force | ForEach-Object { $_.Name })
        if ($left.Length) {
            Write-Host "Removed Workbench from $Prefix, and kept the folder for the other files in it: $($left -join ', ')"
        } else {
            try {
                [System.IO.Directory]::Delete($Prefix)
                Write-Host "Removed Workbench and its folder $Prefix"
            } catch {
                Write-Warning "removed Workbench from $Prefix, but not the empty folder: $(Get-Reason $_)"
            }
        }
    } elseif (-not $unlisted) {
        Write-Host "Workbench is not installed in $Prefix."
        return
    }
    # What a new install uses again, where Workbench looks for it (WORKBENCH_CONFIG_DIR and
    # WORKBENCH_DATA_DIR override both).
    $config = $env:WORKBENCH_CONFIG_DIR
    if (-not $config) { $config = Join-Path ([Environment]::GetFolderPath('ApplicationData')) 'workbench' }
    $state = $env:WORKBENCH_DATA_DIR
    if (-not $state) { $state = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'workbench' }
    $kept = @()
    if (Test-Path -LiteralPath $config -PathType Container) { $kept += "  $config  (config.toml, project overlays)" }
    if (Test-Path -LiteralPath $state -PathType Container) {
        $kept += "  $state  (sign-ins, sessions, Local History, Workspace cards)"
    }
    if ($kept.Length) {
        Write-Host 'Kept your configuration and Workbench''s state for a later install; delete these folders to remove them too:'
        foreach ($line in $kept) { Write-Host $line }
    }
}

$here = $PSScriptRoot
if (-not $Uninstall) {
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
}

if (-not $Prefix) {
    if (-not $env:LOCALAPPDATA) { Fail 'LOCALAPPDATA is not set: pass -Prefix <folder>' }
    $Prefix = Join-Path $env:LOCALAPPDATA 'Programs\Workbench'
}
# Relative to the current location, like any PowerShell path; no trailing separator.
$Prefix = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Prefix)
if ($Prefix.Length -gt 3) { $Prefix = $Prefix.TrimEnd('\') }
if (-not $Uninstall -and $Prefix -ieq $here.TrimEnd('\')) {
    Fail "run install.ps1 from the unpacked archive, not from $Prefix"
}
# PATH separates folders with ; and expands %NAME%.
if ($Prefix.Contains(';') -or $Prefix.Contains('%')) {
    Fail "the install folder goes on PATH, so its name cannot contain ; or %: $Prefix"
}
if (Test-Path -LiteralPath $Prefix -PathType Leaf) {
    Fail "$Prefix is a file, not a folder"
}
if ($Uninstall) {
    Uninstall-Workbench
    exit 0
}
if (Test-Path -LiteralPath $Prefix) {
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
