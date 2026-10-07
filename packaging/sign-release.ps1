#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Phase B: sign a release build with the card, then hand it back to the workflow.

.DESCRIPTION
    The signing key lives on a cryptographic card in a USB reader and cannot be exported - that
    is the whole value of it, so no GitHub-hosted runner will ever reach it. A self-hosted
    runner could, and this is a PUBLIC repository, where a self-hosted runner is a machine
    strangers can aim a pull request at. So the build happens in a workflow and the signature
    happens here, and this script is the seam between them.

    In order, and what it refuses at each step:

      1. downloads the unsigned build phase A produced for this tag;
      2. VERIFIES that build's provenance attestation before touching it - signing something you
         did not check is how a supply chain gets a signature on it;
      3. signs OUR binaries, and only ours, with an RFC 3161 timestamp. Without a timestamp the
         signature dies when the certificate expires, and this one is valid for a year;
      4. reads the certificate back OUT of each signed file and refuses to go on unless it hashes
         to the pin in packaging/codesign.json. A second code-signing certificate on the same
         machine - a renewal, a test one, one from another project - is exactly this accident;
      5. repacks both archives, builds the Windows installer from the signed window package and signs
         it the same way (never for a release candidate - see Test-Candidate), regenerates the bills of
         materials over the SIGNED bytes, and writes SHA256SUMS over what will actually ship. Whether
         the installer CAN be built here is asked before anything else, so a machine without WiX stops
         before the card has signed anything;
      6. uploads everything to the DRAFT release and asks phase C to attest the signed bytes;
      7. waits for that and confirms the draft is COMPLETE - a draft missing one file looks almost
         exactly like a finished one.

    Nothing here publishes. The release stays a draft until a person reads it and presses the
    button, and pressing it runs phase D, which re-checks the published page the way a user would.

    🔴 BE AT THE MACHINE. signtool reaches the card and then waits for its PIN, so this script
    cannot run unattended - measured, by watching it block on exactly that. Whether the card asks
    once or once per file depends on the card middleware's own PIN caching, and there are eleven
    files plus the installer, so watch the first run before assuming. Each signature prints its file, so a run that
    has stopped is easy to tell from one that is working.

.PARAMETER Tag
    The release tag, e.g. v0.2.0.

.PARAMETER ListCertificates
    Print every code-signing certificate in the store with its subject, fingerprints and expiry,
    and do nothing else. This is how the pin in packaging/codesign.json is set, and how the
    holder sees exactly which personal details a signature would make public.

.PARAMETER DryRun
    Everything except signing, uploading and dispatching.

.PARAMETER Wait
    How long to wait for phase C to attach its bundles, in seconds.
