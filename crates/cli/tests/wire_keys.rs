//! Guard for the wire-key contract: every translation key the core puts on the NDJSON wire must
//! exist in BOTH shipped translations. A key that reaches the panel without one renders as raw
//! jargon - the RELEASE-002 class, which a pre-release audit already had to close once by hand.
//!
//! Why this guard exists at all: the GUI side keeps `CoreWireKeys` in `LocalizationTests.cs`, a
//! MANUALLY maintained list whose own comment says "when the core adds a rendered key, add it
//! here". Nothing checked that list against reality, and it had already drifted - the audit of
//! 2026-09-05 found `moment.unsupported_kind`, emitted from a live jump path, missing from the
//! list and from both translation files. A guard written in prose but absent from code is worse
//! than none (untouchable rule 12), so this is a real test that fails on a real gap.
//!
//! Two sets, checked by two tests, because the core names its failures in two different SHAPES.
//!
//! The first is keys that travel over the wire as an event field, written as bare string literals.
//! The second is the calculator's, which the engine writes INSIDE an English sentence on stderr -
//! `chrono calc: step 1 needs a calendar - pass --calendar (calc.needs_calendar)`. The shape scan
//! cannot see those: the literal it finds is the whole sentence, which is not key-shaped, so every
//! `calc.*` key was invisible to this guard.
//!
//! That invisibility had a cost. Eight of the nine calculator keys had no translation at all and the
//! panel showed the engine's raw sentence - wrong language, a process name, and the contract key in
//! brackets - while this file's own header said calculator errors "are not translation keys". They
//! are: the GUI maps `calc.X` to `calc.err.X` (CalcErrorText), which is what the second test below
//! holds it to.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// Source text with the inline test module removed, so fixture keys invented by tests
/// (`some.future_key`, `cleanup.something_new`) never look like production emissions.
///
/// The cut is anchored on `#[cfg(test)] mod tests`, not on `#[cfg(test)]` alone, and the difference
/// is not cosmetic. A crate-root `#[cfg(test)] mod testutil;` declaration sits among the other module
/// declarations at the TOP of `main.rs`, and cutting on the bare attribute threw the whole file away -
/// the scanner dropped from more than fifty keys to eighteen while still passing every other check.
/// The canary below caught it. This is the fix, not the canary's removal.
fn production_source(path: &Path) -> String {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let mut from = 0;
    while let Some(hit) = text[from..].find("#[cfg(test)]") {
        let at = from + hit;
        let after = text[at + "#[cfg(test)]".len()..].trim_start();
        if after.starts_with("mod tests") {
            return text[..at].to_string();
        }
        from = at + "#[cfg(test)]".len();
    }
    text
}

/// A wire key looks like `area.detail` - lowercase words joined by dots and underscores. This
/// shape check is what keeps file names (`icudtl.dat`) and JS expressions (`performance.now`)
/// out of the set without needing a hand-written blocklist.
fn is_key_shaped(s: &str) -> bool {
    for ch in s.chars() {
        match ch {
            '.' | 'a'..='z' | '0'..='9' | '_' => {}
            _ => return false,
        }
    }
    let segments: Vec<&str> = s.split('.').collect();
    if segments.len() < 2 || s.len() < 5 {
        return false;
    }
    // Every segment must be non-empty and contain a letter. This is what rejects "127.0.0.1"
    // without a hand-written blocklist - an address is all digits, a key never is.
    if segments
        .iter()
        .any(|seg| seg.is_empty() || !seg.chars().any(|c| c.is_ascii_lowercase()))
    {
        return false;
    }
    // File names share the shape. The list is short, explicit and only ever grows when a new kind of
    // file name shows up in the sources (icudtl.dat, snapshot_blob.bin, v8_context_snapshot.bin, and an
    // Electron application's app.asar).
    const FILE_SUFFIXES: [&str; 8] = [".dat", ".bin", ".exe", ".dll", ".json", ".js", ".now", ".asar"];
    !FILE_SUFFIXES.iter().any(|suffix| s.ends_with(suffix))
}

