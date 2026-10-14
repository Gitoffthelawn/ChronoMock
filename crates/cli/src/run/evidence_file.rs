//! Where the evidence file of `--report` goes, and how it gets there.
//!
//! The path is judged before the session starts, and the file is written whole or not at all. Both
//! halves exist because of what `--report` used to do with a path (R5-B9, item 1): any path was taken,
//! including the target's own executable (`--report app.exe` overwrote the program it was about), a
//! folder that did not exist was found out only after the whole session had run, and the write went
//! straight into the file, so a failure halfway left a cut-off evidence file that still looked like one.

use std::path::{Path, PathBuf};

/// Why `path` cannot take the evidence file of a run of `target`, or `None` when it can.
///
/// Asked before anything starts, a dry run included - a dry run's 0 says the plan is sound (docs/08
/// section 8), and a run that cannot keep its evidence is not. An existing file is fine to replace, that
/// is what a second run with the same `--report` does, unless it is a program: the target itself, or
/// any Windows executable image, told by its first two bytes rather than by its extension, so a renamed
/// one is caught too.
pub(crate) fn path_problem(path: &str, target: &str) -> Option<String> {
    let file = Path::new(path);
    if file.is_dir() {
        return Some(format!("--report {path} is a folder - name a file inside it, like {}", file.join("evidence.txt").display()));
    }
    let folder = file.parent().filter(|f| !f.as_os_str().is_empty()).unwrap_or(Path::new("."));
    if !folder.is_dir() {
        return Some(format!("--report {path}: the folder {} does not exist", folder.display()));
    }
    if same_file(file, Path::new(target)) {
        return Some(format!("--report {path} is the target itself - the evidence would overwrite the program it is about"));
    }
    if is_program_image(file) {
        return Some(format!(
            "--report {path} is a program file - the evidence would overwrite it, so name a text file instead"
        ));
    }
    None
}

/// Whether two paths name one existing file. Both are resolved, so `.\app.exe` and the full path of the
/// same file match. A path that does not exist yet is never the target.
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Whether the file starts with `MZ`, the mark every Windows executable image (exe, dll, sys) opens
/// with. A file that cannot be opened is not judged here - the write reports what is wrong with it.
fn is_program_image(file: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 2];
    std::fs::File::open(file).and_then(|mut f| f.read_exact(&mut head)).is_ok() && head == *b"MZ"
}

/// Write the evidence into `path` whole or not at all: into a file of its own beside it first, then
/// renamed over it. A rename within one folder replaces the old file in one step, so a reader never
/// sees half of the new text.
///
/// When the rename fails (the old file is open in an editor that holds it), the written copy stays and
/// the message names it - the session cannot be run again, so its evidence must not be thrown away.
pub(crate) fn write(path: &str, text: &str) -> Result<(), String> {
    use std::io::Write;
    let file = Path::new(path);
    let (scratch, mut out) = create_scratch(file).map_err(|e| format!("cannot write evidence to {path}: {e}"))?;
    let written = out.write_all(text.as_bytes());
    drop(out);
    if let Err(e) = written {
        let _ = std::fs::remove_file(&scratch);
        return Err(format!("cannot write evidence to {path}: {e}"));
    }
    std::fs::rename(&scratch, file)
        .map_err(|e| format!("cannot replace {path} ({e}) - the evidence is in {}", scratch.display()))
}

/// How many names beside the report the scratch file tries before it gives up.
const SCRATCH_NAMES: u32 = 100;