#>
[CmdletBinding()]
param(
    [Parameter(Position = 0)] [string] $Tag,
    [switch] $ListCertificates,
    [switch] $DryRun,
    [int] $Wait = 300,
    [string] $Work
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = Split-Path -Parent $PSScriptRoot
$repo = 'donislawdev/ChronoMock'
$attestWorkflow = 'attest-signed.yml'
if (-not $Work) { $Work = Join-Path $root 'build/signing' }

# The Enhanced Key Usage OID for code signing. Matched by OID and never by the friendly name,
# because the friendly name is LOCALISED - on a Polish Windows the same certificate reads
# "Podpisywanie kodu", and a filter written against "Code Signing" reports an empty store. That
# false negative is not hypothetical: it happened while writing this script.
$CODE_SIGNING_OID = '1.3.6.1.5.5.7.3.3'

# 🔴 Exactly the binaries WE build, per archive, by path inside the zip. Not a glob.
#
# The window package ships around 240 assemblies that Microsoft already signed, and re-signing
# somebody else's binary with our certificate would both destroy their signature and assert that we
# produced it. Two more files in there are third-party and unsigned by their own publisher
# (Wpf.Ui.dll and Wpf.Ui.Abstractions.dll) - they stay unsigned for the same reason. The bill of
# materials declares them, so a reader who notices can see what they are.
$OURS = @{
    'ChronoMock-app-win-x64.zip' = @(
        'ChronoMock/ChronoMock.exe',
        'ChronoMock/ChronoMock.dll',
        'ChronoMock/ChronoMock.Protocol.dll',
        'ChronoMock/core/x64/chrono.exe',
        'ChronoMock/core/x64/chrono_hook.dll',
        'ChronoMock/core/x86/chrono.exe',
        'ChronoMock/core/x86/chrono_hook.dll'
    )
    'ChronoMock-cli-win.zip'     = @(
        'chrono-cli/chrono.exe',
        'chrono-cli/chrono_hook.dll',
        'chrono-cli/x86/chrono.exe',
        'chrono-cli/x86/chrono_hook.dll'
    )
}

# Which package id in packaging/components.json each archive is, for regenerating the SBOM.
$PACKAGE_ID = @{
    'ChronoMock-app-win-x64.zip' = 'gui'
    'ChronoMock-cli-win.zip'     = 'cli'
}

# What a complete draft carries. A missing one of these is a phase that did not finish. The installer and
# its SBOM join the list below for a release, and never for a candidate.
$EXPECTED_ASSETS = @('ChronoMock-app-win-x64.zip', 'ChronoMock-cli-win.zip',
    'ChronoMock-app-win-x64.zip.spdx.json', 'ChronoMock-cli-win.zip.spdx.json', 'SHA256SUMS')

# The Windows installer, built in step 5 from the SIGNED window package and signed like the programs inside
# it - an installer built before the card would carry unsigned copies. Named where every published file is
# named, the register's `msi` package (ADR-23).
$WINDOW_ARCHIVE = 'ChronoMock-app-win-x64.zip'
$BUILD_MSI = Join-Path $PSScriptRoot 'build-msi.ps1'
$INSTALLER = (Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot 'components.json') | ConvertFrom-Json).packages.msi.zip
# What the installer is built from besides the signed package. Phase A built that package from the TAG, and
# this script runs from whatever the checkout stands on, so these have to be the tag's own: a template changed
# on main after the tag would otherwise ship in a release that never carried it.
$INSTALLER_INPUTS = @('packaging/msi', 'packaging/build-msi.ps1', 'packaging/msi-readme.md',
    'packaging/components.json', 'assets/chrono.ico')

# 🔴 One rule for a release candidate, the one build-msi.ps1 refuses by: a hyphen in the tag. Windows
# Installer reads only three numbers, so a candidate's installer would take the release's own version and
# the release could not replace it on the machine of whoever tested the candidate.
function Test-Candidate([string] $releaseTag) {
    return $releaseTag.Contains('-')
}

$WARN_DAYS = 90

function Invoke-Step([string[]] $Command) {
    Write-Host "  `$ $($Command -join ' ')"
    $output = & $Command[0] @($Command[1..($Command.Length - 1)]) 2>&1
    if ($LASTEXITCODE -ne 0) {
        $output | ForEach-Object { Write-Host $_ }
        throw "sign-release: '$($Command[0])' failed with exit $LASTEXITCODE"
    }
    return $output
}

function Get-CodeSigningCertificates {
    # Wrapped in @() at both levels on purpose. Under Set-StrictMode a certificate carrying no
    # enhanced key usage at all makes a bare property walk throw rather than return nothing, and the
    # store here holds several of those - so the first version of this line failed on the store
    # itself rather than on any certificate in it.
    Get-ChildItem Cert:\CurrentUser\My, Cert:\LocalMachine\My -ErrorAction SilentlyContinue |
        Where-Object { @($_.EnhancedKeyUsageList | ForEach-Object { $_.ObjectId }) -contains $CODE_SIGNING_OID }
}

function Get-CertificateSha256($certificate) {
    (([System.Security.Cryptography.SHA256]::Create().ComputeHash($certificate.RawData) |
                ForEach-Object { $_.ToString('x2') }) -join '')
}