/// Every `.rs` file under `dir`, recursively. The canary is part of the helper: a walk that finds
/// nothing looks exactly like a codebase with no keys in it.
fn rust_sources_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let entries =
            std::fs::read_dir(&d).unwrap_or_else(|e| panic!("cannot read {}: {e}", d.display()));
        for entry in entries {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    out.sort();
    assert!(
        out.len() >= 5,
        "the walk found only {} Rust sources under {} - it is reading the wrong place",
        out.len(),
        dir.display()
    );
    out
}

/// Keys emitted onto the wire by the core, gathered from the shapes this codebase actually uses.
fn emitted_keys(root: &Path) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();

    // Every production source of the CLI crate, FOUND rather than listed. A hand-kept list has to be
    // extended on every file split, and forgetting is invisible: the keys of the moved code leave the
    // set silently, and the canary below (more than twenty keys) does not notice a set that merely
    // shrank. Measured when `main.rs` was split into modules - the list named four files and the
    // crate had eight. A walk cannot forget.
    // Scanning only the emission SITES misses most of the set: many keys are not literals where
    // the event is built (`reason_key: reason.to_string()`), they arrive through helpers whose
    // match arms hold the literal (`session_reason_key`, `jump_error_key`, `describe_reason`).
    // Measured 2026-09-05: site-only scanning found 19 of them. So the scan is by SHAPE over
    // production code, and `is_key_shaped` plus the test-module cut carry the precision.
    let mut all = rust_sources_under(&root.join("crates/cli/src"));
    all.push(root.join("crates/proto/src/lib.rs"));
    all.push(root.join("crates/mech/src/lib.rs"));
    for path in &all {
        let text = production_source(path);
        for line in text.lines() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            keys.extend(keys_in_line(line));
        }
    }

    keys
}

