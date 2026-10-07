#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Ask THIS machine what the Windows installer does, and say what each step left behind.

.DESCRIPTION
    What the installer does on a machine is not in its source, so crates/cli/tests/msi.rs can only hold the
    lines the behaviour rests on. This script installs it, upgrades it while a program holds one of its
    libraries, rebuilds the same version over itself, refuses an older version, and uninstalls it with a file
    of somebody else's in its folder - and after each step asks the machine, not the installer, what is there.

    It runs the same way on a build runner (ci.yml, job "installer"), on a clean virtual machine over ssh and
    on a developer machine. One script, so the three cannot drift apart.

    THE KIT. Three installers built from one window package:
        old   v0.0.1  the package as it is
        new   v0.0.2  the package with a few bytes added to four files (a managed library, the 64-bit command
                      line, the injected library and a calendar) - the same file version, other bytes
        same  v0.0.2  the package as it is, under the number of "new" - a rebuild of a version, as a feed's
                      moderation may ask for, whose files must replace the first build's
    A kit also carries what the measurement compares the machine with: the hash of every file of both
    packages, which is why a machine that only RUNS a kit (a virtual machine) needs no package and no WiX.

    WHAT IT ASKS. Starting state (nothing of ours is there, and the machine PATH is written down), then per
    step the exit code, Programs and Features, every file in the folder against the package it was built from,
    the machine PATH, a new process asking for `chrono`, the catalogue the command line reads (from an empty
    working folder), the Start menu entry, and what survives an uninstall. Steps that need a desktop are
    behind -Window.

    Every line is `ok`, `FAILED` or `NOT MEASURED` (with the reason). Nothing is passed by default: a step
    that could not be asked says so and the exit code says so.

    Exit codes: 0 everything asked was ok, 1 something FAILED, 2 the starting state was not clean or the
    inputs are wrong (nothing was touched), 3 nothing FAILED but something was NOT MEASURED.

    SAFETY. It refuses to start when a copy of the product is already installed, kills only the processes it
    started itself, and on the way out - however it ends - uninstalls what it installed by product code, so a
    run that is stopped does not leave a test product on the machine. -Cleanup does that by hand.

.PARAMETER PackageFolder
    The window package to build the kit from, the folder that holds ChronoMock.exe (dist/ChronoMock). Not
    needed when -KitFolder is given.

.PARAMETER WorkFolder
    Where the kit (unless -KitFolder is given), the installer logs and the report go. Created if missing.

.PARAMETER KitFolder
    A kit built earlier (-BuildKitOnly), so a machine without WiX can run it.

.PARAMETER BuildKitOnly
    Build the kit into <WorkFolder>/kit and stop. Installs nothing.

.PARAMETER Window
    Also start the installed window as administrator and ask whether it keeps anything of its own in the
    install folder. Needs a desktop. Without one the step is NOT MEASURED, not passed.

.PARAMETER SurvivorProgram
    The program that outlives its session and so keeps the injected library loaded. Default: the system's
    ping, which lives for ten minutes and touches nothing.

.PARAMETER SurvivorArguments
    Its arguments, as one string.

.PARAMETER Cleanup
    Uninstall every copy of the product this machine has, by product code, and stop.
