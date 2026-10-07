//! The Windows installer, and the script that builds it (ADR-23).
//!
//! What the installer does on a machine is measured by installing it, which no test here does. These guards
//! hold what such a measurement cannot see coming: the lines it rests on, the one value that must never
//! change, the names several files must agree on, the refusals of the script, and the calls that make the
//! window keep nothing of its own in an installed folder. Every refusal of the script comes before WiX is
//! looked for, so all of this runs on a machine without WiX.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The product's identity in Windows Installer, for good. Every machine that has the program finds the
/// version it has through it, so a new one would leave the old install beside the new one on every one of
/// them. Written here a second time on purpose: this is the copy that does not move when the script does.
const UPGRADE_CODE: &str = "5F0CEAC5-75A7-4EE1-8C4B-B20E1012999D";

/// The only namespace the installer source may use. Anything else is a WiX extension, and an extension
/// puts code of its own into the package.
const WIX_NAMESPACE: &str = "http://wixtoolset.org/schemas/v4/wxs";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} must be readable: {e}", path.display()))
}

/// A PowerShell script without its comments, so a word in the explanation of a line never counts as the
/// line. Drops `#` lines and `<# ... #>` blocks, which is every comment form the packaging scripts use.
fn active_powershell(text: &str) -> String {
    let mut out = Vec::new();
    let mut in_block = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if in_block {
            in_block = !trimmed.contains("#>");
            continue;
        }
        if trimmed.starts_with("<#") {
            in_block = !trimmed.contains("#>");
            continue;
        }
        if !trimmed.starts_with('#') {
            out.push(line);
        }
    }
    out.join("\n")
}

/// A YAML or C# file without its whole-line comments, for the same reason.
fn without_line_comments(text: &str, marker: &str) -> String {
    text.lines().filter(|l| !l.trim_start().starts_with(marker)).collect::<Vec<_>>().join("\n")
}

/// The value a PowerShell script assigns to a variable in one quoted literal, e.g. `$Name = 'value'`.
fn powershell_literal(script: &str, variable: &str) -> String {
    let prefix = format!("${variable} = '");
    let line = active_powershell(script)
        .lines()
        .map(str::trim_start)
        .find(|l| l.starts_with(&prefix))
        .map(str::to_string)
        .unwrap_or_else(|| panic!("the script assigns no literal to ${variable}"));
    line[prefix.len()..].split('\'').next().expect("a closing quote").to_string()
}

/// pwsh, found on this process's PATH, so it can then be run with a PATH of its own.
fn pwsh() -> PathBuf {
    let path = std::env::var_os("PATH").expect("a PATH");
    std::env::split_paths(&path)
        .map(|dir| dir.join("pwsh.exe"))
        .find(|candidate| candidate.is_file())
        .expect("PowerShell 7 (pwsh) must be on PATH - the packaging scripts run on it (untouchable rule 19)")
}

struct Run {
    code: i32,
    said: String,
}

/// build-msi.ps1 with the given arguments. With `isolated`, WiX is also taken out of its reach - a PATH and
/// a home folder with nothing in them - and its temporary folder is that one too, so a run that gets past
/// every check of its inputs stops at the same place on every system and its leftovers can be seen.
fn build_msi(args: &[&str], isolated: Option<&Path>) -> Run {
    let mut command = Command::new(pwsh());
    command
        .args(["-NoProfile", "-NonInteractive", "-File"])
        .arg(repo_root().join("packaging").join("build-msi.ps1"))
        .args(args);
    if let Some(nowhere) = isolated {
        for variable in ["PATH", "USERPROFILE", "HOME", "TMP", "TEMP"] {
            command.env(variable, nowhere);
        }
    }
    let out = command.output().expect("pwsh could not be started");
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    Run { code: out.status.code().unwrap_or(-1), said }
}

/// One element of the installer source: its name, its attributes and the element it sits in.
struct Element {
    name: String,
    attrs: Vec<(String, String)>,
    parent: Option<usize>,
}