/// The scratch file, created new: never a file that is already there. A process id comes back, so the
/// first name can belong to an earlier run whose rename failed and whose evidence it still holds, and a
/// name that is a link would have the write land in whatever it points at. Either way the next name is
/// tried (review round on PR #104).
fn create_scratch(file: &Path) -> std::io::Result<(PathBuf, std::fs::File)> {
    for attempt in 0..SCRATCH_NAMES {
        let scratch = scratch_path(file, attempt);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&scratch) {
            Ok(out) => return Ok((scratch, out)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other(format!(
        "the {SCRATCH_NAMES} scratch names beside it, from {} on, are all taken",
        scratch_path(file, 0).display()
    )))
}

/// The file the evidence is written into before it takes `file`'s name: the same folder, so the rename
/// stays on one volume, and this process's id in the name, so two runs writing one report do not share
/// it. A later attempt adds its number.
fn scratch_path(file: &Path, attempt: u32) -> PathBuf {
    let name = file.file_name().map_or_else(|| "evidence".into(), |n| n.to_string_lossy().into_owned());
    let pid = std::process::id();
    match attempt {
        0 => file.with_file_name(format!("{name}.{pid}.tmp")),
        n => file.with_file_name(format!("{name}.{pid}-{n}.tmp")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(prefix: &str) -> PathBuf {
        let dir = crate::testutil::unique_temp_dir(prefix);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A path the evidence may take passes: a new file, and an old evidence file a second run replaces.
    #[test]
    fn a_new_file_and_an_old_text_file_are_fine() {
        let dir = folder("ev-ok");
        let new = dir.join("evidence.txt");
        assert_eq!(path_problem(new.to_str().unwrap(), "app.exe"), None);
        std::fs::write(&new, "an earlier run").unwrap();
        assert_eq!(path_problem(new.to_str().unwrap(), "app.exe"), None, "a second run replaces its own file");
    }

    /// Each path the evidence must not take is refused before the session, by name (R5-B9, item 1).
    #[test]
    fn a_folder_a_missing_folder_the_target_and_a_program_are_refused() {
        let dir = folder("ev-refuse");
        let err = path_problem(dir.to_str().unwrap(), "app.exe").expect("a folder");
        assert!(err.contains("is a folder"), "{err}");

        let missing = dir.join("no-such-folder").join("evidence.txt");
        let err = path_problem(missing.to_str().unwrap(), "app.exe").expect("a missing folder");
        assert!(err.contains("does not exist"), "{err}");

        let target = dir.join("app.exe");
        std::fs::write(&target, b"MZ\x90\x00").unwrap();
        let err = path_problem(target.to_str().unwrap(), target.to_str().unwrap()).expect("the target");
        assert!(err.contains("is the target itself"), "{err}");

        // A program under a name that does not say so is still a program.
        let renamed = dir.join("notes.txt");
        std::fs::write(&renamed, b"MZ\x90\x00").unwrap();
        let err = path_problem(renamed.to_str().unwrap(), "other.exe").expect("a program image");
        assert!(err.contains("is a program file"), "{err}");
    }

    /// The write replaces the old file and leaves nothing beside it.
    #[test]
    fn the_write_replaces_the_old_file_and_leaves_no_scratch_file() {
        let dir = folder("ev-write");
        let file = dir.join("evidence.txt");
        std::fs::write(&file, "old").unwrap();
        write(file.to_str().unwrap(), "new").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "new");
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left, vec![std::ffi::OsString::from("evidence.txt")], "the scratch file is gone");
    }

    /// When the old file cannot be replaced - here another program holds it open with no sharing, the
    /// way an editor can - the evidence is not lost: it stays in the scratch file, and the message names
    /// that file. A plain write into the target would have failed and kept nothing.
    #[test]
    fn evidence_that_cannot_replace_the_old_file_stays_in_the_scratch_file() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = folder("ev-held");
        let file = dir.join("evidence.txt");
        std::fs::write(&file, "old").unwrap();
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(&file).unwrap();
        let err = write(file.to_str().unwrap(), "new").unwrap_err();
        drop(held);
        let scratch = scratch_path(&file, 0);
        assert!(err.contains("cannot replace") && err.contains(&scratch.display().to_string()), "{err}");
        assert_eq!(std::fs::read_to_string(&scratch).unwrap(), "new", "the evidence is in the scratch file");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "old", "the held file is untouched");
    }

    /// A scratch file already standing under this process's name - left by an earlier run whose
    /// rename failed, under a process id that came back - keeps the evidence it holds. The write takes
    /// the next name (review round on PR #104).
    #[test]
    fn an_earlier_scratch_file_under_the_same_name_is_never_overwritten() {
        let dir = folder("ev-earlier");
        let file = dir.join("evidence.txt");
        let earlier = scratch_path(&file, 0);
        std::fs::write(&earlier, "an earlier session").unwrap();
        write(file.to_str().unwrap(), "new").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(&earlier).unwrap(), "an earlier session", "the earlier evidence stays");
        let mut left: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).collect();
        left.sort();
        let mut expected = vec![file.clone(), earlier];
        expected.sort();
        assert_eq!(left, expected, "nothing else is left beside them");
    }
}
