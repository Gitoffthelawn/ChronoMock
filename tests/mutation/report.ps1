# Puts the parts of a mutation run together and says what the run found.
#
# tests/mutation/mutate.ps1 -Json writes one file per part, and .github/workflows/mutation.yml downloads
# them all into one folder for this script. It answers three questions in this order, because a later
# answer means nothing without the earlier one:
#
#   1. Did every part report and finish? A part that never wrote its file - a machine that went away, a
#      step that crashed before the first entry - is a failure, not a smaller run. A file saying
#      finished = false is a part stopped half way, by a time limit or a cancelled run.
#   2. Can each part be believed? Its canaries have to have come back BROKEN, its unmutated tree has to
#      have built, and it has to have selected the same entries the plan counted.
#   3. Which entries were not caught?
#
# With -Gate, used weekly and by hand from the Actions tab, any of that is red. Without it, on a pull
# request, the same findings are warnings and the job is green. A decision of 2026-10-16: a
# report first, and a gate only once the runner has earned trust on those machines.
#
#     pwsh -File tests/mutation/report.ps1 -Folder results -Entries 745 -Parts 16 -Gate
#     pwsh -File tests/mutation/report.ps1 -SelfTest
#
# A FILE OF THE WRONG SHAPE IS A PART THAT CANNOT BE READ, NEVER A CRASH. A crash ends with code 1, which
# under -Gate reads exactly like "found something" - so every part is read inside its own try, and what
# went wrong is said as a problem with that part. The self test plants such a file.

[CmdletBinding(PositionalBinding = $false)]
param(
    [string] $Folder = 'results',
    [int] $Entries = 0,
    [int] $Parts = 0,
    [switch] $Gate,
    [switch] $SelfTest
)

$ErrorActionPreference = 'Stop'

# Numbers in the summary read the same on every machine - "0.9 min", not "0,9 min".
[Threading.Thread]::CurrentThread.CurrentCulture = [Globalization.CultureInfo]::InvariantCulture

. (Join-Path $PSScriptRoot 'json.ps1')

# What GitHub reads as an annotation. The message is escaped the way the runner documents for workflow
# commands - a percent sign and both newline characters - so a name holding one cannot end the command
# early or start another.
function Format-Annotation([string] $Level, [string] $Message) {
    $safe = $Message.Replace('%', '%25').Replace("`r", '%0D').Replace("`n", '%0A')
    return "::$Level title=mutation::$safe"
}

function Format-Cell([string] $Text) {
    return $Text.Replace('|', '\|').Replace("`r", ' ').Replace("`n", ' ')
}

function Read-Run([string] $Folder, [int] $Entries, [int] $Parts) {
    $problems = [Collections.Generic.List[string]]::new()
    $results = [Collections.Generic.List[object]]::new()
    $partSeconds = [Collections.Generic.List[double]]::new()
    $counted = 0

    for ($part = 0; $part -lt $Parts; $part++) {
        $path = Join-Path $Folder "result-$part.json"

        if (-not (Test-Path -LiteralPath $path)) {
            $problems.Add("part $part did not report - no result-$part.json")
            continue
        }

        try {
            $state = ConvertFrom-JsonText ([IO.File]::ReadAllText($path))

            if ($state -isnot [pscustomobject] -or $null -eq $state.PSObject.Properties['results']) {
                throw 'it is not one object with a results list'
            }

            $list = @($state.results)
            foreach ($one in $list) {
                if ($one -isnot [pscustomobject] -or $one.verdict -isnot [string] -or $one.name -isnot [string]) {
                    throw 'a result is not an object with a verdict and a name'
                }
            }

            if ([int] $state.selected -ne $Entries) {
                $problems.Add("part $part selected $($state.selected) entries and the plan counted $Entries - the parts did not agree on what to judge")
            }

            # A part whose every entry was NOT HERE builds nothing and runs no canary, rightly.
            $notHereHere = @($list | Where-Object { $_.verdict -eq 'NOT HERE' }).Count
            $judgedHere = [int] $state.entries -gt $notHereHere
            $believed = (-not $judgedHere) -or ([string] $state.canary -eq 'BROKEN' -and [bool] $state.built)

            if ($judgedHere -and [string] $state.canary -ne 'BROKEN') {
                $problems.Add("part $part canaries came back '$($state.canary)' instead of BROKEN, so nothing it says can be believed")
            }

            if (-not [bool] $state.built) {
                $problems.Add("part $part could not build the unmutated tree")
            }

            if (-not [bool] $state.finished) {
                $problems.Add("part $part stopped after $($list.Count) of $($state.entries) entries")
            } elseif ($believed -and $list.Count -ne [int] $state.entries) {
                $problems.Add("part $part finished with $($list.Count) results for $($state.entries) entries")
            }

            $counted += [int] $state.entries
            $partSeconds.Add([double] $state.seconds)
            foreach ($one in $list) { $results.Add($one) }
        } catch {
            $problems.Add("part $part wrote a file that does not have the shape this script reads: $($_.Exception.Message)")
        }
    }

    if ($problems.Count -eq 0 -and $counted -ne $Entries) {
        $problems.Add("the parts held $counted entries between them and the plan counted $Entries")
    }

    return [pscustomobject]@{ Problems = $problems; Results = $results; PartSeconds = $partSeconds }
}