/// Every key-shaped run of characters written between two double quotes on one line.
///
/// Every quote is tried as an opening one, rather than pairing them left to right. Pairing is what
/// Rust source looks like until a raw string holds quotes of its own: `to_ndjson`'s fallback is
/// `r#"{"type":"error",...,"key":"proto.serialize_failed",...}"#`, where left-to-right pairing lands
/// one quote out of step and the key falls BETWEEN two pairs - so the one key the protocol crate emits
/// on its own was invisible to all three tests below. Trying every quote costs nothing in precision:
/// the run has to be key-shaped all the way to the next quote, which text between two literals never is.
fn keys_in_line(line: &str) -> Vec<String> {
    let bytes = line.as_bytes();
    let mut found = Vec::new();
    for (open, _) in line.match_indices('"') {
        let start = open + 1;
        let end = start
            + bytes[start..]
                .iter()
                .take_while(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_'))
                .count();
        if bytes.get(end) == Some(&b'"') && is_key_shaped(&line[start..end]) {
            found.push(line[start..end].to_string());
        }
    }
    found
}

/// The functions in `report.rs` that turn a key into prose for the CLI report. A key the core emits
/// must have an arm in one of them, or the report prints it raw. `describe_chromium_warning` holds the
/// `chromium.*` keys `describe_warning` hands it (R4/16, split for the length ceiling).
const GLOSSING_FUNCTIONS: [&str; 6] = [
    "describe_reason",
    "describe_error",
    "describe_warning",
    "describe_chromium_warning",
    "describe_residue",
    "vanish_cause",
];

/// The bodies of [`GLOSSING_FUNCTIONS`], comment lines dropped so a key MENTIONED in a comment is not
/// taken for an arm.
///
/// A body ends at the first closing brace in column 0, not by counting braces, because the prose in
/// the arms is free to hold a brace of its own. The canaries are part of it: a renamed function would
/// leave the guard checking fewer bodies than it names, and a body that ran on into the next function
/// would count that function's arms as glosses - both look exactly like a report that explains
/// everything.
fn gloss_bodies(report: &str) -> String {
    let mut out = String::new();
    for name in GLOSSING_FUNCTIONS {
        let start = report
            .find(&format!("fn {name}("))
            .unwrap_or_else(|| panic!("report.rs has no `fn {name}(` - a glossing function was renamed and this guard went blind"));
        let len = report[start..]
            .find("\n}")
            .unwrap_or_else(|| panic!("`fn {name}` in report.rs has no closing brace in column 0"));
        let body = &report[start..start + len];
        assert!(
            !body.contains("\nfn ") && !body.contains("\npub"),
            "the body cut for `fn {name}` ran into the next function - the cut is wrong, not the report"
        );
        for line in body.lines() {
            if !line.trim_start().starts_with("//") {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}

/// Whether `key` is a match arm pattern in `bodies`: `"key" =>`, or one alternative of `"a" | "b" =>`.
/// What follows the key decides it - every alternative is followed by either `|` or `=>`, the last
/// one included - so a key that only appears as an arm's VALUE is not taken for a pattern.
///
/// An arm whose text is empty is not a gloss either: every one of these functions treats "" as "no
/// words for this key" and prints the key raw, so `"key" => ""` would have passed this scan while the
/// report said exactly what it said with no arm at all.
fn has_arm(bodies: &str, key: &str) -> bool {
    let quoted = format!("\"{key}\"");
    bodies.match_indices(&quoted).any(|(at, _)| {
        let after = bodies[at + quoted.len()..].trim_start();
        match after.strip_prefix("=>") {
            Some(value) => !value.trim_start().starts_with("\"\""),
            None => after.starts_with('|'),
        }
    })
}

/// The calculator's stable keys, which the engine writes INSIDE its stderr sentences as a trailing
/// `(calc.something)` rather than as a bare literal.
///
/// Anchored on that parenthesis, exactly as the GUI reads it (`CalcErrorText::KeyOf`), so the two
/// sides agree on what a key is. A `calc.` mentioned mid-prose is not one, and neither is a
/// parenthesis that holds ordinary English.
fn calc_keys(root: &Path) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for path in rust_sources_under(&root.join("crates/cli/src")) {
        collect_calc_keys(&production_source(&path), &mut keys);
    }
    keys
}

/// The calculator keys the GUI's own client writes, in the same `(calc.something)` shape.
///
/// Two failures are born on that side of the process boundary, not in the engine: the engine that
/// could not be started at all, and the one that ran past the client's limit. `CalcErrorText` reads
/// them exactly like an engine key, and the scan above never saw them, so `calc.timeout` showed a
/// Polish window an English sentence for as long as it existed (R4/13).
fn gui_calc_keys(root: &Path) -> BTreeSet<String> {
    let dir = root.join("gui/ChronoMock.Protocol");
    let entries =
        std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
    let mut keys = BTreeSet::new();
    for entry in entries {
        let path = entry.expect("directory entry").path();
        if path.extension().is_some_and(|e| e == "cs") {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
            collect_calc_keys(&text, &mut keys);
        }
    }
    keys
}

/// The key families an engine sentence can end in, as the window's `CalcErrorText` reads them: the
/// calculator's, and since R4/18 the preset catalogue's (`chrono presets`), translated the same way.
const ENGINE_KEY_FAMILIES: [&str; 2] = ["(calc.", "(presets."];

fn collect_calc_keys(text: &str, keys: &mut BTreeSet<String>) {
    for family in ENGINE_KEY_FAMILIES {
        let mut rest = text;
        while let Some(open) = rest.find(family) {
            let after = &rest[open + 1..];
            match after.find(')') {
                Some(close) => {
                    let candidate = &after[..close];
                    if is_key_shaped(candidate) {
                        keys.insert(candidate.to_string());
                    }
                    rest = &after[close..];
                }
                None => break,
            }
        }
    }
}

/// Top-level key names present in a translation file. Parsed rather than substring-matched so a
/// key appearing inside a translated SENTENCE is not mistaken for a defined key.
fn translation_keys(path: &Path) -> BTreeSet<String> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    // The files carry // comments (see the stability audit), which strict JSON rejects, so strip
    // whole-line comments before parsing. This mirrors what LocalizationService does at runtime.
    let cleaned: String = text
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let value: serde_json::Value =
        serde_json::from_str(&cleaned).unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()));
    value
        .as_object()
        .expect("translation file is a JSON object")
        .keys()
        .cloned()
        .collect()
}

