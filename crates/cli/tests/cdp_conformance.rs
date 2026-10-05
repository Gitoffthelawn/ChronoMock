//! Opt-in end-to-end CDP conformance (RELEASE-005).
//!
//! No portable CDP target ships with the repo. The `is_chromium_target` detection keys off an
//! Electron/Chromium app's runtime files (`icudtl.dat` + a V8 snapshot) sitting BESIDE the exe, which an
//! installed browser does not satisfy - browsers keep those in a versioned subfolder, not next to the
//! launcher - and some hardened Electron apps refuse the remote-debugging port outright. So there
//! is no fixed target a CI job could rely on. This test is therefore PARAMETERISED: point it at a permissive
//! Chromium/Electron target and it drives the shipped `chrono` binary end to end (launch -> attach -> shim
//! -> the page's own JS `Date`), asserting the page reads the faked clock rather than the real one.
//!
//! It is `#[ignore]` (it does not run in a normal `cargo test`, since it launches a real browser) and skips
//! cleanly when the target is absent or not Chromium-shaped, so it is safe to invoke anywhere.
//!
//! Run it:
//!   set CHRONO_CDP_TARGET=C:\path\to\electron-app.exe
//!   cargo test -p chrono-cli --test cdp_conformance -- --ignored
//!
//! The full chain was verified by hand against a real Electron application during the CDP bring-up - see
//! the README support matrix, "Electron / Chromium ... measured by hand on an Electron app (x64)". This
//! harness makes that repeatable against any permissive target.

use std::process::Command;

#[test]
#[ignore = "opt-in: set CHRONO_CDP_TARGET to a permissive Chromium/Electron exe; it launches that app"]
fn cdp_shim_fakes_the_page_clock() {
    let Ok(target) = std::env::var("CHRONO_CDP_TARGET") else {
        eprintln!("CHRONO_CDP_TARGET not set - skipping (no portable CDP target ships with the repo)");
        return;
    };

    let output = Command::new(env!("CARGO_BIN_EXE_chrono"))
        .args(["__cdp-date", &target, "2038-01-19T03:14:07"])
        .output()
        .expect("run chrono __cdp-date");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // The target was not Chromium-shaped (an installed browser, or the wrong exe): skip, do not fail - the
    // runner simply pointed at a target this mode cannot drive.
    if stdout.contains("not a chromium target") {
        eprintln!("'{target}' is not a Chromium/Electron app (runtime files not beside the exe) - skipping");
        return;
    }

    assert!(
        output.status.success(),
        "chrono __cdp-date failed (exit {:?}) - the target may block the remote-debugging port\nstdout: {stdout}\nstderr: {stderr}",
        output.status.code()
    );
    // The page's own new Date()/Date.now() must read the faked 2038 clock, proving the whole CDP chain took
    // effect (launch -> attach -> shim -> JS time API), not merely that the browser opened.
    assert!(stdout.contains("2038"), "the page did not read the faked 2038 clock\nstdout: {stdout}");
}

