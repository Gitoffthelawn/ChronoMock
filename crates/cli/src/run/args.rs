//! What the user typed, turned into a `RunArgs` and nothing more.
//!
//! Parsing lives apart from driving on purpose. Every value the core receives is decided here or in
//! the moment resolver beside it, so a flag can be read, rejected and tested without starting a
//! process. The struct's fields are open to the rest of `run` and to nothing else.

use std::collections::HashMap;

use chrono_proto::{TargetConsole, TargetSpec};

use crate::zone::parse_zone_to_bias;

pub(crate) struct RunArgs {
    pub(super) target: String,
    pub(super) args: Vec<String>,
    /// Working directory for the target (`--cwd`). `None` means "do not ask for one", and the target
    /// then inherits ours - the behaviour every run had before this flag existed.
    pub(super) cwd: Option<String>,
    pub(super) at: Option<String>,
    pub(super) zone_bias_min: Option<i32>,
    /// Wire mode token: "flow", "frozen", or "multiplier".
    pub(super) mode: String,
    pub(super) multiplier: Option<i64>,
    pub(super) scale_duration: bool,
    /// Also scale QueryPerformanceCounter (ADR-2 reversal, opt-in `--scale-qpc`).
    pub(super) scale_qpc: bool,
    /// Run even when the opening verdict says the substitution did not take effect (`--force`).
    pub(super) force: bool,
    /// Reach the web pages inside the application through its embedded engine's debugging port
    /// (docs/09). On unless `--no-embedded` - the opt-out for a tester who does not want that port
    /// open in their application for the session.
    pub(super) embedded: bool,
    /// Also reach the web pages of an application that runs as administrator (`--elevated-embedded`,
    /// docs/09 section 12.19): the session writes one WebView2 value to the machine registry for its
    /// duration and removes it. Off by default and exclusive of `--no-embedded`.
    pub(super) elevated_embedded: bool,
    /// How many `state` heartbeats to stream before ending. 0 = no cut: the session lasts until the
    /// target, and whatever it started on the session clock, has exited (ADR-16) - or, on the
    /// Chromium path, until the browser closes its debugging connection (R4-S18).
    pub(super) ticks: u64,
    /// After the Nth state heartbeat, send set_multiplier M (in-flight speed change).
    pub(super) set_after: Option<(u64, i64)>,
    /// After the Nth state heartbeat, jump the wall clock to the given moment.
    pub(super) jump_after: Option<(u64, String)>,
    /// Optional path to write the human evidence report to, in addition to stdout.
    pub(super) report: Option<String>,
    pub(super) json: bool,
    /// A named preset id (docs/04 4.3): the moment AND the time mode come from `presets/<id>.json`
    /// instead of --at/--mode. None = build them from the flags. Exclusive of --at/--mode/--scale-duration.
    pub(super) preset: Option<String>,
    /// Preset parameter values from `--param id=value` (docs/04 4.2). Only meaningful with --preset.
    /// In run, a `target_file_creation` hint also resolves from the target's file date.
    pub(super) params: HashMap<String, String>,
    /// Resolve everything and start nothing (`--dry-run`). The session is described rather than run:
    /// no core process, no target, no evidence file, and no verdict to report.
    pub(super) dry_run: bool,
    /// Give up after this many seconds of wall time, whatever the session is doing (`--timeout`).
    /// None = no ceiling, which stays the default because the normal way to bound a run is
    /// `--ticks`, and a session driving a real app has no business being cut off by surprise.
    pub(super) timeout_secs: Option<u64>,
}