impl Element {
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

/// The attributes of one tag, `key="value"` pairs in order.
fn attributes(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(eq) = rest.find("=\"") {
        let key = rest[..eq].trim().to_string();
        let start = eq + 2;
        let close = rest[start..].find('"').expect("an unclosed attribute value");
        out.push((key, rest[start..start + close].to_string()));
        rest = &rest[start + close + 1..];
    }
    out
}

/// The elements of an XML source, comments taken out first. Written by hand rather than with a crate: a
/// dev-dependency is a rule-8 licence-sieve event, and this source is plain markup with no `>` inside a value.
fn elements(source: &str) -> Vec<Element> {
    let mut text = source.to_string();
    while let Some(start) = text.find("<!--") {
        let end = text[start..].find("-->").map(|e| start + e + 3).expect("an unclosed comment");
        text.replace_range(start..end, "");
    }
    let mut found: Vec<Element> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    let mut rest = text.as_str();
    while let Some(lt) = rest.find('<') {
        let after = &rest[lt + 1..];
        let gt = after.find('>').expect("an unclosed tag");
        let tag = &after[..gt];
        rest = &after[gt + 1..];
        if tag.starts_with('?') {
            continue;
        }
        if tag.starts_with('/') {
            open.pop();
            continue;
        }
        let body = tag.trim_end_matches('/');
        let name_end = body.find(char::is_whitespace).unwrap_or(body.len());
        found.push(Element { name: body[..name_end].to_string(), attrs: attributes(&body[name_end..]), parent: open.last().copied() });
        if !tag.ends_with('/') {
            open.push(found.len() - 1);
        }
    }
    assert!(open.is_empty(), "the installer source does not close every element it opens");
    found
}

fn named<'a>(els: &'a [Element], name: &str) -> Vec<&'a Element> {
    els.iter().filter(|e| e.name == name).collect()
}

/// The installer source of a release, rendered the way the build renders it.
fn rendered_installer() -> Vec<Element> {
    let run = build_msi(&["-Tag", "v0.4.0", "-SourceOnly"], None);
    assert_eq!(run.code, 0, "build-msi.ps1 refused to render v0.4.0:\n{}", run.said);
    let els = elements(&run.said);
    assert!(els.len() >= 15, "read {} element(s) from the installer source, so this guard is not reading it", els.len());
    els
}

/// No extension, no custom action, no dialog, nothing that closes a program: the package carries our files
/// and Windows Installer's own tables, and nothing of WiX's. That is also what keeps WiX a build tool in the
/// licence sieve rather than a component of the release.
#[test]
fn the_installer_carries_nothing_but_our_files() {
    let els = rendered_installer();
    for e in &els {
        for (key, value) in &e.attrs {
            assert!(!key.starts_with("xmlns:"), "<{}> brings in the namespace {key}={value} - the installer uses no extension", e.name);
            if key == "xmlns" {
                assert_eq!(value, WIX_NAMESPACE, "<{}> declares another namespace", e.name);
            }
        }
        assert!(
            !["CustomAction", "CloseApplication", "InstallExecuteSequence", "InstallUISequence", "UI", "UIRef", "Binary"]
                .contains(&e.name.as_str()),
            "the installer has a <{}>. It runs no action of its own, closes nothing that is running and has only \
             Windows Installer's own dialogs",
            e.name
        );
    }
}

