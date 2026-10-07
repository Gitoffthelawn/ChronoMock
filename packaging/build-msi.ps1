#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Build the Windows installer of one release from the window package.

.DESCRIPTION
    One installer carries the window and the command line, for every account on the machine (the owner's
    decision of 2026-10-06, beside the two zips, which stay as they are). It is built from the SIGNED
    window package, because the signature is on the programs inside it: an installer made before the card
    signed them would carry unsigned copies. So a release builds it in phase B (sign-release.ps1), on the
    machine with the card, and never in release.yml. The same script builds an unsigned one from
    dist/ChronoMock in ci.yml, so that what the installer DOES is asked on a clean machine.

    What it lays down is the window package plus three things, all added here and nowhere else:
      * calendars/ and presets/ beside each core, so `chrono` on PATH reads the installed catalogue and
        never one from the folder it happens to be started in (the lookup takes the folder beside the
        executable first, and a working directory second);
      * installed.txt beside ChronoMock.exe, the marker that tells the window the installer owns the
        folder, so it keeps its history and logs in %LOCALAPPDATA%\ChronoMock (AppPaths.InstalledMarker);
      * the installer's own README, because the package's one says "no installer".

    Every value comes from one place: the names and addresses from packaging/components.json, the version
    from the tag, the template from packaging/msi/. The built database is then asked what it carries - no
    custom action, no binary of WiX's own, the version, the upgrade code and the Restart Manager setting -
    so a template that grew an extension is refused here and not found on somebody's machine.

    Modes:
      -Payload <dir> -OutDir <dir>   build; the installer is written whole or not at all
      -Check                         whether this machine can build it (the template, the icon, WiX), so
                                     sign-release.ps1 can refuse before the card signs anything
      -SourceOnly                    print the rendered installer source and build nothing (the guards read it)
      -ProductName                   print the name the installer carries, which the signature names

    Exit codes: 0 done, 1 refused - the message says which input and why. Nothing here signs or publishes.

.PARAMETER Tag
    The release, e.g. v0.4.0. A tag with a hyphen is a release candidate and gets no installer.
