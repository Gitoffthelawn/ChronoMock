//! The calculator as its callers meet it: the built binary, one invocation at a time.
//!
//! These are the behaviours that live in `calc_run` rather than in a function a unit test can call -
//! what a refused argument looks like on the error stream, and which zone "today" is read in. The GUI
//! reads both, so a regression here shows on its panel and nowhere in the unit tests.

use std::process::{Command, Output};

fn calc(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_chrono"))
        .arg("calc")
        .args(args)
        .output()
        .expect("the tool must run")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).trim().to_string()
}

/// R4-S23: a refused argument used to be a keyless sentence followed by the whole usage, and the GUI
/// showed that usage as the "detail" under its message. With `--json` (which the GUI always passes,
/// last) the refusal is one line ending in its key, and nothing else.
///
/// The cases are the ones the calculator panel can produce from its own fields: an amount that is
/// empty, fractional or signed, a time without seconds, a zone outside the band or in another shape.
#[test]
fn a_refused_argument_is_one_keyed_line_when_json_is_asked() {
    let cases: [&[&str]; 6] = [
        &["--shift", "+d"],
        &["--shift", "+1.5d"],
        &["--shift", "+-5d"],
        &["--set-time", "23:59"],
        &["--to-zone", "+15:00"],
        &["--to-zone", "+5"],
    ];
    for case in cases {
        let argv = [&["--base", "today"][..], case, &["--json"][..]].concat();
        let out = calc(&argv);
        let said = stderr(&out);
        assert_eq!(out.status.code(), Some(1), "{case:?}: {said}");
        assert!(out.stdout.is_empty(), "{case:?}: a refusal writes no result");
        assert_eq!(said.lines().count(), 1, "{case:?} must be one line: {said}");
        assert!(said.starts_with("chrono calc: "), "{case:?}: {said}");
        assert!(said.ends_with("(calc.bad_argument)"), "{case:?} must end in its key: {said}");
    }
}

/// A person at a terminal still gets the usage under the refusal - it is only the machine caller that
/// does not want it.
#[test]
fn without_json_the_usage_follows_the_refusal() {
    let out = calc(&["--shift", "+abcd"]);
    let said = stderr(&out);
    assert_eq!(out.status.code(), Some(1), "{said}");
    let first = said.lines().next().unwrap_or_default();
    assert!(first.ends_with("(calc.bad_argument)"), "{said}");
    assert!(said.contains("usage: chrono calc"), "{said}");
}

/// R4-N31: "days from now" compared the result's date, read in the zone after `--to-zone`, with today
/// read in the session zone. From -12:00 to +14:00 the very same instant was a day away. Twenty-six
/// hours apart, the two zones never share a date, so the old reading can never pass this.
#[test]
fn days_from_now_counts_from_today_in_the_result_zone() {
    for (session, result) in [("-12:00", "+14:00"), ("+14:00", "-12:00")] {
        let out = calc(&["--zone", session, "--base", "now", "--to-zone", result, "--json"]);
        assert_eq!(out.status.code(), Some(0), "{session} -> {result}: {}", stderr(&out));
        let doc: serde_json::Value = serde_json::from_slice(&out.stdout).expect("calc --json is JSON");
        assert_eq!(
            doc["moment"]["metadata"]["days_from_today"], 0,
            "now re-expressed from {session} in {result} is still today: {doc}"
        );

        // The text output reads the same field.
        let out = calc(&["--zone", session, "--base", "now", "--to-zone", result]);
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("days from now today"), "{session} -> {result}:\n{text}");
    }
}

/// A scratch catalogue: `calendars/` and `presets/` holding the given files, byte for byte. Nothing
/// sits beside the binary under test, so it reads both from its working directory, which is this.
fn catalogue(name: &str, files: &[(&str, Vec<u8>)]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("chrono-calc-cli-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, bytes) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().expect("a folder")).expect("the catalogue folder");
        std::fs::write(&path, bytes).expect("a catalogue file");
    }
    dir
}

fn calc_in(dir: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_chrono"))
        .current_dir(dir)
        .arg("calc")
        .args(args)
        .args(["--zone", "+00:00"])
        .output()
        .expect("the tool must run")
}