/// The lines the installer's behaviour on a machine rests on. Each one changed would still build a working
/// installer - which is why it needs a guard rather than a build. What breaks is an upgrade, an uninstall,
/// the command line on PATH or another account's Start menu, on somebody else's machine.
#[test]
fn the_installer_is_the_shape_that_was_measured() {
    let els = rendered_installer();
    let package = named(&els, "Package");
    assert_eq!(package.len(), 1, "the source has {} <Package>, and it is one package", package.len());
    assert_eq!(package[0].attr("UpgradeCode"), Some(UPGRADE_CODE), "the UpgradeCode is {UPGRADE_CODE} for good");
    assert_eq!(package[0].attr("Scope"), Some("perMachine"), "the package installs for the machine");

    let upgrade = named(&els, "MajorUpgrade");
    assert_eq!(upgrade.len(), 1, "one major upgrade rule");
    assert_eq!(upgrade[0].attr("AllowSameVersionUpgrades"), Some("yes"), "a rebuild of a version replaces the first build");
    assert_eq!(
        upgrade[0].attr("Schedule"),
        Some("afterInstallInitialize"),
        "the old product goes before the new files land: our programs carry the product version, and a file of \
         the same version already on disk is not replaced, so a rebuild under the same number would keep them all"
    );
    assert!(upgrade[0].attr("DowngradeErrorMessage").is_some_and(|m| !m.is_empty()), "a refused downgrade says why");

    let manager = els.iter().filter(|e| e.name == "Property" && e.attr("Id") == Some("MSIRESTARTMANAGERCONTROL")).collect::<Vec<_>>();
    assert!(
        manager.len() == 1 && manager[0].attr("Value") == Some("Disable"),
        "the Restart Manager is off in the package itself: the program holding our library is often the application \
         under test, and the old package's properties decide an upgrade"
    );

    let env = named(&els, "Environment");
    assert_eq!(env.len(), 1, "one PATH entry");
    for (key, want) in [("Name", "PATH"), ("System", "yes"), ("Part", "last"), ("Permanent", "no"), ("Value", "[INSTALLFOLDER]core\\x64")] {
        assert_eq!(env[0].attr(key), Some(want), "the PATH entry has {key} = {:?}", env[0].attr(key));
    }

    let shortcuts = named(&els, "Shortcut");
    assert_eq!(shortcuts.len(), 1, "the window has one shortcut, the command line none");
    assert_eq!(shortcuts[0].attr("Target"), Some("[INSTALLFOLDER]ChronoMock.exe"), "the shortcut starts the window");
    assert_eq!(shortcuts[0].attr("WorkingDirectory"), Some("INSTALLFOLDER"), "the shortcut starts in the install folder");

    let folder = els.iter().filter(|e| e.name == "Directory" && e.attr("Id") == Some("INSTALLFOLDER")).collect::<Vec<_>>();
    assert_eq!(folder.len(), 1, "one install folder");
    let parent = folder[0].parent.map(|p| &els[p]);
    assert_eq!(
        parent.and_then(|p| p.attr("Id")),
        Some("ProgramFiles64Folder"),
        "the install folder sits directly under ProgramFiles64Folder"
    );
}

/// The signature names the product the installer carries. sign-release.ps1 signs with what -ProductName
/// prints, and Windows shows that name when it asks an administrator to let the installer run.
#[test]
fn the_installer_is_signed_with_the_name_it_carries() {
    let els = rendered_installer();
    let package = named(&els, "Package");
    let name = package.first().and_then(|p| p.attr("Name")).expect("the package has a name").to_string();
    let run = build_msi(&["-Tag", "v0.4.0", "-ProductName"], None);
    assert_eq!(run.code, 0, "build-msi.ps1 -ProductName exited {}:\n{}", run.code, run.said);
    assert_eq!(run.said.trim_end(), name, "the name signed and the name carried differ");
}

/// A release candidate gets no installer, and a version Windows Installer cannot hold is refused rather than
/// cut. Windows Installer reads only three numbers, so a candidate's installer would carry the release's own
/// version and the release could not replace it.
#[test]
fn the_installer_is_built_for_a_release_only() {
    for (tag, says) in [
        ("v0.4.0-rc.1", "is a release candidate"),
        ("v0.4.0-beta", "is a release candidate"),
        ("0.4.0", "is not a release tag"),
        ("v256.0.0", "does not fit an installer version"),
        ("v0.256.0", "does not fit an installer version"),
        ("v0.0.65536", "does not fit an installer version"),
    ] {
        let run = build_msi(&["-Tag", tag, "-SourceOnly"], None);
        assert!(run.code == 1 && run.said.contains(says), "-Tag {tag} exited {} and said:\n{}\nwant exit 1 and {says:?}", run.code, run.said);
    }
    let largest = build_msi(&["-Tag", "v255.255.65535", "-SourceOnly"], None);
    assert_eq!(largest.code, 0, "the largest version an installer holds is refused:\n{}", largest.said);
}