#[test]
fn every_wire_key_has_both_translations() {
    let root = repo_root();
    let emitted = emitted_keys(&root);
    assert!(
        emitted.len() > 20,
        "the scanner found only {} keys - the emission shapes changed and this guard went blind",
        emitted.len()
    );

    let en = translation_keys(&root.join("gui/ChronoMock.App/Localization/Strings.en.json"));
    let pl = translation_keys(&root.join("gui/ChronoMock.App/Localization/Strings.pl.json"));

    let missing: Vec<&String> = emitted
        .iter()
        .filter(|k| !en.contains(*k) || !pl.contains(*k))
        .collect();

    assert!(
        missing.is_empty(),
        "these keys reach the wire but have no translation in en and/or pl, so the panel would \
         show raw jargon: {missing:?}\n\
         Add them to gui/ChronoMock.App/Localization/Strings.{{en,pl}}.json (and to CoreWireKeys \
         in LocalizationTests.cs, which mirrors this set for the GUI side)."
    );
}

/// The GUI keeps its own mirror of this set. Two hand-maintained lists drift apart in one
/// direction or the other, so the guard checks that the mirror still covers what the core emits.
#[test]
fn the_gui_mirror_lists_every_emitted_key() {
    let root = repo_root();
    let emitted = emitted_keys(&root);
    let mirror = std::fs::read_to_string(root.join("gui/ChronoMock.App.Tests/LocalizationTests.cs"))
        .expect("LocalizationTests.cs is readable");

    let missing: Vec<&String> = emitted
        .iter()
        .filter(|k| !mirror.contains(&format!("\"{k}\"")))
        .collect();

    assert!(
        missing.is_empty(),
        "CoreWireKeys in LocalizationTests.cs does not list: {missing:?}\n\
         That list is what protects the GUI from a key living only in the core."
    );
}

/// Every calculator refusal the engine names must have a translation the GUI can show.
///
/// The mapping is `calc.X` -> `calc.err.X`, which is what `CalcErrorText.Describe` does. A key with
/// no `calc.err.` entry falls back to the engine's English sentence, which is the state this test
/// exists to stop coming back: eight of nine keys were in it, on a panel that had already decided
/// (rule 15) that user-facing text is a translation key.
#[test]
fn every_calculator_key_has_both_translations() {
    let root = repo_root();
    let mut keys = calc_keys(&root);
    // A literal, not a count derived from the list this checks: a scan that stops finding keys looks
    // exactly like a codebase with none. Nine were measured on 2026-09-09, and the number only grows.
    assert!(
        keys.len() >= 9,
        "the calculator-key scan found only {} keys ({keys:?}) - the emission shape changed and this \
         guard went blind",
        keys.len()
    );
    // The same canary for the client's own two, measured on 2026-10-04.
    let client = gui_calc_keys(&root);
    assert!(
        client.len() >= 2,
        "the scan of the GUI's calculator client found only {} keys ({client:?}) - it went blind",
        client.len()
    );
    keys.extend(client);

    let en = translation_keys(&root.join("gui/ChronoMock.App/Localization/Strings.en.json"));
    let pl = translation_keys(&root.join("gui/ChronoMock.App/Localization/Strings.pl.json"));

    // `family.something` -> `family.err.something`, the mapping `CalcErrorText.Describe` makes. The
    // catalogue's family has to be found too, or this guard would hold half of what it claims.
    assert!(
        keys.iter().any(|k| k.starts_with("presets.")),
        "no `presets.` key was found - the catalogue's refusals are no longer scanned"
    );
    let missing: Vec<String> = keys
        .iter()
        .map(|k| {
            let (family, rest) = k.split_once('.').expect("a key has a family");
            format!("{family}.err.{rest}")
        })
        .filter(|k| !en.contains(k) || !pl.contains(k))
        .collect();

    assert!(
        missing.is_empty(),
        "these calculator refusals have no interface text, so the panel falls back to the engine's \
         English sentence: {missing:?}\n\
         Add them to gui/ChronoMock.App/Localization/Strings.{{en,pl}}.json."
    );
}