#>
[CmdletBinding(DefaultParameterSetName = 'Build')]
param(
    [Parameter(Mandatory)] [string] $Tag,
    [Parameter(ParameterSetName = 'Build', Mandatory)] [string] $Payload,
    [Parameter(ParameterSetName = 'Build', Mandatory)] [string] $OutDir,
    [Parameter(ParameterSetName = 'Check', Mandatory)] [switch] $Check,
    [Parameter(ParameterSetName = 'Source', Mandatory)] [switch] $SourceOnly,
    [Parameter(ParameterSetName = 'Name', Mandatory)] [switch] $ProductName
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = Split-Path -Parent $PSScriptRoot
# A .wxs that WiX cannot read as it stands - every doubled-brace placeholder is filled first. Named .wxs so
# the hygiene scans that read every XML file in the repository read its comments too.
$templatePath = Join-Path $PSScriptRoot 'msi/chronomock.wxs'
$readmePath = Join-Path $PSScriptRoot 'msi-readme.md'
$registerPath = Join-Path $PSScriptRoot 'components.json'
$iconPath = Join-Path $root 'assets/chrono.ico'

if (-not (Test-Path -LiteralPath $registerPath)) { throw "build-msi: missing the register at $registerPath" }
$register = Get-Content -Raw -LiteralPath $registerPath | ConvertFrom-Json

# The file a person downloads, beside the two zips, named where every published file is named (the
# register's `msi` package). It sorts directly above the window package it is built from ('m' before 'z'),
# so a reader meets the installer first and the archive right after it.
$InstallerName = $register.packages.msi.zip
if ($InstallerName -notmatch '^[A-Za-z0-9.-]+\.msi$') {
    throw "build-msi: packaging/components.json names the installer '$InstallerName', which is not a file name ending in .msi"
}

# The product, not a build, and it never changes - see the comment on it in the template. Generated once on
# 2026-10-07 and pinned by crates/cli/tests/msi.rs, which holds a second copy that does not move with this one.
$UpgradeCode = '5F0CEAC5-75A7-4EE1-8C4B-B20E1012999D'

# The WiX this project builds with. Another version builds another package from the same source, so a
# different one is refused rather than used. 5.0.2 is the last release before the Open Source Maintenance
# Fee EULA of v6 and later (the owner's decision of 2026-10-07).
$WixVersion = '5.0.2'

# The marker AppPaths.InstalledMarker looks for. The two names are held together by crates/cli/tests/msi.rs.
$InstalledMarker = 'installed.txt'

# What the shortcut's tooltip and the refusal to go back a version say. The only two sentences a person reads
# from the installer itself.
$ShortcutDescription = 'Run any Windows application at a different date, without touching the system clock'
$DowngradeMessage = 'A newer version of [ProductName] is already installed. Uninstall it first to install this one.'

# What the window package must hold for the installer to be the program it claims to be. Relative to the
# folder that holds ChronoMock.exe.
$RequiredFiles = @(
    'ChronoMock.exe', 'ChronoMock.dll', 'ChronoMock.Protocol.dll',
    'core/x64/chrono.exe', 'core/x64/chrono_hook.dll',
    'core/x86/chrono.exe', 'core/x86/chrono_hook.dll',
    'LICENSE', 'THIRD-PARTY-NOTICES.md', 'README.md'
)
$CatalogueFolders = @('calendars', 'presets')
$CoreFolders = @('core/x64', 'core/x86')

# A refusal is one plain line on stderr and exit 1. Not a thrown exception: PowerShell's error view wraps a
# long message at the console width and frames it with the line of code, so the sentence a person or a
# guard reads was broken in two (measured, the first run of crates/cli/tests/msi.rs). `exit` from here still
# runs the `finally` that removes the staging folder.
function Stop-Build([string] $message) {
    [Console]::Error.WriteLine("build-msi: $message")
    exit 1
}

# The version a tag names, or a refusal saying why it gets no installer.
function Get-InstallerVersion([string] $releaseTag) {
    if ($releaseTag.Contains('-')) {
        Stop-Build ("$releaseTag is a release candidate, and candidates get no installer. Windows Installer reads " +
            "only the three numbers, so it would take $($releaseTag.Split('-')[0]) for the release itself and the " +
            'release would not replace it on a machine that took the candidate')
    }
    if ($releaseTag -notmatch '^v(\d+)\.(\d+)\.(\d+)$') {
        Stop-Build "'$releaseTag' is not a release tag. Pass it the way the release is tagged, for example v0.4.0"
    }
    $major = [int]$Matches[1]
    $minor = [int]$Matches[2]
    $patch = [int]$Matches[3]
    if ($major -gt 255 -or $minor -gt 255 -or $patch -gt 65535) {
        Stop-Build ("$releaseTag does not fit an installer version, which holds at most 255 for the first two " +
            'numbers and 65535 for the third')
    }
    return "$major.$minor.$patch"
}

# Every placeholder the template may use, from the one place each value lives.
function Get-TemplateValues([string] $version, [string] $releaseTag) {
    $product = $register.product
    # The register names the supplier the way SPDX does ("Person: X" or "Organization: X"). The installer
    # shows only the name.
    $publisher = ($product.supplier -split ':\s*', 2)[-1]
    foreach ($field in @($product.name, $publisher, $product.homepage, $product.source)) {
        if (-not $field) { Stop-Build "packaging/components.json is missing a product field the installer names" }
    }
    return [ordered]@{
        APP_NAME             = $product.name
        PUBLISHER            = $publisher
        VERSION              = $version
        UPGRADE_CODE         = $UpgradeCode
        PROJECT_URL          = $product.homepage
        ISSUES_URL           = "$($product.source)/issues"
        RELEASE_NOTES_URL    = "$($product.source)/releases/tag/$releaseTag"
        SHORTCUT_DESCRIPTION = $ShortcutDescription
        DOWNGRADE_MESSAGE    = $DowngradeMessage
    }
}

# The installer source for one release, every placeholder filled - and a template that asks for a value
# nobody gives, or no longer asks for one that is given, is refused rather than built half right.
function Get-InstallerSource([string] $version, [string] $releaseTag) {
    if (-not (Test-Path -LiteralPath $templatePath)) {
        Stop-Build "there is no template at $templatePath. A tree from before the installer existed has none"
    }
    $text = [System.IO.File]::ReadAllText($templatePath)
    $values = Get-TemplateValues $version $releaseTag
    $asked = @([regex]::Matches($text, '\{\{([A-Z_]+)\}\}') | ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique)
    $unknown = @($asked | Where-Object { -not $values.Contains($_) })
    if ($unknown) { Stop-Build "the template asks for $($unknown -join ', '), which build-msi.ps1 does not give" }
    $unused = @($values.Keys | Where-Object { $asked -notcontains $_ })
    if ($unused) { Stop-Build "the template uses no $($unused -join ', ') any more - take it out of build-msi.ps1" }
    foreach ($key in $values.Keys) {
        # Escaped for an XML attribute, which is where every placeholder stands.
        $text = $text.Replace("{{$key}}", [System.Security.SecurityElement]::Escape([string]$values[$key]))
    }
    return $text
}

# The wix command at the pinned version, or a refusal with the command that installs it.
function Find-Wix {
    $install = "    dotnet tool install --global wix --version $WixVersion`n" +
        'It runs on .NET - install the .NET SDK first if there is no dotnet command.'
    # Where `dotnet tool install --global` puts it when that folder is not on PATH. From the environment
    # rather than $HOME, which PowerShell takes from the account and not from USERPROFILE.
    $found = (Get-Command wix -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1)
    $path = if ($found) { $found.Source } elseif ($env:USERPROFILE) { Join-Path $env:USERPROFILE '.dotnet\tools\wix.exe' } else { '' }
    if (-not $path -or -not (Test-Path -LiteralPath $path)) {
        Stop-Build "WiX is not installed, and the installer is built with it. Install the version this project builds with:`n$install"
    }
    $said = (& $path --version 2>&1 | Out-String).Trim()
    $exitCode = $LASTEXITCODE
    $reported = ($said -split '\+', 2)[0]
    if ($exitCode -ne 0 -or $reported -ne $WixVersion) {
        Stop-Build ("$path says it is version '$reported', and the installer is built with $WixVersion - another " +
            "version builds another package from the same source. Replace it:`n    dotnet tool uninstall --global wix`n$install")
    }
    return $path
}

# The window package this installer is built from, refused when it is not the package a release builds -
# or when it is one somebody has run, because a run leaves that person's history and logs beside the exe
# and an installer would hand them to every machine it reaches.
function Assert-Payload([string] $folder) {
    if (-not (Test-Path -LiteralPath $folder -PathType Container)) {
        Stop-Build "-Payload $folder is not a folder. Pass the folder that holds ChronoMock.exe"
    }
    foreach ($relative in $RequiredFiles) {
        if (-not (Test-Path -LiteralPath (Join-Path $folder $relative) -PathType Leaf)) {
            Stop-Build "$folder holds no $relative, so it is not the window package a release builds"
        }
    }
    foreach ($catalogue in $CatalogueFolders) {
        if (-not (Test-Path -LiteralPath (Join-Path $folder $catalogue) -PathType Container)) {
            Stop-Build "$folder holds no $catalogue folder, so it is not the window package a release builds"
        }
    }
    if (Test-Path -LiteralPath (Join-Path $folder $InstalledMarker)) {
        Stop-Build "$folder already holds $InstalledMarker - it is an installed copy, not the package a release builds"
    }
    foreach ($own in @('history', 'logs')) {
        if (Test-Path -LiteralPath (Join-Path $folder $own)) {
            Stop-Build ("$folder holds a $own folder, which a window run from it wrote. That is somebody's own data, " +
                'and an installer built from it would carry it to every machine - build from a fresh package')
        }
    }
}

# The window package laid out the way the installer puts it down - all of it, or a refusal.
# 🔴 Enumerated literally, hidden files included. `Copy-Item -Path <folder>\*` read the folder as a pattern:
# a package in a folder whose name holds brackets copied NOTHING and said nothing, and a hidden file at the
# top was left out (measured 2026-10-07, review of #96). The database check counts what was staged, so an
# incomplete staging would have built and passed - hence the check right after the copy, against the package.
function New-StagedPayload([string] $from, [string] $into) {
    $source = (Get-Item -LiteralPath $from).FullName.TrimEnd('\', '/')
    Get-ChildItem -LiteralPath $source -Force | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination $into -Recurse -Force
    }
    $left = @(Get-ChildItem -LiteralPath $source -Recurse -File -Force |
            ForEach-Object { $_.FullName.Substring($source.Length + 1) } |
            Where-Object { -not (Test-Path -LiteralPath (Join-Path $into $_) -PathType Leaf) })
    if ($left) {
        Stop-Build "staging left out $($left.Count) file(s) of the package, the first $($left[0]) - nothing was built"
    }
    foreach ($core in $CoreFolders) {
        foreach ($catalogue in $CatalogueFolders) {
            Copy-Item -LiteralPath (Join-Path $from $catalogue) -Destination (Join-Path $into $core) -Recurse
        }
    }
    if (-not (Test-Path -LiteralPath $readmePath)) { Stop-Build "missing the installer's README at $readmePath" }
    Copy-Item -LiteralPath $readmePath -Destination (Join-Path $into 'README.md') -Force
    $marker = "This folder was installed by the Chrono Mock installer, which owns it. The window keeps its session`n" +
        "history and diagnostics logs in %LOCALAPPDATA%\ChronoMock instead. Leave this file here - without it the`n" +
        "window treats the folder as a portable copy. Uninstall from Settings, Apps.`n"
    [System.IO.File]::WriteAllText((Join-Path $into $InstalledMarker), $marker, (New-Object System.Text.UTF8Encoding($false)))
}

# The rows one query returns from an installer database, each written to the pipeline as one array of its
# fields, so a caller collects them with @(). A table the database does not have answers with no rows,
# which is the answer for "no custom action" as well.
# 🔴 Every COM call that returns nothing is cast to [void]. Uncast, Execute and Close each wrote a $null to
# the pipeline, and the caller indexed into it - measured on the first build of this script.
# 🔴 And every COM object is released, the records too: a view or a record left to the garbage collector
# keeps the database open, and the installer could not be moved out of staging ("being used by another
# process" - measured, after every check had passed).
# Released on every path, the error ones too: the installer object and the database are made inside the
# `try` whose `finally` releases them, and each record in a `finally` of its own (review of #96).
function Read-MsiRows([string] $msi, [string] $query, [int] $fields) {
    $installer = $null
    $database = $null
    $view = $null
    try {
        $installer = New-Object -ComObject WindowsInstaller.Installer
        $database = $installer.OpenDatabase($msi, 0)
        try {
            $view = $database.OpenView($query)
        }
        catch {
            return
        }
        [void]$view.Execute()
        while ($true) {
            $record = $view.Fetch()
            if ($null -eq $record) { break }
            try {
                $row = [string[]]::new($fields)
                for ($i = 0; $i -lt $fields; $i++) { $row[$i] = $record.StringData($i + 1) }
            }
            finally {
                [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($record)
            }
            Write-Output -NoEnumerate $row
        }
        [void]$view.Close()
    }
    finally {
        foreach ($held in @($view, $database, $installer)) {
            if ($null -ne $held) { [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($held) }
        }
        [System.GC]::Collect()
        [System.GC]::WaitForPendingFinalizers()
    }
}

# The built database, asked what it carries - before it is moved where anybody would find it.
function Assert-Database([string] $msi, [string] $version, [int] $fileCount) {
    $properties = @{}
    foreach ($row in @(Read-MsiRows $msi 'SELECT `Property`, `Value` FROM `Property`' 2)) { $properties[$row[0]] = $row[1] }
    if ($properties.Count -lt 5) {
        Stop-Build "read $($properties.Count) properties out of the built installer, so this check is not reading it"
    }
    $expected = [ordered]@{
        ProductVersion           = $version
        UpgradeCode              = "{$UpgradeCode}"
        MSIRESTARTMANAGERCONTROL = 'Disable'
        ARPNOREPAIR              = '1'
        ARPNOMODIFY              = '1'
    }
    foreach ($name in $expected.Keys) {
        if ($properties[$name] -ne $expected[$name]) {
            Stop-Build "the built installer says $name = '$($properties[$name])', and it has to say '$($expected[$name])'"
        }
    }
    $actions = @(Read-MsiRows $msi 'SELECT `Action` FROM `CustomAction`' 1)
    if ($actions.Count -gt 0) {
        Stop-Build "the built installer carries custom actions ($(($actions | ForEach-Object { $_[0] }) -join ', ')). It runs no code of its own or of WiX's"
    }
    $binaries = @(Read-MsiRows $msi 'SELECT `Name` FROM `Binary`' 1)
    if ($binaries.Count -gt 0) {
        Stop-Build "the built installer embeds binaries ($(($binaries | ForEach-Object { $_[0] }) -join ', ')). It carries our files and nothing of WiX's"
    }
    $files = @(Read-MsiRows $msi 'SELECT `File` FROM `File`' 1)
    if ($files.Count -ne $fileCount) {
        Stop-Build "the built installer lays down $($files.Count) files and the staged package holds $fileCount"
    }
}

# --- the modes --------------------------------------------------------------------------------------

$version = Get-InstallerVersion $Tag

if ($SourceOnly) {
    [Console]::Out.Write((Get-InstallerSource $version $Tag))
    return
}
if ($ProductName) {
    Write-Output (Get-TemplateValues $version $Tag).APP_NAME
    return
}

$source = Get-InstallerSource $version $Tag
if (-not (Test-Path -LiteralPath $iconPath)) { Stop-Build "the icon $iconPath is not in the tree" }
if (-not (Test-Path -LiteralPath $readmePath)) { Stop-Build "missing the installer's README at $readmePath" }

if ($Check) {
    Write-Output "ready to build $InstallerName $version with WiX $WixVersion ($(Find-Wix))"
    return
}

# Every refusal about the inputs - the staged copy being whole among them - comes before WiX is looked for,
# so the guards can ask each of them on a machine without WiX, and a run with WiX missing stops at the same
# place on every system.
Assert-Payload $Payload
if (-not (Test-Path -LiteralPath $OutDir -PathType Container)) { Stop-Build "-OutDir $OutDir is not a folder. Pass one that exists" }
$target = Join-Path $OutDir $InstallerName
if (Test-Path -LiteralPath $target) {
    Stop-Build "$target is already there. Nothing is overwritten - remove it or pass another -OutDir"
}

# Beside nothing of the caller's: sign-release.ps1 hands its own folder of files to publish as -OutDir, and
# a folder left inside it would be published, or would break the checksums written over it.
$staging = Join-Path ([System.IO.Path]::GetTempPath()) ("chrono-msi-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $staging | Out-Null
try {
    $staged = Join-Path $staging 'payload'
    New-Item -ItemType Directory -Path $staged | Out-Null
    New-StagedPayload $Payload $staged
    $fileCount = @(Get-ChildItem -LiteralPath $staged -Recurse -File -Force).Count
    $wix = Find-Wix

    $wxs = Join-Path $staging 'chronomock.wxs'
    [System.IO.File]::WriteAllText($wxs, $source, (New-Object System.Text.UTF8Encoding($false)))
    $built = Join-Path $staging $InstallerName
    $arguments = @('build', $wxs, '-arch', 'x64', '-pdbtype', 'none',
        '-d', "PayloadDir=$staged", '-d', "IconFile=$iconPath", '-o', $built)
    Write-Host "    `$ wix $($arguments -join ' ')"
    & $wix @arguments
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $built)) {
        Stop-Build "wix build exited $LASTEXITCODE, and nothing was written to $OutDir. What it said is above"
    }
    Assert-Database $built $version $fileCount
    # Moved in only once it is whole and checked, so a run that fails or is stopped leaves no half written
    # installer where the caller would find it.
    Move-Item -LiteralPath $built -Destination $target
}
finally {
    Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue
    # Said rather than swallowed: a staging folder that could not go keeps a copy of the whole package
    # (~170 MB) in the temp folder, and nobody would know it was there.
    if (Test-Path -LiteralPath $staging) {
        Write-Warning "build-msi: could not remove the staging folder $staging - delete it by hand"
    }
}
Write-Output $target