/// A window package with everything a release puts in it, as build-msi.ps1 checks it.
fn fresh_package(dir: &Path) {
    for file in [
        "ChronoMock.exe", "ChronoMock.dll", "ChronoMock.Protocol.dll", "core/x64/chrono.exe", "core/x64/chrono_hook.dll",
        "core/x86/chrono.exe", "core/x86/chrono_hook.dll", "LICENSE", "THIRD-PARTY-NOTICES.md", "README.md",
        "calendars/pl.json", "presets/trial.json",
    ] {
        let path = dir.join(file);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("a folder for the fixture");
        std::fs::write(&path, b"fixture").expect("a fixture file");
    }
}

/// One way a package differs from a fresh one: what it is, how the package (or the output folder) is
/// changed, and the words the refusal must contain.
type PackageCase = (&'static str, fn(&Path, &Path), &'static str);

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    names.sort();
    names
}

/// The installer is built from a fresh window package or not at all, and a refusal leaves nothing behind.
/// Each case differs from a package that gets through in one thing, and that package is asked first: it
/// reaches the step that looks for WiX, which this guard keeps out of reach. So each refusal below is the
/// refusal of what the case changed, not of something the fixture got wrong.
#[test]
fn the_installer_is_built_from_a_fresh_window_package_or_not_at_all() {
    let cases: [PackageCase; 7] = [
        ("nothing wrong with the package", |_, _| {}, "dotnet tool install --global wix --version 5.0.2"),
        ("the 32-bit injected library is missing", |p, _| std::fs::remove_file(p.join("core/x86/chrono_hook.dll")).expect("removed"), "holds no core/x86/chrono_hook.dll"),
        ("a window run from it left its history", |p, _| std::fs::create_dir(p.join("history")).expect("made"), "holds a history folder"),
        ("a window run from it left its logs", |p, _| std::fs::create_dir(p.join("logs")).expect("made"), "holds a logs folder"),
        ("it is an installed copy", |p, _| std::fs::write(p.join("installed.txt"), b"x").expect("written"), "already holds installed.txt"),
        ("there are no calendars", |p, _| std::fs::remove_dir_all(p.join("calendars")).expect("removed"), "holds no calendars folder"),
        ("the installer is already there", |_, o| std::fs::write(o.join("ChronoMock-app-win-x64.msi"), b"an earlier one").expect("written"), "Nothing is overwritten"),
    ];
    let base = std::env::temp_dir().join(format!("chrono-msi-guard-{}", std::process::id()));
    for (n, (what, change, says)) in cases.iter().enumerate() {
        let case = base.join(n.to_string());
        let (package, out, nowhere) = (case.join("ChronoMock"), case.join("out"), case.join("nowhere"));
        for dir in [&package, &out, &nowhere] {
            std::fs::create_dir_all(dir).expect("a folder for the case");
        }
        fresh_package(&package);
        change(&package, &out);
        let before = entries(&out);
        let run = build_msi(
            &["-Tag", "v9.9.9", "-Payload", &package.to_string_lossy(), "-OutDir", &out.to_string_lossy()],
            Some(&nowhere),
        );
        assert!(run.code == 1 && run.said.contains(says), "{what}: exit {}, build-msi.ps1 said:\n{}\nwant exit 1 and {says:?}", run.code, run.said);
        assert_eq!(entries(&out), before, "{what}: -OutDir changed under a run that refused");
        assert!(entries(&nowhere).iter().all(|e| !e.starts_with("chrono-msi-")), "{what}: a refusal left a staging folder");
    }
    let _ = std::fs::remove_dir_all(&base);
}