/// Every key the core emits must be explained by the CLI report too, not only by the GUI.
///
/// The two tests above hold the GUI to it and nothing held the other client: seven keys reached the
/// wire with both translations and a mirror entry, and `chrono run` printed each of them raw - the
/// jump refusals, both ways a dropped command leaves the session running and both ways the command
/// stream ends it. One more, the vanish most sessions end in, was explained only by a catch-all arm
/// that said the same thing of every reason, known or not.
///
/// What this does NOT check: that the arm stands in the function the key actually travels through. An
/// error key explained only in `describe_warning` passes here and still prints raw. Which function a
/// key reaches depends on the event that carries it, and that is not visible from the key.
///
/// No exception list, on purpose. Every event that carries a key reaches the report through one of
/// these functions, so there is no key the CLI cannot show - only keys it has not explained yet.
#[test]
fn every_wire_key_has_a_gloss_in_the_cli_report() {
    let root = repo_root();
    let emitted = emitted_keys(&root);
    assert!(
        emitted.len() > 20,
        "the scanner found only {} keys - the emission shapes changed and this guard went blind",
        emitted.len()
    );
    let report = production_source(&root.join("crates/cli/src/report.rs"));
    let bodies = gloss_bodies(&report);

    let missing: Vec<&String> = emitted.iter().filter(|k| !has_arm(&bodies, k)).collect();

    assert!(
        missing.is_empty(),
        "these keys reach the wire but the CLI report has no words for them, so `chrono run` prints \
         them raw: {missing:?}\n\
         Add an arm for each to the report.rs function its event goes through (describe_error for an \
         `error` event, describe_warning for warning_keys, describe_reason for a verdict reason, \
         describe_residue for ended.residue_keys, vanish_cause for a vanish)."
    );
}

/// The scan reads a key wherever it is quoted, including inside a raw string that holds JSON - the
/// shape that hid `proto.serialize_failed` - and still refuses text that is not a key.
#[test]
fn the_key_scan_reads_keys_inside_raw_json_strings() {
    let raw = r###"r#"{"type":"error","v":1,"key":"proto.serialize_failed","origin":"proto"}"#,"###;
    assert!(
        keys_in_line(raw).iter().any(|k| k == "proto.serialize_failed"),
        "got {:?}",
        keys_in_line(raw)
    );
    assert_eq!(keys_in_line(r#"key: "moment.invalid".into(),"#), ["moment.invalid"]);
    // Between two literals there is never a key-shaped run all the way to the next quote.
    assert!(keys_in_line(r#"f("a b", "icudtl.dat", "127.0.0.1")"#).is_empty());
}

/// The arm matcher finds a key as a whole arm and as either alternative of a joined one, and a key
/// that is only an arm's value is not a pattern.
#[test]
fn an_arm_is_a_match_pattern_not_a_mention() {
    let bodies = "match k {\n \"a.one\" => \"x\",\n \"a.two\" | \"a.three\" => {\n \"y\"\n }\n \"a.five\" => \"\",\n _ => \"a.four\",\n}\n";
    for key in ["a.one", "a.two", "a.three"] {
        assert!(has_arm(bodies, key), "{key}");
    }
    assert!(!has_arm(bodies, "a.four"), "a key in an arm's VALUE is not an arm");
    assert!(!has_arm(bodies, "a.five"), "an arm with no words prints the key raw, so it is not a gloss");
}