/// Parse `--mode` into a wire mode token and optional multiplier.
/// `flow` = real speed, `frozen` = held, `xN` = accelerated N times (N >= 1).
pub(crate) fn parse_mode(raw: &str) -> Result<(String, Option<i64>), String> {
    match raw {
        "flow" => Ok(("flow".into(), None)),
        "frozen" => Ok(("frozen".into(), None)),
        _ => {
            let n = raw
                .strip_prefix('x')
                .or_else(|| raw.strip_prefix('X'))
                .ok_or_else(|| format!("mode must be flow, frozen, or xN like x60, got '{raw}'"))?;
            let m: i64 = n
                .parse()
                .map_err(|_| format!("bad multiplier in mode '{raw}'"))?;
            // The friendly surface keeps its own floor of 1: `x0` here would be a confusing way to
            // spell `--mode frozen`, which already exists. The ceiling is the shared one, so the CLI
            // and the protocol agree on what a session may run at.
            if m < 1 {
                return Err(format!("multiplier must be >= 1, got '{raw}'"));
            }
            if m > chrono_core::MULTIPLIER_MAX {
                return Err(format!(
                    "multiplier must be <= {}, got '{raw}' - past that the fake clock leaves the \
                     representable date range mid-session and the time channels start disagreeing",
                    chrono_core::MULTIPLIER_MAX
                ));
            }
            Ok(("multiplier".into(), Some(m)))
        }
    }
}

/// Split an `--args` value into arguments, honouring double-quotes so one argument may contain
/// spaces (`--args '"a b" c'` -> ["a b", "c"]). Whitespace outside quotes separates arguments - a
/// quote toggles quoting and is dropped. Plain space-separated values behave exactly as before, so
/// existing usage is unchanged - `mech`'s quoting re-quotes each token for the target's CRT (P9).
pub(crate) fn split_args(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut has_token = false;
    for c in raw.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                has_token = true;
            }
            c if c.is_whitespace() && !in_quotes => {
                if has_token {
                    out.push(std::mem::take(&mut cur));
                    has_token = false;
                }
            }
            c => {
                cur.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        out.push(cur);
    }
    out
}

/// What the driver puts in `start.target`. Separated from the command so a test can see it: the
/// three fields used to be written inline with `cwd` hard-coded to `None`, and putting that back
/// changed nothing any test could notice - measured, by reverting it.
pub(crate) fn target_spec_for(ra: &RunArgs) -> TargetSpec {
    TargetSpec {
        path: ra.target.clone(),
        args: ra.args.clone(),
        cwd: ra.cwd.clone(),
        embedded: ra.embedded,
        elevated_embedded: ra.elevated_embedded,
        // The terminal `chrono run` was started from: its console for input, its stderr for output
        // (R4-D15). Named rather than left to the default, so the driver's choice is visible here.
        console: TargetConsole::Shared,
    }
}

/// What stops `--elevated-embedded` before anything starts, or `None`. The option writes the machine
/// registry, which only an administrator can do, so from a shell that is not one it is refused with the
/// same code a bad flag gets - and a dry run refuses it too, because a plan that described a command
/// the run would then refuse would be describing nothing. A token that could not be read counts as not
/// elevated: the refusal is the safe side of not knowing.
pub(crate) fn elevation_problem(ra: &RunArgs, elevated: Option<bool>) -> Option<&'static str> {
    if ra.elevated_embedded && elevated != Some(true) {
        Some("--elevated-embedded needs Chrono Mock to run as administrator, and it is not: it writes a WebView2 value to the machine registry")
    } else {
        None
    }
}

/// The value of `--set-after`: `<tick>:<multiplier>`, with the multiplier under the same invariant as
/// `--mode xN` (`parse_mode`) - an in-flight multiplier is >= 1 and within the clock's range. A zero or
/// negative value would freeze or run the wall clock backward as a silent side effect of an
/// unvalidated surface (rule 4) - freezing in flight is not a feature here.
fn parse_set_after(raw: &str) -> Result<(u64, i64), String> {
    let (t, m) = raw.split_once(':').ok_or("--set-after must be <tick>:<multiplier>")?;
    let tick: u64 = t.parse().map_err(|_| format!("bad tick in '{raw}'"))?;
    let mult: i64 = m.parse().map_err(|_| format!("bad multiplier in '{raw}'"))?;
    if mult < 1 {
        return Err(format!("--set-after multiplier must be >= 1, got '{raw}'"));
    }
    if mult > chrono_core::MULTIPLIER_MAX {
        return Err(format!("--set-after multiplier must be <= {}, got '{raw}'", chrono_core::MULTIPLIER_MAX));
    }
    Ok((tick, mult))
}