function Get-Median([double[]] $Values) {
    if ($Values.Count -eq 0) { return 0 }
    $sorted = @($Values | Sort-Object)
    return $sorted[[int] [Math]::Floor(($sorted.Count - 1) / 2)]
}

if ($SelfTest) {
    # Each case is a folder of planted part files and the exit code it must give. The script is run as
    # a child process, the way the workflow runs it, so the exit code is the real one.
    $root = Join-Path ([IO.Path]::GetTempPath()) ("chronomock-mutation-report-selftest-{0}" -f $PID)
    $env:GITHUB_STEP_SUMMARY = ''

    function New-Part([string] $Folder, [int] $Shard, [int] $Selected, [string[]] $Verdicts, [hashtable] $Change) {
        $state = [ordered]@{
            shard = $Shard; parts = 2; selected = $Selected; entries = $Verdicts.Count; started = '2026-10-16T02:07:00Z'
            seconds = 60; canary = 'BROKEN'; built = $true; baselineRed = @(); finished = $true
            results = @($Verdicts | ForEach-Object -Begin { $n = 0 } -Process {
                $n++
                [ordered]@{ verdict = $_; name = "entry $Shard-$n"; test = 'tests::some_test'; file = 'crates/core/src/lib.rs'; why = 'its file'; notHere = $(if ($_ -eq 'NOT HERE') { 'needs a live browser' } else { '' }); seconds = 10 }
            })
        }
        if ($Change) { foreach ($key in $Change.Keys) { $state[$key] = $Change[$key] } }
        [IO.File]::WriteAllText((Join-Path $Folder "result-$Shard.json"), ($state | ConvertTo-Json -Depth 5), [Text.UTF8Encoding]::new($false))
    }

    $cases = @(
        @{ Name = 'every entry caught'; Gate = $true; Expect = 0; Build = { param($f) New-Part $f 0 4 @('caught', 'caught') $null; New-Part $f 1 4 @('caught', 'caught') $null } }
        @{ Name = 'a MISSED entry under the gate'; Gate = $true; Expect = 1; Said = 'MISSED'; Build = { param($f) New-Part $f 0 4 @('caught', 'MISSED') $null; New-Part $f 1 4 @('caught', 'caught') $null } }
        @{ Name = 'a MISSED entry as a report'; Gate = $false; Expect = 0; Said = '::warning title=mutation::MISSED'; Build = { param($f) New-Part $f 0 4 @('caught', 'MISSED') $null; New-Part $f 1 4 @('caught', 'caught') $null } }
        @{ Name = 'a NO TEST entry under the gate'; Gate = $true; Expect = 1; Said = 'NO TEST'; Build = { param($f) New-Part $f 0 4 @('caught', 'NO TEST') $null; New-Part $f 1 4 @('caught', 'caught') $null } }
        @{ Name = 'a part that did not report'; Gate = $true; Expect = 1; Said = 'did not report'; Build = { param($f) New-Part $f 0 4 @('caught', 'caught') $null } }
        @{ Name = 'a part stopped half way'; Gate = $true; Expect = 1; Said = 'stopped after'; Build = { param($f) New-Part $f 0 4 @('caught', 'caught') $null; New-Part $f 1 4 @('caught') @{ finished = $false; entries = 2 } } }
        @{ Name = 'a canary that came back caught'; Gate = $true; Expect = 1; Said = 'canaries'; Build = { param($f) New-Part $f 0 4 @('caught', 'caught') $null; New-Part $f 1 4 @('caught', 'caught') @{ canary = 'rust caught' } } }
        @{ Name = 'NOT HERE is not a failure'; Gate = $true; Expect = 0; Said = 'needs a live browser'; Build = { param($f) New-Part $f 0 4 @('caught', 'NOT HERE') $null; New-Part $f 1 4 @('caught', 'caught') $null } }
        @{ Name = 'a part that selected other entries'; Gate = $true; Expect = 1; Said = 'did not agree'; Build = { param($f) New-Part $f 0 4 @('caught', 'caught') $null; New-Part $f 1 5 @('caught', 'caught') $null } }
        @{ Name = 'a file of the wrong shape'; Gate = $true; Expect = 1; Said = 'does not have the shape'; Build = { param($f) New-Part $f 0 4 @('caught', 'caught') $null; [IO.File]::WriteAllText((Join-Path $f 'result-1.json'), '{"results": "not a list", "entries": 2}') } }
        @{ Name = 'a part whose tree did not build'; Gate = $true; Expect = 1; Said = 'could not build'; Build = { param($f) New-Part $f 0 4 @('caught', 'caught') $null; New-Part $f 1 4 @('caught', 'caught') @{ built = $false } } }
    )

    $failed = @()
    foreach ($case in $cases) {
        $caseFolder = Join-Path $root ($cases.IndexOf($case))
        [void] (New-Item -ItemType Directory -Path $caseFolder -Force)
        & $case.Build $caseFolder

        $arguments = @('-NoProfile', '-File', $PSCommandPath, '-Folder', $caseFolder, '-Entries', '4', '-Parts', '2')
        if ($case.Gate) { $arguments += '-Gate' }
        $said = (& pwsh @arguments) -join "`n"
        $code = $LASTEXITCODE

        if ($code -ne $case.Expect) { $failed += "$($case.Name): exit $code, expected $($case.Expect)" }
        if ($case.Said -and -not $said.Contains($case.Said)) { $failed += "$($case.Name): the report did not say '$($case.Said)'" }

        foreach ($file in Get-ChildItem -LiteralPath $caseFolder -File) { [IO.File]::Delete($file.FullName) }
        [IO.Directory]::Delete($caseFolder)
    }
    [IO.Directory]::Delete($root)

    if ($failed.Count) {
        $failed | ForEach-Object { Write-Output "self test FAILED: $_" }
        exit 1
    }

    Write-Output ("self test passed: {0} planted runs gave the exit code and the words they must" -f $cases.Count)
    exit 0
}