#>
[CmdletBinding()]
param(
    [string] $PackageFolder = '',
    [string] $WorkFolder = '',
    [string] $KitFolder = '',
    [switch] $BuildKitOnly,
    [switch] $Window,
    [string] $SurvivorProgram = '',
    [string] $SurvivorArguments = '-n 600 127.0.0.1',
    [switch] $Cleanup
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$script:Failed = 0
$script:Passed = 0
$script:NotMeasured = 0
$script:LogPath = ''
$script:LogFolder = ''
$script:Product = $null
$script:Started = New-Object System.Collections.Generic.List[int]

# The one shell call the icon question needs.
if (-not ('Probe.Shell' -as [type])) {
    Add-Type -Namespace Probe -Name Shell -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("shell32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)]
public static extern uint ExtractIconEx(string file, int index, System.IntPtr[] large, System.IntPtr[] small, uint icons);
'@
}

# --- reporting ------------------------------------------------------------------------------------------

function Say([string] $line) {
    Write-Host $line
    if ($script:LogPath) { Add-Content -LiteralPath $script:LogPath -Value $line }
}

function Check([bool] $ok, [string] $what) {
    if ($ok) { $script:Passed++; Say "ok      $what" } else { $script:Failed++; Say "FAILED  $what" }
}

function Skip([string] $what) {
    $script:NotMeasured++
    Say "NOT MEASURED  $what"
}

function Note([string] $what) { Say "note    $what" }

# A refusal of the inputs is one line and exit 2, before anything on the machine is touched.
function Stop-Run([string] $message) {
    [Console]::Error.WriteLine("test-installer: $message")
    exit 2
}

# --- what the machine says --------------------------------------------------------------------------------

function Get-Prop($item, [string] $name) {
    $property = $item.PSObject.Properties[$name]
    if ($property) { return $property.Value }
    return $null
}

# Programs and Features entries under the product's name, in both registry views.
function Get-ArpEntries {
    $found = New-Object System.Collections.Generic.List[object]
    foreach ($root in @('HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall', 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall')) {
        if (-not (Test-Path -LiteralPath $root)) { continue }
        foreach ($key in Get-ChildItem -LiteralPath $root -ErrorAction SilentlyContinue) {
            $values = Get-ItemProperty -LiteralPath $key.PSPath -ErrorAction SilentlyContinue
            if ($values -and (Get-Prop $values 'DisplayName') -eq $script:Product.name) {
                $found.Add([pscustomobject]@{ Code = $key.PSChildName; View = $root; Values = $values })
            }
        }
    }
    # Not -NoEnumerate: an empty array sent as one object would reach @() as a list of one.
    return $found.ToArray()
}

# The machine PATH as stored, with the references in it unexpanded, and the kind it is stored as.
function Get-MachinePath {
    $key = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey('SYSTEM\CurrentControlSet\Control\Session Manager\Environment')
    try {
        return [pscustomobject]@{
            Value = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            Kind  = $key.GetValueKind('Path').ToString()
        }
    }
    finally { $key.Dispose() }
}

function Restore-MachinePath($saved) {
    $key = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey('SYSTEM\CurrentControlSet\Control\Session Manager\Environment', $true)
    try { $key.SetValue('Path', $saved.Value, [Microsoft.Win32.RegistryValueKind]$saved.Kind) }
    finally { $key.Dispose() }
}

function Get-OursOnPath([string] $installDir) {
    $want = (Join-Path $installDir 'core\x64').TrimEnd('\')
    return @((Get-MachinePath).Value.Split(';') | Where-Object { $_.Trim().TrimEnd('\') -ieq $want })
}

# Every file under a folder, hidden ones included: relative path (lower case, forward slashes) -> sha256.
function Get-FileHashes([string] $folder) {
    $hashes = New-Object 'System.Collections.Generic.Dictionary[string,string]'
    if (-not (Test-Path -LiteralPath $folder -PathType Container)) { return $hashes }
    $base = (Get-Item -LiteralPath $folder).FullName.TrimEnd('\')
    foreach ($file in Get-ChildItem -LiteralPath $base -Recurse -File -Force) {
        $relative = $file.FullName.Substring($base.Length + 1).Replace('\', '/').ToLowerInvariant()
        $hashes[$relative] = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    return $hashes
}

# What the restart will still delete or move: the sources of PendingFileRenameOperations, lower case.
function Get-PendingSources {
    $pending = New-Object System.Collections.Generic.HashSet[string]
    $value = Get-ItemProperty -LiteralPath 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager' -Name PendingFileRenameOperations -ErrorAction SilentlyContinue
    # Not `$x = if (...) { @() }`: an empty array out of an if is nothing, and nothing is $null.
    $operations = @()
    if ($value) { $operations = @(Get-Prop $value 'PendingFileRenameOperations') }
    for ($i = 0; $i -lt $operations.Count; $i += 2) {
        [void]$pending.Add(($operations[$i] -replace '^\\\?\?\\', '').ToLowerInvariant())
    }
    # The comma keeps the set whole: returned bare, a set is poured into the pipeline, and an empty one
    # becomes nothing at all.
    return , $pending
}

# --- the installer ----------------------------------------------------------------------------------------

# msiexec with its arguments in a variable and a limit on the wait. A quote that swallows the file name leaves
# msiexec waiting on a help window nobody can close (measured on a machine without a desktop).
function Invoke-Msi([string] $verb, [string] $target, [string] $logName) {
    $log = Join-Path $script:LogFolder $logName
    $arguments = @($verb, ('"' + $target + '"'), '/qn', '/norestart', '/l*v', ('"' + $log + '"'))
    $began = Get-Date
    $process = Start-Process -FilePath (Join-Path $env:SystemRoot 'System32\msiexec.exe') -ArgumentList $arguments -PassThru
    $null = $process.Handle
    if (-not $process.WaitForExit(600000)) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        $result = [pscustomobject]@{ Code = -1; Seconds = 600; Log = $log }
    }
    else {
        $result = [pscustomobject]@{ Code = $process.ExitCode; Seconds = [int]((Get-Date) - $began).TotalSeconds; Log = $log }
    }
    Say "> msiexec $verb $(Split-Path -Leaf $target) -> exit $($result.Code) in $($result.Seconds) s (log: $logName)"
    return $result
}

# Whether $installDir holds exactly the files a package puts down, byte for byte. $expected maps relative path
# -> sha256 (an empty hash means any content). Files the restart is still to delete are not unexpected.
function Test-InstalledTree([string] $installDir, $expected, [string] $label) {
    $actual = Get-FileHashes $installDir
    $pending = Get-PendingSources
    $missing = @($expected.Keys | Where-Object { -not $actual.ContainsKey($_) })
    $different = @($expected.Keys | Where-Object { $actual.ContainsKey($_) -and $expected[$_] -and $actual[$_] -ne $expected[$_] })
    $extra = @($actual.Keys | Where-Object { -not $expected.ContainsKey($_) })
    $waiting = @($extra | Where-Object { $pending.Contains((Join-Path $installDir $_).Replace('/', '\').ToLowerInvariant()) })
    $unexpected = @($extra | Where-Object { $waiting -notcontains $_ })
    # Up to five names, because which files stayed is what a reader needs to see why.
    Check ($missing.Count -eq 0) "$label : no file of the package is missing ($($missing.Count) missing$(if ($missing) { ': ' + (($missing | Select-Object -First 5) -join ', ') }))"
    Check ($different.Count -eq 0) "$label : every file is the package's bytes ($($different.Count) differ$(if ($different) { ': ' + (($different | Select-Object -First 5) -join ', ') }))"
    Check ($unexpected.Count -eq 0) "$label : nothing else is in the folder ($($unexpected.Count) extra$(if ($unexpected) { ': ' + (($unexpected | Select-Object -First 5) -join ', ') }))"
    if ($waiting.Count -gt 0) { Note "$label : $($waiting.Count) file(s) set aside and left for the next restart" }
}

# The tree an installer built from a package puts down: the package, the installer's own README, the marker
# and the catalogues beside each core.
function Get-ExpectedTree($tree, $extras) {
    $expected = New-Object 'System.Collections.Generic.Dictionary[string,string]'
    foreach ($entry in $tree.GetEnumerator()) { $expected[$entry.Key] = $entry.Value }
    $expected['readme.md'] = $extras.readmeSha256
    $expected[$extras.marker] = ''
    foreach ($core in $extras.cores) {
        foreach ($folder in $extras.catalogueFolders) {
            foreach ($entry in $tree.GetEnumerator()) {
                if ($entry.Key.StartsWith("$folder/")) { $expected["$core/$($entry.Key)"] = $entry.Value }
            }
        }
    }
    return $expected
}

# --- the kit ----------------------------------------------------------------------------------------------

function Add-Overlay([string] $path, [byte[]] $bytes) {
    $stream = [System.IO.File]::Open($path, [System.IO.FileMode]::Append, [System.IO.FileAccess]::Write)
    try { $stream.Write($bytes, 0, $bytes.Length) } finally { $stream.Dispose() }
}

function Get-FileVersion([string] $path) {
    $version = (Get-Item -LiteralPath $path).VersionInfo.FileVersion
    if ($version) { return [string]$version } else { return '' }
}

# Three installers and the facts the measurement compares a machine with. The second package is the first with
# bytes added where Windows ignores them (the end of a program, the end of a calendar), so the file version of
# each is the same and the hash is not.
function New-Kit([string] $package, [string] $kit) {
    $root = Split-Path -Parent $PSScriptRoot
    $register = Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot 'components.json') | ConvertFrom-Json
    $product = $register.product
    $installerName = [string]$register.packages.msi.zip
    $buildMsi = Join-Path $PSScriptRoot 'build-msi.ps1'
    $pwsh = (Get-Process -Id $PID).Path
    New-Item -ItemType Directory -Force -Path $kit | Out-Null
    foreach ($name in 'old.msi', 'new.msi', 'same.msi', 'kit.json') {
        if (Test-Path -LiteralPath (Join-Path $kit $name)) { Stop-Run "$name is already in $kit - pass an empty folder" }
    }

    $second = Join-Path $kit 'package-b'
    New-Item -ItemType Directory -Force -Path $second | Out-Null
    Get-ChildItem -LiteralPath $package -Force | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $second -Recurse -Force }
    $watched = @('ChronoMock.dll', 'core/x64/chrono.exe', 'core/x64/chrono_hook.dll', 'calendars/pl.json')
    $facts = @()
    foreach ($relative in $watched) {
        $first = Join-Path $package $relative
        $other = Join-Path $second $relative
        if (-not (Test-Path -LiteralPath $first)) { Stop-Run "the package has no $relative, so the kit cannot tell which bytes landed" }
        # Sixteen bytes Windows ignores: zeros after a program, line feeds after a JSON text.
        $overlay = [byte[]]::new(16)
        if ($relative.EndsWith('.json')) { [Array]::Fill($overlay, [byte]10) }
        Add-Overlay $other $overlay
        $facts += [ordered]@{
            path = $relative
            versionA = Get-FileVersion $first
            versionB = Get-FileVersion $other
            sha256A = (Get-FileHash -LiteralPath $first -Algorithm SHA256).Hash.ToLowerInvariant()
            sha256B = (Get-FileHash -LiteralPath $other -Algorithm SHA256).Hash.ToLowerInvariant()
        }
    }
    foreach ($fact in $facts) {
        if ($fact.sha256A -eq $fact.sha256B) { Stop-Run "$($fact.path) has the same hash in both packages, so the kit does not tell whose bytes landed" }
    }

    $builds = @(
        @{ name = 'old.msi'; tag = 'v0.0.1'; from = $package },
        @{ name = 'new.msi'; tag = 'v0.0.2'; from = $second },
        @{ name = 'same.msi'; tag = 'v0.0.2'; from = $package }
    )
    foreach ($build in $builds) {
        $into = Join-Path $kit ('out-' + [guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $into | Out-Null
        $said = & $pwsh -NoProfile -File $buildMsi -Tag $build.tag -Payload $build.from -OutDir $into 2>&1
        if ($LASTEXITCODE -ne 0) { $said | ForEach-Object { Write-Host $_ }; Stop-Run "build-msi.ps1 refused $($build.tag) from $($build.from)" }
        Move-Item -LiteralPath (Join-Path $into $installerName) -Destination (Join-Path $kit $build.name)
        Remove-Item -LiteralPath $into -Recurse -Force
        Write-Host "built $($build.name) ($($build.tag)) from $($build.from)"
    }

    $treeA = Get-FileHashes $package
    $treeB = Get-FileHashes $second
    $said = (& (Join-Path $package 'core\x64\chrono.exe') version 2>&1 | Out-String).Trim()
    $manifest = [ordered]@{
        product = [ordered]@{ name = $product.name; publisher = (($product.supplier -split ':\s*', 2)[-1]); homepage = $product.homepage; source = $product.source }
        old = [ordered]@{ file = 'old.msi'; version = '0.0.1'; tag = 'v0.0.1' }
        new = [ordered]@{ file = 'new.msi'; version = '0.0.2'; tag = 'v0.0.2' }
        same = [ordered]@{ file = 'same.msi'; version = '0.0.2'; tag = 'v0.0.2' }
        treeA = $treeA
        treeB = $treeB
        extras = [ordered]@{
            readmeSha256 = (Get-FileHash -LiteralPath (Join-Path $PSScriptRoot 'msi-readme.md') -Algorithm SHA256).Hash.ToLowerInvariant()
            marker = 'installed.txt'
            cores = @('core/x64', 'core/x86')
            catalogueFolders = @('calendars', 'presets')
        }
        watched = $facts
        cliVersionSaid = $said
    }
    [System.IO.File]::WriteAllText((Join-Path $kit 'kit.json'), ($manifest | ConvertTo-Json -Depth 6), (New-Object System.Text.UTF8Encoding($false)))
    Remove-Item -LiteralPath $second -Recurse -Force
    Write-Host "kit written to $kit"
}

# --- the program that outlives its session ----------------------------------------------------------------

# Runs the installed command line on a program for two heartbeats. The session ends, the program does not,
# and so it keeps the injected library loaded from the install folder. Returns the process, or $null with the
# reason said.
function Start-Survivor([string] $installDir) {
    $program = if ($SurvivorProgram) { $SurvivorProgram } else { Join-Path ([Environment]::SystemDirectory) 'ping.exe' }
    if (-not (Test-Path -LiteralPath $program)) { Skip "an application that outlives its session: $program does not exist"; return $null }
    $name = [System.IO.Path]::GetFileNameWithoutExtension($program)
    $before = @(Get-Process -Name $name -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
    $arguments = "run `"$program`" --at 2030-06-15T12:00:00 --zone +00:00 --ticks 2 --timeout 60"
    if ($SurvivorArguments) { $arguments += " --args `"$SurvivorArguments`"" }
    $core = Join-Path $installDir 'core\x64\chrono.exe'
    $out = Join-Path $script:LogFolder 'survivor-session-out.txt'
    $err = Join-Path $script:LogFolder 'survivor-session-err.txt'
    $session = Start-Process -FilePath $core -ArgumentList $arguments -PassThru -WindowStyle Hidden -RedirectStandardOutput $out -RedirectStandardError $err
    $null = $session.Handle
    if (-not $session.WaitForExit(90000)) {
        Stop-Process -Id $session.Id -Force -ErrorAction SilentlyContinue
        Skip 'an application that outlives its session: the session did not end in 90 s'
        return $null
    }
    $after = @(Get-Process -Name $name -ErrorAction SilentlyContinue | Where-Object { $before -notcontains $_.Id })
    foreach ($process in $after) { $script:Started.Add($process.Id) }
    if ($after.Count -eq 0) {
        $reason = (Get-Content -LiteralPath $err -ErrorAction SilentlyContinue | Select-Object -First 3) -join ' | '
        Skip "an application that outlives its session: nothing of $name is left after the session (chrono exited $($session.ExitCode)) $reason"
        return $null
    }
    return $after[0]
}

function Stop-Survivors {
    foreach ($id in $script:Started.ToArray()) { Stop-Process -Id $id -Force -ErrorAction SilentlyContinue }
    $script:Started.Clear()
}

# --- the window -------------------------------------------------------------------------------------------

# Starts a window from $folder and waits for it to show. Returns the process or $null. The window is started
# by this (administrator) process, so it runs as administrator.
function Start-WindowAndWait([string] $folder) {
    $process = Start-Process -FilePath (Join-Path $folder 'ChronoMock.exe') -WorkingDirectory $folder -PassThru
    $script:Started.Add($process.Id)
    $deadline = (Get-Date).AddSeconds(45)
    while ((Get-Date) -lt $deadline) {
        $process.Refresh()
        if ($process.HasExited) { return $null }
        if ($process.MainWindowHandle -ne [IntPtr]::Zero) { break }
        Start-Sleep -Milliseconds 500
    }
    $process.Refresh()
    if ($process.HasExited -or $process.MainWindowHandle -eq [IntPtr]::Zero) { return $null }
    # What the window writes at start has been written by now.
    Start-Sleep -Seconds 4
    return $process
}

function Stop-Window($process) {
    if ($null -eq $process) { return }
    [void]$process.CloseMainWindow()
    if (-not $process.WaitForExit(15000)) { Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue }
}

# The window keeps nothing of its own in the folder the installer owns, and it does when it is not installed.
# The second half is the control: a window that is not the installed one MUST make the folders, or the first
# half proves nothing.
function Test-WindowKeepsNothing([string] $installDir, [string] $marker) {
    # The control is the installed folder itself, copied without the marker: the same bytes, a window that
    # does not know it is installed.
    $canary = Join-Path $script:WorkFolderResolved 'copy-without-marker'
    if (Test-Path -LiteralPath $canary) { Remove-Item -LiteralPath $canary -Recurse -Force }
    New-Item -ItemType Directory -Path $canary | Out-Null
    Get-ChildItem -LiteralPath $installDir -Force | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $canary -Recurse -Force }
    Remove-Item -LiteralPath (Join-Path $canary $marker) -Force
    $control = Start-WindowAndWait $canary
    if ($null -eq $control) { Skip 'the window as administrator: no window showed (no desktop on this machine?), so the installed one was not asked either'; return }
    $made = @('history', 'logs') | Where-Object { Test-Path -LiteralPath (Join-Path $canary $_) }
    Stop-Window $control
    if ($made.Count -eq 0) { Skip 'the window as administrator: a copy that is not installed made neither a history nor a logs folder, so the installed copy making none proves nothing'; return }
    Note "the control, a copy without the marker, made: $($made -join ', ')"

    $window = Start-WindowAndWait $installDir
    if ($null -eq $window) { Skip 'the window as administrator: the installed window did not show'; return }
    $own = @(Get-ChildItem -LiteralPath $installDir -Recurse -Force -ErrorAction SilentlyContinue | Where-Object { $_.Name -in 'history', 'logs' -or $_.Name -like '.write-probe*' })
    Stop-Window $window
    Check ($own.Count -eq 0) "the window started as administrator from the install folder keeps nothing of its own there ($($own.Count) found$(if ($own) { ': ' + $own[0].FullName }))"
    Remove-Item -LiteralPath $canary -Recurse -Force -ErrorAction SilentlyContinue
}

# --- what is asked after an install -----------------------------------------------------------------------

function Test-Installed($kitFacts, [string] $version, $tree, [string] $label) {
    $name = $script:Product.name
    $installDir = Join-Path $env:ProgramFiles $name
    $entries = @(Get-ArpEntries)
    Check ($entries.Count -eq 1) "$label : one entry in Programs and Features ($(($entries | ForEach-Object { (Get-Prop $_.Values 'DisplayVersion') + ' ' + $_.Code }) -join ', '))"
    if ($entries.Count -ge 1) {
        $values = $entries[0].Values
        Check ((Get-Prop $values 'DisplayVersion') -eq $version) "$label : it says $version ($(Get-Prop $values 'DisplayVersion'))"
        Check ((Get-Prop $values 'Publisher') -eq $script:Product.publisher) "$label : the publisher is $($script:Product.publisher) ($(Get-Prop $values 'Publisher'))"
        Check ((Get-Prop $values 'URLInfoAbout') -eq $script:Product.homepage) "$label : about is the product page ($(Get-Prop $values 'URLInfoAbout'))"
        Check ((Get-Prop $values 'HelpLink') -eq "$($script:Product.source)/issues") "$label : help is the issue tracker ($(Get-Prop $values 'HelpLink'))"
        Check ((Get-Prop $values 'URLUpdateInfo') -eq "$($script:Product.source)/releases/tag/v$version") "$label : update info is the release page ($(Get-Prop $values 'URLUpdateInfo'))"
        Check ((Get-Prop $values 'NoModify') -eq 1 -and (Get-Prop $values 'NoRepair') -eq 1) "$label : there is nothing to change or repair (NoModify $(Get-Prop $values 'NoModify'), NoRepair $(Get-Prop $values 'NoRepair'))"
        # Windows Installer writes no DisplayIcon for ARPPRODUCTICON. It registers the icon with the product
        # and keeps a copy of it in its own cache, which is where the Apps list reads it from.
        $icon = ''
        foreach ($product in Get-ChildItem -LiteralPath 'HKLM:\SOFTWARE\Classes\Installer\Products' -ErrorAction SilentlyContinue) {
            $registered = Get-ItemProperty -LiteralPath $product.PSPath -ErrorAction SilentlyContinue
            if ($registered -and (Get-Prop $registered 'ProductName') -eq $name) { $icon = [string](Get-Prop $registered 'ProductIcon') }
        }
        # Asked the way the shell asks it. The cached copy has no file extension (the icon's id in the installer
        # source is its name), and the shell reads an icon by its content, which this confirms.
        $icons = 0
        if ($icon -and (Test-Path -LiteralPath $icon)) { $icons = [Probe.Shell]::ExtractIconEx($icon, -1, $null, $null, 0) }
        Check ($icons -ge 1) "$label : the product's icon is registered and the shell reads $icons icon(s) out of its cached copy ($icon)"
    }
    Test-InstalledTree $installDir (Get-ExpectedTree $tree $kitFacts.extras) $label
    Check (@(Get-OursOnPath $installDir).Count -eq 1) "$label : core\x64 is on the machine PATH once ($(@(Get-OursOnPath $installDir).Count))"

    # A new process, with the PATH a new terminal would read.
    $machine = [Environment]::ExpandEnvironmentVariables((Get-MachinePath).Value)
    $user = [Environment]::GetEnvironmentVariable('Path', 'User')
    $saved = $env:Path
    $empty = Join-Path $script:WorkFolderResolved ('cwd-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $empty | Out-Null
    try {
        $env:Path = "$machine;$user"
        Push-Location $empty
        $where = @(& cmd.exe /c 'where chrono' 2>&1)
        $said = ((& cmd.exe /c 'chrono version' 2>&1) | Out-String).Trim()
        $presets = ((& cmd.exe /c 'chrono presets --json' 2>&1) | Out-String)
        & cmd.exe /c 'chrono calc --base 2026-01-02T00:00:00 --shift +1bd --calendar pl' 2>&1 | Out-Null
        $calcCode = $LASTEXITCODE
        $thirty = ((& (Join-Path $installDir 'core\x86\chrono.exe') version 2>&1) | Out-String).Trim()
    }
    finally {
        Pop-Location
        $env:Path = $saved
        Remove-Item -LiteralPath $empty -Recurse -Force -ErrorAction SilentlyContinue
    }
    Check (($where | Select-Object -First 1) -ieq (Join-Path $installDir 'core\x64\chrono.exe')) "$label : a new process finds chrono first in the install folder ($($where | Select-Object -First 1))"
    Check ($said -eq $kitFacts.cliVersionSaid) "$label : chrono on PATH answers like the package's own ('$said' against '$($kitFacts.cliVersionSaid)')"
    $dir = ''
    try { $dir = [string](($presets | ConvertFrom-Json).dir) } catch { $dir = '' }
    Check ($dir -ieq (Join-Path $installDir 'core\x64\presets')) "$label : started from an empty folder it reads the catalogue of the install ($dir)"
    Check ($calcCode -eq 0) "$label : started from an empty folder it counts business days in an installed calendar (exit $calcCode)"
    Check ($thirty -match 'x86') "$label : the 32-bit tool answers by its path ($thirty)"

    $shortcut = Join-Path ([Environment]::GetFolderPath('CommonPrograms')) "$name.lnk"
    Check (Test-Path -LiteralPath $shortcut) "$label : the Start menu of every account has the entry"
    if (Test-Path -LiteralPath $shortcut) {
        $link = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcut)
        Check ($link.TargetPath -ieq (Join-Path $installDir 'ChronoMock.exe')) "$label : it starts the window ($($link.TargetPath))"
        Check ($link.WorkingDirectory.TrimEnd('\') -ieq $installDir) "$label : it starts in the install folder ($($link.WorkingDirectory))"
    }
}

# --- the run ----------------------------------------------------------------------------------------------

function Invoke-Cleanup {
    $any = $false
    foreach ($entry in @(Get-ArpEntries)) {
        $any = $true
        Say "uninstalling $($entry.Code)"
        [void](Invoke-Msi '/x' $entry.Code ("cleanup-$($entry.Code.Trim('{}')).log"))
    }
    if (-not $any) { Say 'nothing of the product is installed' }
}

if ($Cleanup) {
    # The name comes from the kit when a machine has only that, and from the register otherwise.
    $productName = if ($KitFolder -and (Test-Path -LiteralPath (Join-Path $KitFolder 'kit.json'))) {
        (Get-Content -Raw -LiteralPath (Join-Path $KitFolder 'kit.json') | ConvertFrom-Json).product.name
    }
    else {
        (Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot 'components.json') | ConvertFrom-Json).product.name
    }
    $script:Product = [pscustomobject]@{ name = [string]$productName }
    $script:LogFolder = if ($WorkFolder) { New-Item -ItemType Directory -Force -Path $WorkFolder | Select-Object -ExpandProperty FullName } else { $env:TEMP }
    Invoke-Cleanup
    exit 0
}

if (-not $WorkFolder) { Stop-Run 'pass -WorkFolder, where the kit, the logs and the report go' }
if (-not $KitFolder -and -not $PackageFolder) { Stop-Run 'pass -PackageFolder (the folder that holds ChronoMock.exe) to build a kit from, or -KitFolder to run one' }
$script:WorkFolderResolved = (New-Item -ItemType Directory -Force -Path $WorkFolder).FullName
$script:LogFolder = (New-Item -ItemType Directory -Force -Path (Join-Path $script:WorkFolderResolved 'logs')).FullName
$script:LogPath = Join-Path $script:LogFolder "installer-$($env:COMPUTERNAME).txt"
if (Test-Path -LiteralPath $script:LogPath) { Remove-Item -LiteralPath $script:LogPath -Force }

if (-not $KitFolder) {
    if (-not (Test-Path -LiteralPath (Join-Path $PackageFolder 'ChronoMock.exe'))) { Stop-Run "$PackageFolder holds no ChronoMock.exe - pass the window package" }
    $KitFolder = Join-Path $script:WorkFolderResolved 'kit'
    New-Kit (Get-Item -LiteralPath $PackageFolder).FullName $KitFolder
    if ($BuildKitOnly) { exit 0 }
}
if (-not (Test-Path -LiteralPath (Join-Path $KitFolder 'kit.json'))) { Stop-Run "$KitFolder holds no kit.json - build one with -BuildKitOnly" }
$kit = Get-Content -Raw -LiteralPath (Join-Path $KitFolder 'kit.json') | ConvertFrom-Json -AsHashtable
$script:Product = [pscustomobject]$kit.product
$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { Stop-Run 'run this from an elevated session - an installer for every account needs one' }

$name = $script:Product.name
$installDir = Join-Path $env:ProgramFiles $name
$shortcut = Join-Path ([Environment]::GetFolderPath('CommonPrograms')) "$name.lnk"
$vendorKey = "HKLM:\SOFTWARE\$($script:Product.publisher)\$name"
$perUser = Join-Path $env:LOCALAPPDATA 'ChronoMock'
$treeA = New-Object 'System.Collections.Generic.Dictionary[string,string]'
foreach ($entry in $kit.treeA.GetEnumerator()) { $treeA[$entry.Key] = $entry.Value }
$treeB = New-Object 'System.Collections.Generic.Dictionary[string,string]'
foreach ($entry in $kit.treeB.GetEnumerator()) { $treeB[$entry.Key] = $entry.Value }

Say "machine $($env:COMPUTERNAME), $((Get-CimInstance Win32_OperatingSystem).Caption) $([Environment]::OSVersion.Version), pwsh $($PSVersionTable.PSVersion), $(Get-Date -Format s)"

# 0. the state this starts from is asserted, not assumed
$dirty = @()
if (@(Get-ArpEntries).Count -ne 0) { $dirty += "$(@(Get-ArpEntries).Count) entr(y/ies) in Programs and Features" }
if (Test-Path -LiteralPath $installDir) { $dirty += "the folder $installDir" }
if (@(Get-OursOnPath $installDir).Count -ne 0) { $dirty += 'an entry on the machine PATH' }
if (Test-Path -LiteralPath $shortcut) { $dirty += 'the Start menu entry' }
if (Test-Path -LiteralPath $vendorKey) { $dirty += "the registry key $vendorKey" }
if ($dirty.Count -gt 0) { Stop-Run "something of ours is here already: $($dirty -join ', '). Run with -Cleanup first, nothing was touched" }

$startPath = Get-MachinePath
Note "machine PATH written down ($($startPath.Value.Length) characters, stored as $($startPath.Kind))"
$ownedPerUser = -not (Test-Path -LiteralPath $perUser)
$sentinel = Join-Path $perUser 'logs\installer-test-sentinel.txt'
if ($ownedPerUser) {
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $sentinel) | Out-Null
    Set-Content -LiteralPath $sentinel -Value 'the uninstall must leave this folder alone'
}

$old = Join-Path $KitFolder $kit.old.file
$new = Join-Path $KitFolder $kit.new.file
$same = Join-Path $KitFolder $kit.same.file
$completed = $false
try {
    foreach ($fact in $kit.watched) {
        Note "kit: $($fact.path) is '$($fact.versionA)' and '$($fact.versionB)', hashes differ$(if (-not $fact.versionA) { ' (a file with no version: Windows Installer decides by hash)' })"
        Check ($fact.versionA -eq $fact.versionB) "kit: $($fact.path) has the same file version in both packages ('$($fact.versionA)')"
    }

    Say ''
    Say "== install $($kit.old.version)"
    $result = Invoke-Msi '/i' $old 'install-old.log'
    Check ($result.Code -eq 0) "the installer installs (exit $($result.Code))"
    Test-Installed $kit $kit.old.version $treeA 'installed'
    if ($Window) { Test-WindowKeepsNothing $installDir $kit.extras.marker } else { Note 'the window as administrator is not asked without -Window' }

    Say ''
    Say "== upgrade to $($kit.new.version) while an application holds the injected library"
    $held = Start-Survivor $installDir
    $holds = $false
    if ($held) {
        try {
            $modules = @($held.Modules | Where-Object { $_.ModuleName -ieq 'chrono_hook.dll' })
            $holds = ($modules.Count -gt 0) -and ($modules[0].FileName -ieq (Join-Path $installDir 'core\x64\chrono_hook.dll'))
            if (-not $holds) { Skip "the upgrade over a loaded library: the application does not hold chrono_hook.dll from the install folder ($(($modules | ForEach-Object { $_.FileName }) -join ', '))" }
        }
        catch { Skip "the upgrade over a loaded library: the modules of the application could not be read ($($_.Exception.Message))" }
    }
    # Held for real, asked of the file itself: a library loaded into a running program refuses a writer.
    if ($holds) {
        $locked = $false
        try { [System.IO.File]::Open((Join-Path $installDir 'core\x64\chrono_hook.dll'), 'Open', 'Write', 'None').Dispose() } catch [System.IO.IOException] { $locked = $true }
        Check $locked 'the injected library is locked by the running application (a writer is refused)'
        if (-not $locked) { $holds = $false }
    }
    $result = Invoke-Msi '/i' $new 'upgrade.log'
    # Exit 0 and 3010 are both an upgrade that worked: 3010 would say the restart is needed to finish it, 0 says
    # only a leftover waits for one. Which of the two is a measurement, said below, not an expectation.
    Check ($result.Code -in 0, 3010) "the upgrade over the held library installs (exit $($result.Code) in $($result.Seconds) s)"
    if ($holds) {
        Check (-not $held.HasExited) 'the application that held the library is still running - nothing ended it'
        $said = (Get-Content -LiteralPath $result.Log -Encoding Unicode -ErrorAction SilentlyContinue) -join "`n"
        if ($said -match 'chrono_hook\.dll is being held in use by') {
            Note 'the installer saw the library in use (its log says so) and went on without closing anything'
        }
        else {
            Skip 'the upgrade over a loaded library: the installer log does not say it saw the library in use, so the upgrade may not have met it'
        }
        $queued = @((Get-PendingSources) | Where-Object { $_ -like '*config.msi*' })
        Note "after the upgrade $($queued.Count) file(s) under Config.Msi wait for a restart to be deleted"
    }
    Test-Installed $kit $kit.new.version $treeB 'upgraded'
    $firstCode = @(Get-ArpEntries)[0].Code

    Say ''
    Say "== a rebuild of $($kit.same.version), the same version with other bytes and another product code"
    $result = Invoke-Msi '/i' $same 'rebuild.log'
    Check ($result.Code -in 0, 3010) "the rebuild installs over the first (exit $($result.Code))"
    Test-Installed $kit $kit.same.version $treeA 'rebuilt'
    $entries = @(Get-ArpEntries)
    Check ($entries.Count -eq 1 -and $entries[0].Code -ne $firstCode) "the rebuild replaced the first build instead of standing beside it ($firstCode -> $(($entries | ForEach-Object { $_.Code }) -join ', '))"

    Say ''
    Say "== $($kit.old.version) over $($kit.same.version)"
    $result = Invoke-Msi '/i' $old 'downgrade.log'
    Check ($result.Code -ne 0) "the older version is refused (exit $($result.Code))"
    $log = Get-Content -LiteralPath (Join-Path $script:LogFolder 'downgrade.log') -Raw -ErrorAction SilentlyContinue
    Check ($log -match [regex]::Escape("A newer version of $name is already installed")) 'and the log carries the sentence the package gives'
    Test-Installed $kit $kit.same.version $treeA 'after the refusal'

    Say ''
    Say '== uninstall, with a file of somebody else in the folder'
    Stop-Survivors
    # Taken here, after everything the window may have written during the run: what the uninstall must leave
    # alone is the folder as it is when the uninstall starts.
    $perUserBefore = Get-FileHashes $perUser
    Note "$perUser holds $($perUserBefore.Count) file(s) when the uninstall starts"
    $planted = Join-Path $installDir 'somebody-elses.txt'
    Set-Content -LiteralPath $planted -Value 'not ours'
    $installed = @(Get-ArpEntries)
    if ($installed.Count -ne 1) { throw "uninstall: $($installed.Count) entries in Programs and Features, so there is no one product code to remove" }
    $result = Invoke-Msi '/x' $installed[0].Code 'uninstall.log'
    Check ($result.Code -in 0, 3010) "the uninstall succeeds (exit $($result.Code))"
    Check (@(Get-ArpEntries).Count -eq 0) 'no entry is left in Programs and Features'
    $left = Get-FileHashes $installDir
    $pending = Get-PendingSources
    $strays = @($left.Keys | Where-Object { $_ -ne 'somebody-elses.txt' -and -not $pending.Contains((Join-Path $installDir $_).Replace('/', '\').ToLowerInvariant()) })
    Check ($left.ContainsKey('somebody-elses.txt') -and $strays.Count -eq 0) "the folder holds only the planted file ($($strays.Count) other$(if ($strays) { ', the first ' + $strays[0] }))"
    Check (@(Get-OursOnPath $installDir).Count -eq 0) 'nothing of ours is left on the machine PATH'
    Check (-not (Test-Path -LiteralPath $shortcut)) 'the Start menu entry is gone'
    Check (-not (Test-Path -LiteralPath $vendorKey)) "no registry key $vendorKey"
    Note "the publisher's key HKLM:\SOFTWARE\$($script:Product.publisher) is $(if (Test-Path -LiteralPath "HKLM:\SOFTWARE\$($script:Product.publisher)") { 'still there' } else { 'gone' })"
    $endPath = Get-MachinePath
    Check ($endPath.Value -ceq $startPath.Value) "the machine PATH is what it was before, character for character ($($endPath.Value.Length) against $($startPath.Value.Length))"
    if ($endPath.Kind -ne $startPath.Kind) { Note "the machine PATH was stored as $($startPath.Kind) and is now stored as $($endPath.Kind)" }
    $perUserAfter = Get-FileHashes $perUser
    $changed = @($perUserBefore.Keys | Where-Object { -not $perUserAfter.ContainsKey($_) -or $perUserAfter[$_] -ne $perUserBefore[$_] }) + @($perUserAfter.Keys | Where-Object { -not $perUserBefore.ContainsKey($_) })
    Check ($changed.Count -eq 0) "$perUser is as it was, byte for byte ($($perUserAfter.Count) file(s), $($changed.Count) changed)"
    $completed = $true
}
catch {
    # Said and counted, so the summary below is printed and the exit code is not the one of a crash.
    $script:Failed++
    Say "FAILED  the run stopped on an error: $($_.Exception.Message) ($($_.InvocationInfo.ScriptLineNumber))"
}
finally {
    Stop-Survivors
    foreach ($entry in @(Get-ArpEntries)) {
        Say "the product is still installed ($($entry.Code)) - uninstalling it, so this run leaves nothing behind"
        [void](Invoke-Msi '/x' $entry.Code ("restore-$($entry.Code.Trim('{}')).log"))
    }
    if (Test-Path -LiteralPath $installDir) { Remove-Item -LiteralPath $installDir -Recurse -Force -ErrorAction SilentlyContinue }
    if ($ownedPerUser -and (Test-Path -LiteralPath $perUser)) { Remove-Item -LiteralPath $perUser -Recurse -Force -ErrorAction SilentlyContinue }
    # What a broken installer left behind was reported above as a failure. It is also put back, so the next
    # run does not start on a machine this one dirtied - and a machine's PATH is put back to the text written
    # down at the start, not to a guess.
    if (Test-Path -LiteralPath $shortcut) { Remove-Item -LiteralPath $shortcut -Force -ErrorAction SilentlyContinue; Say 'cleaned: the Start menu entry the installer left' }
    if (Test-Path -LiteralPath $vendorKey) { Remove-Item -LiteralPath $vendorKey -Recurse -Force -ErrorAction SilentlyContinue; Say "cleaned: the registry key $vendorKey the installer left" }
    if ((Get-MachinePath).Value -cne $startPath.Value) { Restore-MachinePath $startPath; Say 'cleaned: the machine PATH put back to the text written down at the start' }
}

Say ''
Say "ok: $script:Passed, failed: $script:Failed, not measured: $script:NotMeasured$(if (-not $completed) { ' - the run did not reach its end' })"
if (-not $completed) { exit 1 }
if ($script:Failed -gt 0) { exit 1 }
if ($script:NotMeasured -gt 0) { exit 3 }
exit 0