/// R2-W1 end to end: a session whose window is CLOSED while it runs must still report what it covered.
///
/// This is the shape the unit test on `cdp_verdict` cannot reach. That test pins the rule the function
/// applies - this one pins what the session hands it. Before the fix the session counted the contexts
/// still ATTACHED, and Chromium destroys its targets while shutting down - so a healthy run turned into
/// "DID NOT TAKE EFFECT (contexts: 0)" with exit code 11 and no coverage at all. Measured on a real
/// Electron application: a taskkill does NOT reproduce it (the socket dies before the destroy events
/// arrive), only a graceful window close does.
#[test]
#[ignore = "opt-in: set CHRONO_CDP_TARGET to a permissive Chromium/Electron exe; it launches and closes that app"]
fn a_closed_window_does_not_erase_what_the_session_covered() {
    let Ok(target) = std::env::var("CHRONO_CDP_TARGET") else {
        eprintln!("CHRONO_CDP_TARGET not set - skipping");
        return;
    };

    let child = Command::new(env!("CARGO_BIN_EXE_chrono"))
        .args(["run", "--at", "2038-01-01T00:00:00", "--mode", "x60", "--ticks", "30", &target])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("run chrono");

    // Let the app attach, shim and actually call a time API, then close its window the polite way.
    std::thread::sleep(std::time::Duration::from_secs(12));
    close_windows_of(&target);

    let out = child.wait_with_output().expect("collect chrono output");
    let stdout = String::from_utf8_lossy(&out.stdout);

    if stdout.contains("not a Chromium") || stdout.contains("launch_failed") {
        eprintln!("target is not Chromium-shaped or refused the debug port - skipping: {stdout}");
        return;
    }

    assert!(
        !stdout.contains("contexts: 0"),
        "the session covered contexts; a closing window must not erase them:
{stdout}"
    );
    assert_eq!(out.status.code(), Some(0), "a covered session must not exit as a failure:
{stdout}");
    // The browser it launched closed: that ends the session, and is not a handover to another program
    // (R4-S18). Its connection drops within milliseconds of the exit, inside the margin.
    assert!(!stdout.contains("followed:"), "a browser that closed handed nothing over:\n{stdout}");
    assert!(!stdout.contains("handing the application over"), "and the report does not say it did:\n{stdout}");
}

/// R4-S18 end to end: a target that starts the browser and exits - a launcher - must not end the
/// session. Before the fix the session ended with the launcher in about 0.4 s, with no verdict, and
/// took the browser with it.
///
/// The launcher is a script beside empty runtime files, so the folder reads as a Chromium target, and
/// it hands the session's own arguments (profile and debugging port) to the real target with `start`.
#[test]
#[ignore = "opt-in: set CHRONO_CDP_TARGET to a permissive Chromium/Electron exe; it launches that app through a script"]
fn a_launcher_that_hands_the_app_over_does_not_end_the_session() {
    let Ok(target) = std::env::var("CHRONO_CDP_TARGET") else {
        eprintln!("CHRONO_CDP_TARGET not set - skipping");
        return;
    };
    let stand = std::env::temp_dir().join(format!("chrono-s18-launcher-{}", std::process::id()));
    std::fs::create_dir_all(&stand).expect("stand folder");
    for runtime in ["icudtl.dat", "snapshot_blob.bin"] {
        std::fs::write(stand.join(runtime), b"").expect("runtime marker");
    }
    let launcher = stand.join("launcher.cmd");
    std::fs::write(&launcher, format!("@echo off\r\nstart \"\" \"{target}\" %*\r\n")).expect("launcher script");

    let out = Command::new(env!("CARGO_BIN_EXE_chrono"))
        .args(["run", &launcher.display().to_string(), "--at", "2038-01-19T03:14:07", "--json", "--ticks", "6"])
        .output()
        .expect("run chrono");
    let _ = std::fs::remove_dir_all(&stand);
    let stdout = String::from_utf8_lossy(&out.stdout);

    if stdout.contains("target.launch_failed") || stdout.contains("target.attach_failed") {
        eprintln!("the target refused the debug port - skipping: {stdout}");
        return;
    }
    let states = stdout.lines().filter(|l| l.contains("\"type\":\"state\"")).count();
    assert!(states >= 6, "the session ran to its ticks rather than ending with the launcher ({states} states):\n{stdout}");
    assert!(stdout.contains("\"chromium.followed_browser\""), "the report says why the session outlived its target:\n{stdout}");
    let image = std::path::Path::new(&target).file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let verdict = stdout.lines().find(|l| l.contains("\"type\":\"session_verdict\"")).unwrap_or_default();
    assert!(
        verdict.to_ascii_lowercase().contains(&format!("\"image\":\"{}\"", image.to_ascii_lowercase())),
        "the browser the session went on with is named in `followed`:\n{verdict}"
    );
    assert_eq!(out.status.code(), Some(0), "a covered session exits 0:\n{stdout}");
}

/// Ask every window of that executable to close, the way a user would - not a kill, which tears the
/// socket down before Chromium sends the target-destroyed events this test is about.
fn close_windows_of(target: &str) {
    let name = std::path::Path::new(target)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(target);
    let script = format!(
        "Get-Process -Name '{name}' -ErrorAction SilentlyContinue | ForEach-Object {{ $_.CloseMainWindow() | Out-Null }}"
    );
    let _ = Command::new("powershell.exe").args(["-NoProfile", "-Command", &script]).output();
}