function Find-SignTool {
    $kits = 'C:\Program Files (x86)\Windows Kits\10\bin'
    $found = @()
    if (Test-Path -LiteralPath $kits) {
        foreach ($version in (Get-ChildItem -LiteralPath $kits -Directory | Sort-Object Name)) {
            $candidate = Join-Path $version.FullName 'x64\signtool.exe'
            if (Test-Path -LiteralPath $candidate) { $found += $candidate }
        }
    }
    if (-not $found) {
        throw ("sign-release: no signtool.exe under $kits - install the Windows SDK " +
            "('Windows SDK Signing Tools' is enough)")
    }
    return $found[-1]
}

# 🔴 The certificate expires on a known date, and a script that merely PRINTS that date draws no
# conclusion from it. The first release after expiry would then fail in the middle of the ritual, at
# the signing step, with the card already in the reader - the worst moment to learn of a certificate
# problem. Pure, so `now` is passed in and the logic can be checked without a card.
function Get-ExpiryNotice($notAfter, [datetime] $now) {
    if (-not $notAfter) { return @('  🔴 the store reported no expiry date - check the card by hand') }
    $when = [datetime]$notAfter
    $days = [int]($when - $now).TotalDays
    if ($days -lt 0) {
        throw ("sign-release: the pinned certificate EXPIRED $(-$days) days ago ($($when.ToString('yyyy-MM-dd'))).`n" +
            "Signing with it now produces a signature Windows will reject. Renew the certificate, then move`n" +
            "certificate_sha256 in packaging/codesign.json to the NEW one - a renewal is a different`n" +
            "certificate, not the same one with a later date.")
    }
    if ($days -le $WARN_DAYS) {
        return @("  🔴 WARNING: $days days left on this certificate ($($when.ToString('yyyy-MM-dd'))).",
            "     Renewing issues a NEW certificate, so certificate_sha256 in packaging/codesign.json",
            "     has to move with it or the next release refuses to sign at all.")
    }
    return @("  $days days left on the certificate")
}

function Get-Pin {
    $path = Join-Path $PSScriptRoot 'codesign.json'
    if (-not (Test-Path -LiteralPath $path)) { throw "sign-release: missing the pin at $path" }
    $pin = Get-Content -Raw -LiteralPath $path | ConvertFrom-Json
    if (-not $pin.certificate_sha256 -or $pin.certificate_sha256 -notmatch '^[0-9a-f]{64}$') {
        throw ("sign-release: packaging/codesign.json has no usable certificate_sha256. Run this script " +
            "with -ListCertificates, find the card's certificate and paste its sha256 there.")
    }
    if (-not $pin.timestamp_url) { throw 'sign-release: packaging/codesign.json has no timestamp_url' }
    return $pin
}

function Assert-Path([string] $path, [string] $why) {
    if (-not (Test-Path -LiteralPath $path)) { throw "sign-release: missing '$path' - $why" }
}

# 🔴 Read the certificate back OUT of a signed file. A second code-signing certificate on this machine would
# sign just as willingly and the release page would look identical. One check for the programs and for the
# installer, so the two cannot drift apart.
function Assert-SignedByPin([string] $file, [string] $label, [string] $pinned) {
    $signature = Get-AuthenticodeSignature -LiteralPath $file
    if ($signature.Status -ne 'Valid') {
        throw "sign-release: $label came back with signature status $($signature.Status). Nothing has been uploaded."
    }
    $actual = Get-CertificateSha256 $signature.SignerCertificate
    if ($actual -ne $pinned) {
        throw ("sign-release: $label was signed by a DIFFERENT certificate.`n" +
            "  expected $pinned`n  got      $actual`nNothing has been uploaded.")
    }
    if (-not $signature.TimeStamperCertificate) {
        throw ("sign-release: $label carries no timestamp. Without one the signature dies with the " +
            "certificate. Nothing has been uploaded.")
    }
}