if ($Parts -le 0) {
    Write-Output 'No part was planned, so there is nothing to put together.'
    exit 0
}

$run = Read-Run $Folder $Entries $Parts
$level = if ($Gate) { 'error' } else { 'warning' }

$tally = [ordered]@{}
foreach ($group in ($run.Results | Group-Object { $_.verdict } | Sort-Object Name)) { $tally[$group.Name] = $group.Count }

$notCaught = @($run.Results | Where-Object { $_.verdict -ne 'caught' -and $_.verdict -ne 'NOT HERE' })
$notHere = @($run.Results | Where-Object { $_.verdict -eq 'NOT HERE' })
$caught = @($run.Results | Where-Object { $_.verdict -eq 'caught' }).Count
$entrySeconds = [double[]] @($run.Results | Where-Object { $_.verdict -ne 'NOT HERE' } | ForEach-Object { [double] $_.seconds })
$slowest = $run.Results | Sort-Object { [double] $_.seconds } -Descending | Select-Object -First 1

$mode = if ($Gate) { 'gate' } else { 'report - never red on a pull request' }
$lines = [Collections.Generic.List[string]]::new()
$lines.Add("## Mutation run ($mode)")
$lines.Add('')
$lines.Add(("{0} entries planned in {1} part(s). {2} caught, {3} not caught, {4} NOT HERE." -f $Entries, $Parts, $caught, $notCaught.Count, $notHere.Count))
$lines.Add('')
$lines.Add('| verdict | entries |')
$lines.Add('|---|---|')
foreach ($key in $tally.Keys) { $lines.Add("| $key | $($tally[$key]) |") }

