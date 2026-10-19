//! What the random properties of this crate share: so far the canary of the weekly long run, and
//! nothing else - each property here draws from the one thing it searches (a command line, a registry
//! key, an environment block), and none of them has a piece another one needs.
//!
//! Compiled for tests only (`#[cfg(test)] mod search;` in lib.rs). The properties stand beside the
//! code they search, each in a `mod properties`, so `cargo test properties` selects every one of
//! them - the weekly run in .github/workflows/search.yml relies on that name. chrono-core, chrono-proto
//! and chrono-cli have a file of the same name and the same kind: a crate cannot reach another crate's
//! test code.

mod properties {
    use proptest::prelude::*;

    proptest! {
        /// The canary of the weekly run: red when CHRONO_SEARCH_CANARY is set, and a property that
        /// holds in every other run. Each part runs it first and stops if it does NOT see it fail,
        /// so a part that then finds nothing has shown it would have said so.
        #[test]
        fn the_canary_fails_only_when_asked_to(drawn in any::<u64>()) {
            let asked = std::env::var_os("CHRONO_SEARCH_CANARY").is_some();
            prop_assert!(!asked, "the canary failed, as it was asked to (drew {})", drawn);
        }
    }
}