function Get-Sha256([string] $path) {
    (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
}

# Every binary in a directory tree with the state of its Authenticode signature. Used before and
# after signing: the difference has to be exactly the files we meant to sign, which is what catches
# a path list that reached further than it should.
function Get-SignatureStates([string] $directory) {
    $states = @{}
    foreach ($file in Get-ChildItem -LiteralPath $directory -Recurse -Include *.exe, *.dll -File) {
        $relative = $file.FullName.Substring($directory.Length).TrimStart('\', '/') -replace '\\', '/'
        $states[$relative] = (Get-AuthenticodeSignature -LiteralPath $file.FullName).Status.ToString()
    }
    return $states
}

# ---------------------------------------------------------------------------------------------
# -ListCertificates: the only mode that touches nothing
# ---------------------------------------------------------------------------------------------

if ($ListCertificates) {
    $pinned = $null
    $path = Join-Path $PSScriptRoot 'codesign.json'
    if (Test-Path -LiteralPath $path) {
        $pinned = (Get-Content -Raw -LiteralPath $path | ConvertFrom-Json).certificate_sha256
    }
    $certificates = @(Get-CodeSigningCertificates)
    if (-not $certificates) {
        Write-Host 'No code-signing certificate in the Windows store.'
        Write-Host 'Plug in the card reader and check the card middleware can see the card.'
        Write-Host "Matched by the code-signing OID $CODE_SIGNING_OID rather than by name, because the"
        Write-Host 'friendly name is localised and a name filter reports an empty store on a localised Windows.'
        return
    }
    Write-Host ''
    Write-Host '🔴 A certificate issued to an individual carries the holder name, town and province in its'
    Write-Host '   subject, and every signed file carries that with it. The first signed release makes it'
    Write-Host '   public and nothing takes it back. Read the subject below before signing anything.'
    Write-Host ''
    foreach ($certificate in $certificates) {
        $sha256 = Get-CertificateSha256 $certificate
        Write-Host "subject     : $($certificate.Subject)"
        Write-Host "issuer      : $($certificate.Issuer)"
        Write-Host "sha1 thumb  : $($certificate.Thumbprint)   (what signtool selects by)"
        Write-Host "sha256      : $sha256   (what packaging/codesign.json pins)"
        Write-Host "valid       : $($certificate.NotBefore.ToString('yyyy-MM-dd')) .. $($certificate.NotAfter.ToString('yyyy-MM-dd'))"
        Get-ExpiryNotice $certificate.NotAfter ([datetime]::Now) | ForEach-Object { Write-Host $_ }
        Write-Host "private key : $($certificate.HasPrivateKey)"
        if ($pinned -and $pinned -eq $sha256) { Write-Host '   THIS IS THE PINNED ONE' }
        elseif ($pinned) { Write-Host '   not the pinned certificate' }
        else { Write-Host '   nothing is pinned yet - paste the sha256 above into packaging/codesign.json' }
        Write-Host ''
    }
    return
}

# ---------------------------------------------------------------------------------------------
# The ritual
# ---------------------------------------------------------------------------------------------

if (-not $Tag) { throw 'sign-release: give the tag, e.g. ./packaging/sign-release.ps1 v0.2.0' }
if (-not $IsWindows) { throw 'sign-release: the card lives on Windows, run this there' }

$work = Join-Path $Work $Tag
if (Test-Path -LiteralPath $work) { Remove-Item -LiteralPath $work -Recurse -Force }
New-Item -ItemType Directory -Path $work -Force | Out-Null
Write-Host "working in $work"

$withInstaller = -not (Test-Candidate $Tag)
$expectedAssets = @($EXPECTED_ASSETS)
if ($withInstaller) { $expectedAssets += @($INSTALLER, "$INSTALLER.spdx.json") }

# Before anything is downloaded or signed: a machine that cannot build the installer would otherwise stop at
# step 5, with the programs already signed and a draft that can never get its installer from this run.
Write-Host "`n[0/8] whether the installer can be built here"
if ($withInstaller) {
    git rev-parse --verify --quiet "refs/tags/$Tag" | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw ("sign-release: the tag $Tag is not in this checkout, so the installer's inputs cannot be compared " +
            'with it. Run git fetch --tags')
    }
    git diff --quiet "refs/tags/$Tag" -- @INSTALLER_INPUTS
    $diffCode = $LASTEXITCODE
    if ($diffCode -eq 1) {
        throw ("sign-release: the installer's inputs in this checkout differ from $Tag " +
            "($($INSTALLER_INPUTS -join ', ')), so the installer would not be the one the tag describes. " +
            "Run this from the tag: git switch --detach $Tag")
    }
    if ($diffCode -ne 0) { throw "sign-release: git diff against $Tag failed with exit $diffCode" }
    Invoke-Step @('pwsh', '-NoProfile', '-File', $BUILD_MSI, '-Tag', $Tag, '-Check') | ForEach-Object { Write-Host "  $_" }
}
else {
    Write-Host "  $Tag is a release candidate, and candidates get no installer"
}

Write-Host "`n[1/8] fetching the build this tag produced"
Invoke-Step @('gh', 'run', 'download', '--repo', $repo, '--name', "unsigned-build-$Tag", '--dir', $work) | Out-Null
$archives = @(Get-ChildItem -LiteralPath $work -Filter *.zip -File)
if ($archives.Count -ne $OURS.Count) {
    throw "sign-release: expected $($OURS.Count) archives in the artefact, got $($archives.Name -join ', ')"
}

Write-Host "`n[2/8] verifying what the workflow says it built"
foreach ($archive in $archives) {
    Invoke-Step @('gh', 'attestation', 'verify', $archive.FullName, '--repo', $repo) | Out-Null
}

Write-Host "`n[3/8] unpacking"
$unpacked = @{}
$before = @{}
foreach ($archive in $archives) {
    if (-not $OURS.ContainsKey($archive.Name)) {
        throw "sign-release: the artefact carries '$($archive.Name)', which this script has no signing list for"
    }
    $target = Join-Path $work ("unpacked/" + [System.IO.Path]::GetFileNameWithoutExtension($archive.Name))
    Expand-Archive -LiteralPath $archive.FullName -DestinationPath $target -Force
    $unpacked[$archive.Name] = $target
    $before[$archive.Name] = Get-SignatureStates $target
    foreach ($relative in $OURS[$archive.Name]) {
        Assert-Path (Join-Path $target $relative) 'the signing list names a file this archive does not carry'
    }
    Write-Host ("  {0}: {1} binaries, {2} of them ours" -f $archive.Name,
        $before[$archive.Name].Count, $OURS[$archive.Name].Count)
}

Write-Host "`n[4/8] signing with the card"
$pin = Get-Pin
$certificate = Get-CodeSigningCertificates | Where-Object { (Get-CertificateSha256 $_) -eq $pin.certificate_sha256 } | Select-Object -First 1
if (-not $certificate) {
    throw ("sign-release: the pinned certificate ($($pin.certificate_sha256.Substring(0, 16))...) is not in the " +
        "Windows store. Plug in the card reader and check the middleware sees the card. If the certificate " +
        "was renewed, certificate_sha256 in packaging/codesign.json has to move with it - run this script " +
        "with -ListCertificates to see what is there.")
}
Write-Host "  certificate: $(($certificate.Subject -split ',')[0])"
Get-ExpiryNotice $certificate.NotAfter ([datetime]::Now) | ForEach-Object { Write-Host $_ }
$signtool = Find-SignTool
Write-Host "  signtool: $signtool"

foreach ($archive in $archives) {
    foreach ($relative in $OURS[$archive.Name]) {
        $file = Join-Path $unpacked[$archive.Name] $relative
        if ($DryRun) {
            Write-Host "  DRY RUN, would sign $relative"
            continue
        }
        Invoke-Step @($signtool, 'sign', '/sha1', $certificate.Thumbprint, '/fd', 'sha256',
            '/tr', $pin.timestamp_url, '/td', 'sha256', '/q', $file) | Out-Null
        Assert-SignedByPin $file $relative $pin.certificate_sha256
    }
    if (-not $DryRun) {
        # Exactly the files we meant to sign changed state, and nobody else's signature broke.
        # 🔴 The comparison list is $OURS VERBATIM. Get-SignatureStates keys relative to the
        # directory the archive was expanded INTO, and each archive carries its own top folder, so
        # its keys have exactly the shape $OURS uses to find these files a few lines above - which
        # Assert-Path already proved when it resolved every one of them. Deriving a second shape by
        # stripping that folder is what broke the first signing run: measured, the stripped list
        # matched 0 of 7 keys in the window archive and 0 of 4 in the command line one, so every
        # file that had just been signed looked like one we should never have touched. A check that
        # rebuilds the value it is checking against agrees with its own arithmetic and with nothing
        # on disk. Note that -DryRun skips this whole block, so a dry run could not have caught it.
        $after = Get-SignatureStates $unpacked[$archive.Name]
        $changed = @($after.Keys | Where-Object { $after[$_] -ne $before[$archive.Name][$_] })
        $expected = @($OURS[$archive.Name])
        $unexpected = @($changed | Where-Object { $expected -notcontains $_ })
        if ($unexpected) {
            throw ("sign-release: signing changed files it should not have touched in $($archive.Name): " +
                ($unexpected -join ', '))
        }
        $broken = @($after.Keys | Where-Object { $before[$archive.Name][$_] -eq 'Valid' -and $after[$_] -ne 'Valid' })
        if ($broken) {
            throw "sign-release: signing broke somebody else's signature in $($archive.Name): $($broken -join ', ')"
        }
        Write-Host ("  {0}: {1} files signed by the pinned certificate, timestamped, nothing else touched" -f
            $archive.Name, $OURS[$archive.Name].Count)
    }
}

if ($DryRun) {
    if ($withInstaller) {
        # From the UNSIGNED package, only to show the installer builds here. It goes with the work folder.
        Invoke-Step @('pwsh', '-NoProfile', '-File', $BUILD_MSI, '-Tag', $Tag,
            '-Payload', (Join-Path $unpacked[$WINDOW_ARCHIVE] 'ChronoMock'), '-OutDir', $work) | Out-Null
        Assert-Path (Join-Path $work $INSTALLER) 'build-msi.ps1 reported success and wrote no installer'
        Write-Host "  DRY RUN, would sign $INSTALLER (built here from the unsigned package)"
    }
    Write-Host "`ndry run finished - nothing was signed, uploaded or published"
    return
}

Write-Host "`n[5/8] repacking"
$shipped = @()
foreach ($archive in $archives) {
    Remove-Item -LiteralPath $archive.FullName -Force
    $source = Join-Path $unpacked[$archive.Name] '*'
    Compress-Archive -Path $source -DestinationPath $archive.FullName -Force
    $shipped += $archive.FullName
    Write-Host ("  {0}  {1}" -f $archive.Name, (Get-Sha256 $archive.FullName))
}
# What phase C attests and what the draft is checked against at the end: the archives, and the installer.
$subjects = @($archives | ForEach-Object { $_.FullName })
if ($withInstaller) {
    # From the folder the window archive was just repacked from, so the installer carries the same signed
    # bytes as the archive beside it on the release page.
    $installerPath = Join-Path $work $INSTALLER
    Invoke-Step @('pwsh', '-NoProfile', '-File', $BUILD_MSI, '-Tag', $Tag,
        '-Payload', (Join-Path $unpacked[$WINDOW_ARCHIVE] 'ChronoMock'), '-OutDir', $work) | Out-Null
    Assert-Path $installerPath 'build-msi.ps1 reported success and wrote no installer'
    # /d is the program's name in the window Windows shows an administrator before the installer runs, and
    # it is the name the installer carries, asked of the same script that put it there.
    $productName = "$(@(Invoke-Step @('pwsh', '-NoProfile', '-File', $BUILD_MSI, '-Tag', $Tag, '-ProductName'))[-1])".Trim()
    if (-not $productName) { throw 'sign-release: build-msi.ps1 -ProductName printed no name' }
    Invoke-Step @($signtool, 'sign', '/sha1', $certificate.Thumbprint, '/fd', 'sha256',
        '/tr', $pin.timestamp_url, '/td', 'sha256', '/d', $productName, '/q', $installerPath) | Out-Null
    Assert-SignedByPin $installerPath $INSTALLER $pin.certificate_sha256
    $shipped += $installerPath
    $subjects += $installerPath
    Write-Host ("  {0}  {1}  signed as '{2}'" -f $INSTALLER, (Get-Sha256 $installerPath), $productName)
}

Write-Host "`n[6/8] bills of materials over the signed bytes, and the checksums"
# 🔴 Regenerated here rather than in phase A, and this is a deliberate difference from the ritual as
# written elsewhere. Each SBOM carries the sha256 of the archive it describes, and repacking after
# signing changes that hash - a document generated before the signature would describe an archive
# nobody ships. Phase C then attests these against the signed bytes.
foreach ($archive in $archives) {
    # 🔴 `<archive>.spdx.json`, keeping the `.zip`, and the extension is load-bearing rather than
    # cosmetic. GitHub sorts a release's assets alphabetically by file name with no other lever, so a
    # document named `<stem>.spdx.json` sorts ABOVE the archive it describes ('s' before 'z') and a
    # reader meets an SBOM before anything downloadable. With the extension kept, the archive's own
    # name is a prefix of the document's, so the shorter one leads - the shape the signature bundles
    # already had.
    #
    # This line cost a release. Every OTHER place was renamed by replacing the literal old names, and
    # this one COMPUTES the name, so it kept producing the old spelling while `$EXPECTED_ASSETS` above
    # and `attest-signed.yml` both expected the new one. Phase C failed with "SBOM file not found" on
    # v0.3.0. The lesson generalises past this file: a rename is finished when every place that
    # DERIVES the name is found, not when every place that spells it out is.
    $sbom = Join-Path $work ($archive.Name + '.spdx.json')
    & (Join-Path $PSScriptRoot 'sbom.ps1') -PackageId $PACKAGE_ID[$archive.Name] -ZipPath $archive.FullName -OutPath $sbom
    $shipped += $sbom
}
if ($withInstaller) {
    $installerSbom = Join-Path $work ($INSTALLER + '.spdx.json')
    & (Join-Path $PSScriptRoot 'sbom.ps1') -PackageId 'msi' -ZipPath $installerPath -OutPath $installerSbom
    $shipped += $installerSbom
}
$sums = Join-Path $work 'SHA256SUMS'
$lines = $shipped | ForEach-Object { "{0}  {1}" -f (Get-Sha256 $_), (Split-Path -Leaf $_) }
[System.IO.File]::WriteAllText($sums, ($lines -join "`n") + "`n", (New-Object System.Text.UTF8Encoding($false)))
$shipped += $sums
Write-Host "  SHA256SUMS over $($shipped.Count - 1) files"

Write-Host "`n[7/8] handing it back to the workflow"
Invoke-Step (@('gh', 'release', 'upload', $Tag) + $shipped + @('--repo', $repo, '--clobber')) | Out-Null
$digests = $subjects | ForEach-Object { "$(Split-Path -Leaf $_)=$(Get-Sha256 $_)" }
Invoke-Step @('gh', 'workflow', 'run', $attestWorkflow, '--repo', $repo,
    '-f', "tag=$Tag", '-f', "digests=$($digests -join ',')") | Out-Null

Write-Host "`n[8/8] confirming the draft is complete"
# 🔴 This step exists because the script would otherwise end at "dispatched, go look". The upload and
# the dispatch are two calls, phase C is a third thing, and a half-finished draft looks almost exactly
# like a finished one. Waits for phase C rather than assuming it: attesting takes well under a minute,
# and "well under a minute" is not "already done" at the moment the dispatch returns.
$deadline = (Get-Date).AddSeconds([Math]::Max(0, $Wait))
$signedDigests = @{}
foreach ($subject in $subjects) { $signedDigests[(Split-Path -Leaf $subject)] = Get-Sha256 $subject }
while ($true) {
    # 🔴 NO SPACE after the comma, and it is not a matter of taste. PowerShell splits a native
    # command's arguments on whitespace, so `--json assets, isDraft` reaches gh as two arguments,
    # `assets,` and `isDraft`. gh refuses the field list, `2>$null` swallows the complaint, and
    # $view comes back null - so this loop read an empty asset list off a release that was complete
    # and reported every expected file as missing. Measured on v0.3.0: with the space, null; without
    # it, seven assets. tools/lint.ps1 knows this failure as its rule 3 and names it "silence that
    # looks like an answer", which is exactly what it was.
    $view = gh release view $Tag --repo $repo --json 'assets,isDraft' 2>$null | ConvertFrom-Json
    $names = @()
    if ($view) { $names = @($view.assets | ForEach-Object { $_.name }) }
    $missing = @($expectedAssets | Where-Object { $names -notcontains $_ })
    $bundles = @($names | Where-Object { $_.EndsWith('.sigstore.json') })
    if (-not $missing -and $bundles.Count -ge $subjects.Count) { break }
    if ((Get-Date) -gt $deadline) {
        Write-Host "  waited $Wait s and the draft is still incomplete."
        if ($missing) { Write-Host "  missing: $($missing -join ', ')" }
        if ($bundles.Count -lt $subjects.Count) { Write-Host "  attestation bundles present: $($bundles.Count) of $($subjects.Count)" }
        Write-Host "  assets present: $(($names | Sort-Object) -join ', ')"
        throw ("sign-release: the draft is NOT complete. Nothing was published, so nothing is broken - but do " +
            "not press publish until the missing piece is there. Check the run log of $attestWorkflow. " +
            "Re-running this script is safe, the upload uses --clobber.")
    }
    Start-Sleep -Seconds 5
}

# The digests are checked against what the RELEASE carries, not against the local files we made -
# those are the same bytes only if the upload really landed.
$confirm = Join-Path $work 'confirm'
New-Item -ItemType Directory -Path $confirm -Force | Out-Null
Invoke-Step @('gh', 'release', 'download', $Tag, '--repo', $repo, '--pattern', 'SHA256SUMS', '--dir', $confirm) | Out-Null
$published = Get-Content -Raw -LiteralPath (Join-Path $confirm 'SHA256SUMS')
foreach ($name in $signedDigests.Keys) {
    if ($published -notmatch [regex]::Escape($signedDigests[$name])) {
        throw ("sign-release: SHA256SUMS on the release does NOT name the digest we signed for $name.`n" +
            "  signed:    $($signedDigests[$name])`n  published: $published")
    }
}
$view = gh release view $Tag --repo $repo --json isDraft | ConvertFrom-Json
if (-not $view.isDraft) { throw "sign-release: $Tag is NOT a draft any more - it is already public" }

Write-Host ''
foreach ($name in ($names | Sort-Object)) { Write-Host "  asset  $name" }
Write-Host '  PASS: every expected asset is there, the published checksums name the digests we signed, still a draft'
Write-Host ''
Write-Host 'Done. The release is still a DRAFT.'
Write-Host 'Read it, then publish. Publishing runs phase D, which re-checks the published bytes the way a user would.'