if ($run.PartSeconds.Count -gt 0) {
    $lines.Add('')
    $lines.Add(("Parts took {0:N1} to {1:N1} min, median {2:N1}. An entry took a median of {3:N1} s." -f
        (($run.PartSeconds | Measure-Object -Minimum).Minimum / 60), (($run.PartSeconds | Measure-Object -Maximum).Maximum / 60),
        ((Get-Median ([double[]] $run.PartSeconds)) / 60), (Get-Median $entrySeconds)))
    if ($slowest) { $lines.Add(("The slowest entry took {0:N1} s: {1}" -f [double] $slowest.seconds, (Format-Cell $slowest.name))) }
}

if ($run.Problems.Count) {
    $lines.Add('')
    $lines.Add('### Problems with the parts')
    $lines.Add('')
    foreach ($problem in $run.Problems) { $lines.Add("- $problem") }
}

if ($notCaught.Count) {
    $lines.Add('')
    $lines.Add('### Not caught')
    $lines.Add('')
    $lines.Add('MISSED means the guard does not check what the entry says - fix the guard, never the mutation. STALE means an anchor is no longer in its file exactly once, NO TEST that the filter ran no test - the code or the test moved on. BASELINE RED means the test is red with no mutation at all. BROKEN means the mutation does not compile. TIMEOUT means the test hung instead of failing, ERROR that the test command failed without naming a test. The part log holds what each command said.')
    $lines.Add('')
    $lines.Add('| verdict | entry | test | file |')
    $lines.Add('|---|---|---|---|')
    foreach ($one in ($notCaught | Sort-Object { $_.verdict }, { $_.name })) {
        $lines.Add(("| {0} | {1} | {2} | {3} |" -f $one.verdict, (Format-Cell $one.name), (Format-Cell ([string] $one.test)), (Format-Cell ([string] $one.file))))
    }
}

if ($notHere.Count) {
    $lines.Add('')
    $lines.Add('### NOT HERE - judged on another machine, not on this runner')
    $lines.Add('')
    foreach ($one in ($notHere | Sort-Object { $_.name })) { $lines.Add(("- {0}: {1}" -f (Format-Cell $one.name), (Format-Cell ([string] $one.notHere)))) }
}

$lines | ForEach-Object { Write-Output $_ }

if ($env:GITHUB_STEP_SUMMARY) {
    [IO.File]::AppendAllText($env:GITHUB_STEP_SUMMARY, (($lines -join "`n") + "`n"), [Text.UTF8Encoding]::new($false))
}

foreach ($problem in $run.Problems) { Write-Output (Format-Annotation $level $problem) }
foreach ($one in $notCaught) { Write-Output (Format-Annotation $level ("{0}: {1} -> {2}" -f $one.verdict, $one.name, $one.test)) }

if ($Gate -and ($run.Problems.Count -gt 0 -or $notCaught.Count -gt 0)) { exit 1 }
exit 0
