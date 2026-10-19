//! The weekly search reaches every random property.
//!
//! A property stands beside the code it searches, in a `mod properties`, and on a pull request it
//! draws a few hundred cases from one fixed seed. The many cases that find something rare come from
//! .github/workflows/search.yml, one part a crate - so a crate given properties and no part is searched
//! by nobody, every Sunday, with every run green. That is the silence rule 6 forbids, and adding a part
//! by hand is exactly the step a session forgets: the third crate to get properties (chrono-proto,
//! 2026-10-19) needed a new part, a path for its pull requests and a canary of its own, and nothing
//! said so.
//!
//! Read as text, line by line, because the workflow is small and its matrix is written one key a line.
//! A layout this reader does not understand reads as no parts at all, which the floor below reddens.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// One part of the weekly matrix: the package it tests and the cargo target it selects.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Part {
    package: String,
    target: String,
}

/// The parts of the matrix, from the `- part:` entries up to the job's steps.
fn parts(workflow: &str) -> Vec<Part> {
    let mut found: Vec<Part> = Vec::new();
    for line in workflow.lines().map(str::trim) {
        if line.starts_with("steps:") {
            break;
        }
        if line.starts_with("- part:") {
            found.push(Part { package: String::new(), target: String::new() });
        } else if let (Some(part), Some(value)) = (found.last_mut(), line.strip_prefix("package:")) {
            part.package = value.trim().to_string();
        } else if let (Some(part), Some(value)) = (found.last_mut(), line.strip_prefix("target:")) {
            part.target = value.trim().to_string();
        }
    }
    found
}

/// Whether a Rust file holds random properties.
fn has_properties(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|text| text.lines().any(declares_properties))
}

/// Whether a line declares a `properties` module, in any form the compiler takes: behind
/// attributes on the same line, with any visibility, inline (`{`) or in a file of its own (`;`).
/// Only the bare inline form was read at first, and a crate that wrote `pub(crate) mod properties {`
/// or `mod properties;` would have been searched by nobody with this guard green.
fn declares_properties(line: &str) -> bool {
    let mut rest = line.trim();
    while let Some(after) = rest.strip_prefix("#[") {
        rest = after.split_once(']').map_or("", |(_, tail)| tail).trim_start();
    }
    if let Some(after) = rest.strip_prefix("pub") {
        rest = after.trim_start();
        if let Some(inner) = rest.strip_prefix('(') {
            rest = inner.split_once(')').map_or("", |(_, tail)| tail).trim_start();
        }
    }
    rest.strip_prefix("mod properties")
        .is_some_and(|tail| matches!(tail.trim_start().chars().next(), Some('{' | ';')))
}

/// The reader above, on the forms it has to take and the near misses it has to leave.
#[test]
fn every_form_of_a_properties_module_is_read_and_nothing_else() {
    for taken in [
        "mod properties {",
        "    mod properties {",
        "mod properties{",
        "mod properties;",
        "pub mod properties {",
        "pub(crate) mod properties {",
        "pub(super) mod properties;",
        "pub(in crate::cdp) mod properties {",
        "#[cfg(test)] mod properties;",
        "#[cfg(test)] pub(crate) mod properties {",
    ] {
        assert!(declares_properties(taken), "{taken:?} was not read as a properties module");
    }
    for left in [
        "/// mod properties {",
        "// mod properties {",
        "mod properties_of_the_core {",
        "mod tests {",
        "use crate::properties;",
        "let module = \"mod properties {\";",
        "",
    ] {
        assert!(!declares_properties(left), "{left:?} was read as a properties module");
    }
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for path in entries.filter_map(Result::ok).map(|e| e.path()) {
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// A crate of the workspace and where its properties stand.
struct Searched {
    dir: String,
    package: String,
    /// The target its `src/` properties run in, when it has some: `--lib` or `--bins`.
    in_src: Option<String>,
    /// The integration tests in `tests/` that hold properties, by file stem.
    in_tests: Vec<String>,
}

fn package_name(manifest: &Path) -> Option<String> {
    let doc: toml::Table = toml::from_str(&std::fs::read_to_string(manifest).ok()?).ok()?;
    Some(doc.get("package")?.get("name")?.as_str()?.to_string())
}

fn searched_crates() -> Vec<Searched> {
    let crates = repo_root().join("crates");
    let mut out = Vec::new();
    for dir in std::fs::read_dir(&crates).expect("the crates folder is readable").filter_map(Result::ok) {
        let root = dir.path();
        let Some(package) = package_name(&root.join("Cargo.toml")) else { continue };
        let mut sources = Vec::new();
        rust_files(&root.join("src"), &mut sources);
        let target = if root.join("src").join("lib.rs").is_file() { "--lib" } else { "--bins" };
        let in_src = sources.iter().any(|p| has_properties(p)).then(|| target.to_string());
        let mut tests = Vec::new();
        rust_files(&root.join("tests"), &mut tests);
        let in_tests = tests
            .iter()
            .filter(|p| has_properties(p))
            .filter_map(|p| Some(p.file_stem()?.to_str()?.to_string()))
            .collect();
        out.push(Searched { dir: dir.file_name().to_string_lossy().into_owned(), package, in_src, in_tests });
    }
    out
}

fn workflow() -> String {
    std::fs::read_to_string(repo_root().join(".github").join("workflows").join("search.yml"))
        .expect("the weekly search workflow is readable")
}

/// Every crate whose code holds properties has a part, a canary its part can run first, and a path
/// that makes a pull request touching its search run the search - and every part names properties
/// that exist, so a part left behind by a move cannot pass for one that searches.
#[test]
fn every_crate_with_random_properties_is_a_part_of_the_weekly_search() {
    let workflow = workflow();
    let listed: BTreeSet<Part> = parts(&workflow).into_iter().collect();
    let mut wanted = BTreeSet::new();
    let mut missing = Vec::new();
    for c in searched_crates() {
        if let Some(target) = &c.in_src {
            wanted.insert(Part { package: c.package.clone(), target: target.clone() });
            let search = repo_root().join("crates").join(&c.dir).join("src").join("search.rs");
            let canary = std::fs::read_to_string(&search).unwrap_or_default();
            if !canary.contains("fn the_canary_fails_only_when_asked_to(") {
                missing.push(format!("{}: no canary in crates/{}/src/search.rs", c.package, c.dir));
            }
            if !workflow.lines().any(|l| l.trim() == format!("- crates/{}/src/search.rs", c.dir)) {
                missing.push(format!("{}: crates/{}/src/search.rs is not among the pull request paths", c.package, c.dir));
            }
        }
        for stem in &c.in_tests {
            wanted.insert(Part { package: c.package.clone(), target: format!("--test {stem}") });
        }
    }
    let unlisted: Vec<&Part> = wanted.difference(&listed).collect();
    let stale: Vec<&Part> = listed.difference(&wanted).collect();
    assert!(
        wanted.len() >= 6,
        "found {} places with random properties, where there were six on 2026-10-20 - the scan reads \
         the wrong place, or properties were removed and this floor has to come down with them",
        wanted.len()
    );
    assert!(unlisted.is_empty(), "properties no weekly part searches - add a part to search.yml: {unlisted:?}");
    assert!(stale.is_empty(), "weekly parts that name no properties - remove or correct them: {stale:?}");
    assert!(missing.is_empty(), "{missing:#?}");
}