/// The staged copy is the whole package, wherever the package lies. A package in a folder whose name holds
/// brackets, with a hidden file at its top, gets all the way to the step that looks for WiX - which a copy
/// that left anything out would not, because the copy is checked against the package right after it is
/// made. Read as a pattern, that folder name copied nothing and the build went on (review of #96).
#[test]
fn the_installer_stages_the_whole_package_wherever_it_lies() {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    let base = std::env::temp_dir().join(format!("chrono-msi-stage-{}", std::process::id()));
    let package = base.join("pkg [1]").join("ChronoMock");
    let (out, nowhere) = (base.join("out"), base.join("nowhere"));
    for dir in [&package, &out, &nowhere] {
        std::fs::create_dir_all(dir).expect("a folder for the case");
    }
    fresh_package(&package);
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .attributes(FILE_ATTRIBUTE_HIDDEN)
        .open(package.join("hidden.dat"))
        .expect("a hidden file at the top of the package");
    let run = build_msi(
        &["-Tag", "v9.9.9", "-Payload", &package.to_string_lossy(), "-OutDir", &out.to_string_lossy()],
        Some(&nowhere),
    );
    assert!(
        run.code == 1 && run.said.contains("dotnet tool install --global wix --version 5.0.2"),
        "a whole package in a bracketed folder did not reach the step that looks for WiX (exit {}):\n{}",
        run.code,
        run.said
    );
    assert!(entries(&nowhere).iter().all(|e| !e.starts_with("chrono-msi-")), "the run left its staging folder behind");
    let _ = std::fs::remove_dir_all(&base);
}

/// The names several files have to agree on, read from each of them. Four files spell the installer's name,
/// two the marker, two the upgrade code - and each disagreement builds, signs and publishes without a word.
#[test]
fn every_file_agrees_on_the_names_the_installer_lives_by() {
    let script = read("packaging/build-msi.ps1");
    assert_eq!(powershell_literal(&script, "UpgradeCode"), UPGRADE_CODE, "build-msi.ps1 carries another UpgradeCode");
    assert_eq!(powershell_literal(&script, "WixVersion"), "5.0.2", "the WiX version moved without this guard");

    let marker = powershell_literal(&script, "InstalledMarker");
    let paths = without_line_comments(&read("gui/ChronoMock.App/Session/AppPaths.cs"), "//");
    assert!(
        paths.contains(&format!("internal const string InstalledMarker = \"{marker}\";")),
        "the window looks for another marker than the one the installer lays down ({marker})"
    );

    let register: serde_json::Value = serde_json::from_str(&read("packaging/components.json")).expect("the register is JSON");
    let name = register["packages"]["msi"]["zip"].as_str().expect("the register names the installer").to_string();
    let attest = without_line_comments(&read(".github/workflows/attest-signed.yml"), "#");
    assert!(attest.contains(&format!("INSTALLER: {name}")), "phase C looks for another installer than {name}");
    assert!(attest.contains(&format!("sbom-path: assets/{name}.spdx.json")), "phase C attests another SBOM than {name}'s");
    let verify = without_line_comments(&read(".github/workflows/verify-release.yml"), "#");
    for want in [format!("$installer = '{name}'"), format!("$installer = 'published/{name}'")] {
        assert!(verify.contains(&want), "phase D names another installer than {name}: no {want:?}");
    }
    let sign = active_powershell(&read("packaging/sign-release.ps1"));
    assert!(sign.contains(".packages.msi.zip"), "phase B does not take the installer's name from the register");
    assert!(script.contains("$register.packages.msi.zip"), "build-msi.ps1 does not take the installer's name from the register");
}

/// One rule for a release candidate in every place that asks it: a hyphen in the tag. Two rules would
/// disagree about a tag like v1.0.0-beta, and the one that says "release" would put an installer on a
/// candidate's page or wait for one that never comes.
#[test]
fn every_place_asks_the_same_question_of_a_candidate() {
    for (file, active, rule) in [
        ("packaging/build-msi.ps1", active_powershell(&read("packaging/build-msi.ps1")), "if ($releaseTag.Contains('-'))"),
        ("packaging/sign-release.ps1", active_powershell(&read("packaging/sign-release.ps1")), "return $releaseTag.Contains('-')"),
        (".github/workflows/attest-signed.yml", without_line_comments(&read(".github/workflows/attest-signed.yml"), "#"), "*-*) want=false ;;"),
        (".github/workflows/verify-release.yml", without_line_comments(&read(".github/workflows/verify-release.yml"), "#"), "-not $env:TAG.Contains('-')"),
    ] {
        assert!(active.contains(rule), "{file} no longer asks a candidate {rule:?}");
    }
}

