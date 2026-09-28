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