/// The flags that cannot stand together, refused rather than resolved by picking one silently.
fn check_combinations(
    has_preset: bool,
    saw_time_flag: bool,
    has_params: bool,
    embedded: bool,
    elevated_embedded: bool,
) -> Result<(), String> {
    // A preset supplies both the moment and the time mode, so combining it with --at/--mode/
    // --scale-duration would mean two sources.
    if has_preset && saw_time_flag {
        return Err("--preset supplies the moment and mode; it cannot be combined with \
                    --at/--mode/--scale-duration"
            .into());
    }
    // --param only makes sense with --preset (it fills a preset's declared parameters).
    if has_params && !has_preset {
        return Err("--param needs --preset (parameters belong to a preset)".into());
    }
    // One asks for the pages of an application that runs as administrator to be reached, the other
    // for the pages to be left alone - saying both is a contradiction, not a setting.
    if elevated_embedded && !embedded {
        return Err("--elevated-embedded cannot be combined with --no-embedded".into());
    }
    Ok(())
}

pub(crate) fn parse_run_args(argv: &[String]) -> Result<RunArgs, String> {
    let mut target: Option<String> = None;
    let mut args: Vec<String> = Vec::new();
    let mut cwd: Option<String> = None;
    let mut at: Option<String> = None;
    let mut zone_bias_min: Option<i32> = None;
    let mut mode = String::from("flow");
    let mut multiplier: Option<i64> = None;
    let mut scale_duration = false;
    let mut scale_qpc = false;
    let mut embedded = true;
    let mut elevated_embedded = false;
    let mut force = false;
    let mut dry_run = false;
    let mut ticks: u64 = 0;
    let mut timeout_secs: Option<u64> = None;
    let mut set_after: Option<(u64, i64)> = None;
    let mut jump_after: Option<(u64, String)> = None;
    let mut json = false;
    let mut report: Option<String> = None;
    let mut preset: Option<String> = None;
    let mut params: HashMap<String, String> = HashMap::new();
    // Whether any moment/mode flag appeared, so `--preset` (which supplies both) can reject being
    // combined with them instead of silently ignoring one source.
    let mut saw_time_flag = false;

    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--at" => {
                i += 1;
                at = Some(argv.get(i).ok_or("--at needs a value")?.clone());
                saw_time_flag = true;
            }
            "--preset" => {
                i += 1;
                preset = Some(argv.get(i).ok_or("--preset needs an id like month-end")?.clone());
            }
            "--param" => {
                i += 1;
                let raw = argv.get(i).ok_or("--param needs id=value like start_date=2026-01-01")?;
                let (id, value) = raw
                    .split_once('=')
                    .ok_or_else(|| format!("--param must be id=value, got '{raw}'"))?;
                if id.is_empty() {
                    return Err(format!("--param needs a non-empty id, got '{raw}'"));
                }
                params.insert(id.to_string(), value.to_string());
            }
            "--zone" => {
                i += 1;
                let raw = argv.get(i).ok_or("--zone needs a value like +02:00")?;
                zone_bias_min = Some(parse_zone_to_bias(raw)?);
            }
            "--mode" => {
                i += 1;
                let raw = argv.get(i).ok_or("--mode needs a value like x60")?;
                let (m, mult) = parse_mode(raw)?;
                mode = m;
                multiplier = mult;
                saw_time_flag = true;
            }
            "--args" => {
                i += 1;
                let raw = argv.get(i).ok_or("--args needs a value")?;
                args = split_args(raw);
            }
            "--cwd" => {
                i += 1;
                let raw = argv.get(i).ok_or("--cwd needs a value")?;
                // An explicitly empty value is a usage error, not "no directory". The two mean
                // different things on the wire (absent vs present-and-empty) and only one of them is
                // something CreateProcessW can be given.
                if raw.trim().is_empty() {
                    return Err("--cwd needs a directory - omit the flag to start where the tool does".into());
                }
                cwd = Some(raw.clone());
            }
            "--scale-duration" => {
                scale_duration = true;
                saw_time_flag = true;
            }
            "--force" => {
                force = true;
            }
            "--no-embedded" => {
                embedded = false;
            }
            "--elevated-embedded" => {
                elevated_embedded = true;
            }
            "--dry-run" => {
                dry_run = true;
            }
            "--scale-qpc" => {
                // ADR-2 reversal, opt-in. NOT a preset-exclusive time flag: a preset carries its own
                // scale_duration but never scale_qpc, so --scale-qpc is the only source and composes with
                // --preset (unlike --scale-duration, which would double a preset's own setting).
                scale_qpc = true;
            }
            "--ticks" => {
                i += 1;
                let raw = argv.get(i).ok_or("--ticks needs a value")?;
                ticks = raw.parse().map_err(|_| format!("bad --ticks value '{raw}'"))?;
            }
            "--timeout" => {
                i += 1;
                let raw = argv.get(i).ok_or("--timeout needs a value in seconds")?;
                let secs: u64 = raw.parse().map_err(|_| format!("bad --timeout value '{raw}'"))?;
                if secs == 0 {
                    return Err("--timeout must be at least 1 second".into());
                }
                timeout_secs = Some(secs);
            }
            "--set-after" => {
                i += 1;
                let raw = argv.get(i).ok_or("--set-after needs <tick>:<multiplier>")?;
                set_after = Some(parse_set_after(raw)?);
            }
            "--jump-after" => {
                i += 1;
                let raw = argv.get(i).ok_or("--jump-after needs <tick>:<moment>")?;
                let (t, mom) = raw
                    .split_once(':')
                    .ok_or("--jump-after must be <tick>:<moment>")?;
                jump_after = Some((t.parse().map_err(|_| format!("bad tick in '{raw}'"))?, mom.to_string()));
            }
            "--json" => json = true,
            "--report" => {
                i += 1;
                report = Some(argv.get(i).ok_or("--report needs a path")?.clone());
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown flag '{other}'"));
            }
            other => {
                if target.is_none() {
                    target = Some(other.to_string());
                } else {
                    return Err(format!("unexpected argument '{other}'"));
                }
            }
        }
        i += 1;
    }

    check_combinations(preset.is_some(), saw_time_flag, !params.is_empty(), embedded, elevated_embedded)?;

    Ok(RunArgs {
        target: target.ok_or("missing <target>")?,
        args,
        cwd,
        at,
        zone_bias_min,
        mode,
        multiplier,
        scale_duration,
        scale_qpc,
        force,
        embedded,
        elevated_embedded,
        dry_run,
        ticks,
        timeout_secs,
        set_after,
        jump_after,
        report,
        json,
        preset,
        params,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_flow_and_frozen_carry_no_multiplier() {
        assert_eq!(parse_mode("flow").unwrap(), ("flow".to_string(), None));
        assert_eq!(parse_mode("frozen").unwrap(), ("frozen".to_string(), None));
    }

    #[test]
    fn mode_accepts_xn_either_case() {
        assert_eq!(parse_mode("x60").unwrap(), ("multiplier".to_string(), Some(60)));
        assert_eq!(parse_mode("X1440").unwrap(), ("multiplier".to_string(), Some(1440)));
    }

    #[test]
    fn mode_rejects_unknown_and_nonpositive() {
        assert!(parse_mode("fast").is_err());
        assert!(parse_mode("x0").is_err());
        assert!(parse_mode("x-5").is_err());
        assert!(parse_mode("xabc").is_err());
        // Upper bound (R2-K2): the largest accepted rate passes, one past it does not. Above this
        // the fake clock leaves the representable range mid-session and the channels disagree.
        assert!(parse_mode(&format!("x{}", chrono_core::MULTIPLIER_MAX)).is_ok());
        assert!(parse_mode(&format!("x{}", chrono_core::MULTIPLIER_MAX + 1)).is_err());
    }

    #[test]
    fn set_after_multiplier_must_be_at_least_one() {
        // The in-flight multiplier gets the same >= 1 rule as --mode xN, instead of silently
        // accepting a zero/negative that would freeze or reverse the wall clock.
        let ok = parse_run_args(&["t".into(), "--set-after".into(), "5:60".into()]).unwrap();
        assert_eq!(ok.set_after, Some((5, 60)));
        assert!(parse_run_args(&["t".into(), "--set-after".into(), "5:0".into()]).is_err());
        assert!(parse_run_args(&["t".into(), "--set-after".into(), "5:-3".into()]).is_err());
    }

    /// `--cwd` closes an asymmetry that lasted from the day the protocol was written: `TargetSpec.cwd`
    /// has always been on the wire and `chrono-mech` has always handed it to CreateProcessW, but no
    /// surface offered it - the CLI had `--args` and nothing else, and the driver hard-coded `cwd: None`.
    #[test]
    fn cwd_is_parsed_from_the_command_line() {
        let ok = parse_run_args(&["app.exe".into(), "--cwd".into(), r"C:\work".into()]).unwrap();
        assert_eq!(ok.cwd.as_deref(), Some(r"C:\work"));

        // Absent stays absent: "do not ask for a directory" is not the same as asking for one.
        let none = parse_run_args(&["app.exe".into()]).unwrap();
        assert_eq!(none.cwd, None);
    }

    /// The half the parser test does NOT cover. The driver used to write `cwd: None` inline, so a
    /// parser that read the flag perfectly still sent nothing - and reverting that line failed no
    /// test at all until this one existed.
    #[test]
    fn the_driver_puts_the_parsed_cwd_on_the_wire() {
        let ra = parse_run_args(&["app.exe".into(), "--cwd".into(), "C:/work".into()]).unwrap();
        assert_eq!(target_spec_for(&ra).cwd.as_deref(), Some("C:/work"));

        let bare = parse_run_args(&["app.exe".into()]).unwrap();
        assert_eq!(target_spec_for(&bare).cwd, None);
    }

    /// Reaching the web pages inside the application is on unless the tester opts out - the flag is
    /// the one thing the driver has to send, and a bare run carries the default to the wire.
    #[test]
    fn no_embedded_is_the_opt_out_and_the_default_reaches_the_pages() {
        let bare = parse_run_args(&["app.exe".into()]).unwrap();
        assert!(target_spec_for(&bare).embedded, "a bare run reaches the pages inside the app");

        let off = parse_run_args(&["app.exe".into(), "--no-embedded".into()]).unwrap();
        assert!(!off.embedded);
        assert!(!target_spec_for(&off).embedded, "the opt-out has to reach the wire");
    }

    /// The option that writes the machine registry is off unless asked for, says so on the wire, and
    /// cannot be asked for together with the flag that leaves the pages alone.
    #[test]
    fn elevated_embedded_is_opt_in_and_reaches_the_wire() {
        let bare = parse_run_args(&["app.exe".into()]).unwrap();
        assert!(!bare.elevated_embedded, "a bare run writes nothing to the registry");
        assert!(!target_spec_for(&bare).elevated_embedded);

        let on = parse_run_args(&["app.exe".into(), "--elevated-embedded".into()]).unwrap();
        assert!(on.elevated_embedded);
        assert!(target_spec_for(&on).elevated_embedded, "the opt-in has to reach the wire");
        assert!(target_spec_for(&on).embedded, "and it does not turn the channel off");

        let both = parse_run_args(&["app.exe".into(), "--elevated-embedded".into(), "--no-embedded".into()]);
        assert!(both.err().unwrap().contains("cannot be combined"), "either order is a contradiction");
        assert!(parse_run_args(&["app.exe".into(), "--no-embedded".into(), "--elevated-embedded".into()]).is_err());
    }

    /// A shell that is not an administrator is refused, a dry run included, and one whose token could not
    /// be read is treated as not being one. Without the option nothing is asked of the token at all.
    #[test]
    fn elevated_embedded_is_refused_unless_the_shell_is_known_to_be_elevated() {
        let on = parse_run_args(&["app.exe".into(), "--elevated-embedded".into()]).unwrap();
        assert!(elevation_problem(&on, Some(true)).is_none());
        assert!(elevation_problem(&on, Some(false)).is_some());
        assert!(elevation_problem(&on, None).is_some(), "unknown is not elevated");
        let off = parse_run_args(&["app.exe".into()]).unwrap();
        for token in [Some(true), Some(false), None] {
            assert!(elevation_problem(&off, token).is_none(), "without the option there is nothing to refuse");
        }
    }

    /// An explicitly empty value is a usage error rather than a quiet "no directory". The two are
    /// different on the wire (absent vs present-and-empty), and only absent is something the
    /// mechanism can act on - CreateProcessW cannot be given an empty directory.
    #[test]
    fn an_empty_cwd_is_refused_rather_than_read_as_no_directory() {
        assert!(parse_run_args(&["app.exe".into(), "--cwd".into(), String::new()]).is_err());
        assert!(parse_run_args(&["app.exe".into(), "--cwd".into(), "   ".into()]).is_err());
        assert!(parse_run_args(&["app.exe".into(), "--cwd".into()]).is_err());
    }

    #[test]
    fn args_split_plain_and_quoted() {
        let s = |x: &str| x.to_string();
        // Plain space-separated values behave exactly as before (backward compatible).
        assert_eq!(split_args("a b c"), vec![s("a"), s("b"), s("c")]);
        assert_eq!(split_args("  x   y "), vec![s("x"), s("y")]);
        assert_eq!(split_args("out 100 10"), vec![s("out"), s("100"), s("10")]);
        // Double-quotes group a single argument that contains spaces (the P9 fix).
        assert_eq!(split_args("\"a b\" c"), vec![s("a b"), s("c")]);
        assert!(split_args("").is_empty());
        assert_eq!(split_args("\"\""), vec![s("")]); // an explicit empty argument
    }

    /// `run --preset` supplies the moment and mode, so combining it with a time flag is a usage
    /// error - alone (with a target) it parses and carries the id.
    #[test]
    fn run_preset_flag_is_exclusive_of_time_flags() {
        let ok = parse_run_args(&["--preset".into(), "month-end".into(), "app.exe".into()]).unwrap();
        assert_eq!(ok.preset.as_deref(), Some("month-end"));
        assert_eq!(ok.target, "app.exe");
        assert!(parse_run_args(&["--preset".into(), "m".into(), "--at".into(), "2020-01-01T00:00:00".into(), "app.exe".into()]).is_err());
        assert!(parse_run_args(&["--preset".into(), "m".into(), "--mode".into(), "x60".into(), "app.exe".into()]).is_err());
        assert!(parse_run_args(&["--preset".into(), "m".into(), "--scale-duration".into(), "app.exe".into()]).is_err());
    }

    /// --param in run needs --preset too.
    #[test]
    fn run_param_needs_preset() {
        assert!(parse_run_args(&["--param".into(), "start_date=2026-01-01".into(), "app.exe".into()]).is_err());
        let ok = parse_run_args(&[
            "--preset".into(),
            "trial-first-day-after".into(),
            "--param".into(),
            "start_date=2026-01-01".into(),
            "app.exe".into(),
        ])
        .unwrap();
        assert_eq!(ok.params.get("start_date").map(String::as_str), Some("2026-01-01"));
    }
}