/// One first release for the installer in every phase: 0.4.0. A fix on an older line comes from a tree with
/// no installer template, so phase B makes none - and a phase C that expected one there would fail that
/// release, as phase D would fail one that carried it (review of #96).
#[test]
fn every_phase_starts_the_installer_at_the_same_release() {
    let sign = active_powershell(&read("packaging/sign-release.ps1"));
    let attest = without_line_comments(&read(".github/workflows/attest-signed.yml"), "#");
    let verify = without_line_comments(&read(".github/workflows/verify-release.yml"), "#");
    for (file, active, rule) in [
        ("packaging/sign-release.ps1", &sign, "$INSTALLER_SINCE = [version]'0.4.0'"),
        ("packaging/sign-release.ps1", &sign, "$withInstaller = Test-Installer $Tag"),
        (".github/workflows/attest-signed.yml", &attest, "printf '%s\\n' 0.4.0 \"$numbers\" | sort -V"),
        (".github/workflows/verify-release.yml", &verify, "($version -ge [version]'0.4.0')"),
    ] {
        assert!(active.contains(rule), "{file} no longer starts the installer at 0.4.0 the way the others do: no {rule:?}");
    }
}

/// Phase B asks whether it can build the installer before the card signs anything, builds it from the tag's
/// own inputs, and signs and checks it the way it signs and checks the programs. A check after the card would
/// leave signed programs on a draft that can never get its installer from that run.
#[test]
fn the_signing_asks_for_the_installer_before_the_card_and_signs_it_like_the_programs() {
    let sign = active_powershell(&read("packaging/sign-release.ps1"));
    let check = sign.find("'-Tag', $Tag, '-Check'").expect("sign-release.ps1 never asks build-msi.ps1 -Check");
    let card = sign.find("$pin = Get-Pin").expect("sign-release.ps1 never reads the pin");
    assert!(check < card, "the installer check comes after the card is reached");
    for (what, want) in [
        ("its inputs are compared with the tag", "git diff --quiet \"refs/tags/$Tag\" -- @INSTALLER_INPUTS"),
        ("it signs the installer with the name it carries", "'/d', $productName"),
        ("it reads the installer's certificate back against the pin", "Assert-SignedByPin $installerPath $INSTALLER $pin.certificate_sha256"),
        ("the installer gets its own bill of materials", "-PackageId 'msi' -ZipPath $installerPath"),
        ("a complete draft has the installer", "$expectedAssets += @($INSTALLER, \"$INSTALLER.spdx.json\")"),
        ("phase C attests the installer too", "$subjects += $installerPath"),
    ] {
        assert!(sign.contains(want), "{what}: sign-release.ps1 does not contain {want:?}");
    }
}

/// The window keeps nothing of its own in a folder the installer put it in. The choice itself is tested in
/// C# (DataFolderTests). What a unit test cannot see is a store that stops asking it, so the calls are held
/// here: both stores go through the one choice, and the choice is told whether the copy is installed.
#[test]
fn the_window_keeps_nothing_of_its_own_in_an_installed_folder() {
    let folder = without_line_comments(&read("gui/ChronoMock.App/Session/WritableFolder.cs"), "//");
    assert!(folder.contains("AppPaths.IsInstalled,"), "WritableFolder.ForApp no longer tells the choice whether the copy is installed");
    let history = without_line_comments(&read("gui/ChronoMock.App/Session/SessionHistory.cs"), "//");
    assert!(history.contains("WritableFolder.ForApp(\"history\")"), "the history store chooses its folder some other way");
    let log = without_line_comments(&read("gui/ChronoMock.App/Session/DiagnosticsLog.cs"), "//");
    assert!(log.contains("WritableFolder.ForApp(\"logs\")"), "the diagnostics log chooses its folder some other way");
}