/// R4-N37 and R4-N33 as a user meets them, through `--calendar` and `--preset`. A file saved with a
/// byte order mark loads (it used to be "expected value at line 1 column 1"), a file whose id is not
/// its name is refused naming both (it used to be loaded and quoted under the id inside it), a broken
/// preset names its file (a calendar's error always did), a preset that says two things at once is
/// refused instead of settled by whichever field the reader looked at first, and so is one whose moment
/// names a parameter it does not declare.
#[test]
fn a_catalogue_file_loads_as_saved_and_is_named_when_refused() {
    let calendar = |id: &str| {
        format!(
            r#"{{"schema":"chronomock.calendar/1","id":"{id}","country":"XX","weekend":["saturday","sunday"],"observed":"none","valid_from":2000,"law_as_of":"2026-01-01","source":"t","holidays":[]}}"#
        )
        .into_bytes()
    };
    let preset = |id: &str, base: &str| {
        format!(
            r#"{{"schema":"chronomock.preset/1","id":"{id}","name":{{"en":"n","pl":"n"}},"explains":{{"en":"e","pl":"e"}},"applies_to":"calculator","parameters":[{{"id":"d","type":"date","default":"2020-01-01"}}],"moment":{{"base":{base}}}}}"#
        )
        .into_bytes()
    };
    let with_bom = |bytes: Vec<u8>| [vec![0xEF, 0xBB, 0xBF], bytes].concat();
    let dir = catalogue(
        "load",
        &[
            ("calendars/bom.json", with_bom(calendar("bom"))),
            ("calendars/other.json", calendar("pl")),
            ("presets/bom.json", with_bom(preset("bom", r#"{"parameter":"d"}"#))),
            ("presets/renamed.json", preset("month-end", r#"{"parameter":"d"}"#)),
            ("presets/broken.json", br#"{"schema":"chronomock.preset/1","id":"broken""#.to_vec()),
            ("presets/both.json", preset("both", r#"{"absolute":"2030-01-01T00:00:00","parameter":"d"}"#)),
            ("presets/undeclared.json", preset("undeclared", r#"{"parameter":"e"}"#)),
        ],
    );

    let out = calc_in(&dir, &["--calendar", "bom", "--base", "2026-07-01T12:00:00", "--shift", "+1bd"]);
    assert_eq!(out.status.code(), Some(0), "a calendar with a byte order mark: {}", stderr(&out));
    let out = calc_in(&dir, &["--preset", "bom"]);
    assert_eq!(out.status.code(), Some(0), "a preset with a byte order mark: {}", stderr(&out));

    let out = calc_in(&dir, &["--calendar", "other", "--base", "2026-07-01T12:00:00"]);
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(err.contains("'pl'") && err.contains("'other'") && err.contains("other.json"), "{err}");
    let out = calc_in(&dir, &["--preset", "renamed"]);
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(err.contains("'month-end'") && err.contains("'renamed'"), "{err}");

    let out = calc_in(&dir, &["--preset", "broken"]);
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(err.contains("broken.json"), "a broken preset names its file: {err}");

    let out = calc_in(&dir, &["--preset", "both"]);
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(err.contains("'absolute' and 'parameter'"), "the contradiction is named: {err}");

    // A base naming a parameter the file does not declare used to say "has no value", and passing the
    // value was then refused as an unknown parameter. With or without it, the file is what is refused.
    for args in [&["--preset", "undeclared"][..], &["--preset", "undeclared", "--param", "e=2030-01-01"]] {
        let out = calc_in(&dir, args);
        let err = stderr(&out);
        assert_eq!(out.status.code(), Some(1), "{err}");
        assert!(
            err.contains("parameter 'e'") && err.contains("does not declare") && err.contains("undeclared.json"),
            "the undeclared parameter and its file are named: {err}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// R4/18 (D5): a preset with a market counts its business days in that market's calendar, as the
/// window's calculator does when the preset is chosen and `run --preset` has since R4-S12. This path
/// used to stop with "pass --calendar" (exit 5) while the window computed the same preset. A calendar
/// named on the command line still wins, as one picked in the window does, and a preset with no market
/// still needs one.
#[test]
fn a_market_preset_counts_in_its_markets_calendar_unless_another_is_named() {
    // Two calendars that disagree about Saturday: in `us-banking` it is a weekend, in `pl` here it is
    // not. Friday 2026-07-03 plus one business day is then Monday in one and Saturday in the other.
    let calendar = |id: &str, weekend: &str| {
        format!(
            r#"{{"schema":"chronomock.calendar/1","id":"{id}","country":"XX","weekend":[{weekend}],"observed":"none","valid_from":2000,"law_as_of":"2026-01-01","source":"t","holidays":[]}}"#
        )
        .into_bytes()
    };
    let preset = |id: &str, market: &str| {
        format!(
            r#"{{"schema":"chronomock.preset/1","id":"{id}","name":{{"en":"n"}},"explains":{{"en":"e"}},"applies_to":"calculator",{market}"moment":{{"base":{{"absolute":"2026-07-03T00:00:00"}},"steps":[{{"shift":{{"sign":"+","amount":1,"unit":"business_days"}}}}]}}}}"#
        )
        .into_bytes()
    };
    let dir = catalogue(
        "market",
        &[
            ("calendars/us-banking.json", calendar("us-banking", r#""saturday","sunday""#)),
            ("calendars/pl.json", calendar("pl", r#""sunday""#)),
            ("presets/due-us.json", preset("due-us", r#""market":"us","#)),
            ("presets/due-nowhere.json", preset("due-nowhere", "")),
        ],
    );
    let moment = |out: &Output| {
        let doc: serde_json::Value = serde_json::from_slice(&out.stdout).expect("calc --json is JSON");
        (doc["moment"]["iso"].as_str().unwrap_or_default().to_string(), doc["moment"]["metadata"]["calendar"].clone())
    };

    let out = calc_in(&dir, &["--preset", "due-us", "--json"]);
    assert_eq!(out.status.code(), Some(0), "the market's calendar is used: {}", stderr(&out));
    assert_eq!(moment(&out), ("2026-07-06T00:00:00".to_string(), serde_json::json!("us-banking")));

    let out = calc_in(&dir, &["--preset", "due-us", "--calendar", "pl", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(moment(&out), ("2026-07-04T00:00:00".to_string(), serde_json::json!("pl")), "a named calendar wins");

    // The text says which calendar it counted in and where that came from, since nobody named it.
    let out = calc_in(&dir, &["--preset", "due-us"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("calendar: us-banking, from the preset's market (us)"), "{text}");

    let out = calc_in(&dir, &["--preset", "due-nowhere", "--json"]);
    assert_eq!(out.status.code(), Some(5), "{}", stderr(&out));
    assert!(stderr(&out).contains("calc.needs_calendar"), "{}", stderr(&out));

    let _ = std::fs::remove_dir_all(&dir);
}

/// R4-S20 and R4/20 as a user meets them. The shipped Polish calendar refuses a business-day step that
/// walks into 2001 (exit 1, with the year), answers for a date before 2002 without judging it (no
/// business day, no holiday, the `before_calendar` mark), and a calendar checked against the law long
/// before today marks a later result `calendar_outdated` - the case of a package updated once and used
/// for years. The old check date is 2020, so the last condition holds on any machine whose clock is
/// past 2021.
#[test]
fn a_calendar_answers_only_for_its_years_and_says_when_it_is_old() {
    let shipped = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../calendars/pl.json")).expect("the shipped pl");
    let old = old_polish_calendar(&shipped);
    let dir = catalogue("range", &[("calendars/pl.json", shipped.into_bytes()), ("calendars/old.json", old.into_bytes())]);

    let out = calc_in(&dir, &["--calendar", "pl", "--base", "2002-01-02T00:00:00", "--shift", "-1bd"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("(calc.before_calendar)") && stderr(&out).contains("2002"), "{}", stderr(&out));

    let out = calc_in(&dir, &["--calendar", "pl", "--base", "1985-05-03T00:00:00", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).expect("calc --json is JSON");
    let metadata = &doc["moment"]["metadata"];
    assert!(metadata["business_day"].is_null() && metadata["holiday"].is_null(), "{doc}");
    assert_eq!(metadata["calendar_valid_from"], 2002, "{doc}");
    assert_eq!(doc["moment"]["significance"], serde_json::json!(["before_calendar"]), "{doc}");

    let out = calc_in(&dir, &["--calendar", "old", "--base", "2026-07-01T12:00:00", "--shift", "+1bd", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).expect("calc --json is JSON");
    assert!(doc["moment"]["significance"].as_array().is_some_and(|marks| marks.contains(&serde_json::json!("calendar_outdated"))), "{doc}");
    assert_eq!(doc["moment"]["metadata"]["calendar_law_as_of"], "2020-01-01", "{doc}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// R4/20: a walk back from a date after the check can end before it and still have judged days the
/// check never saw - the mark follows the days the step walked over, not only where it landed.
#[test]
fn a_walk_over_days_after_an_old_check_is_marked_even_when_it_ends_before_it() {
    let shipped = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../calendars/pl.json")).expect("the shipped pl");
    let dir = catalogue("reach", &[("calendars/old.json", old_polish_calendar(&shipped).into_bytes())]);
    let out = calc_in(&dir, &["--calendar", "old", "--base", "2020-01-10T00:00:00", "--shift", "-10bd", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).expect("calc --json is JSON");
    assert!(doc["moment"]["iso"].as_str().is_some_and(|iso| iso < "2020-01-01"), "the walk ends before the check: {doc}");
    assert!(doc["moment"]["significance"].as_array().is_some_and(|marks| marks.contains(&serde_json::json!("calendar_outdated"))), "{doc}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The shipped Polish calendar under the id `old`, checked against the law on 2020-01-01 - whatever day
/// the shipped file itself was last checked on, so the tests that use it do not move with that date.
fn old_polish_calendar(shipped: &str) -> String {
    let renamed = shipped.replacen(r#""id": "pl""#, r#""id": "old""#, 1);
    let key = r#""law_as_of": ""#;
    let at = renamed.find(key).expect("the shipped calendar has a check date") + key.len();
    let old = format!("{}2020-01-01{}", &renamed[..at], &renamed[at + "YYYY-MM-DD".len()..]);
    assert!(old.contains(r#""id": "old""#) && old != shipped, "the copy has to differ from the shipped file");
    old
}
