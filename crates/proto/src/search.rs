//! What the random properties of this crate share: numbers drawn from their edges as well as from
//! anywhere, text with the characters a line of the protocol has to carry safely, and the canary of
//! the weekly long run.
//!
//! Compiled for tests only (`#[cfg(test)] mod search;` in lib.rs). The properties stand beside the
//! code they search, each in a `mod properties`, so `cargo test properties` selects every one of
//! them - the weekly run in .github/workflows/search.yml relies on that name. chrono-core and
//! chrono-cli have a file of the same name and the same kind: a crate cannot reach another crate's
//! test code.

use proptest::collection::vec;
use proptest::prelude::*;

/// Any `u64`, with its edges drawn as often as the middle - a uniform draw almost never gives zero
/// or the largest value, and a count left off the wire when it is zero is decided on exactly one.
pub(crate) fn edge_u64() -> impl Strategy<Value = u64> {
    prop_oneof![
        4 => any::<u64>(),
        2 => 0u64..=3,
        1 => (u64::MAX - 3)..=u64::MAX,
    ]
}

/// Any `i64`, its edges drawn by name for the same reason as [`edge_u64`].
pub(crate) fn edge_i64() -> impl Strategy<Value = i64> {
    prop_oneof![
        4 => any::<i64>(),
        2 => -3i64..=3,
        1 => (i64::MAX - 3)..=i64::MAX,
        1 => i64::MIN..=(i64::MIN + 3),
    ]
}

/// Text a message may carry: nothing at all, plain words, and the characters a one-line framing has
/// to survive - a line break of each kind, the two separators a JavaScript reader breaks a line on,
/// a quote, a backslash, a NUL, a control character - among any others.
pub(crate) fn text() -> impl Strategy<Value = String> {
    let risky = prop_oneof![
        Just('\n'),
        Just('\r'),
        Just('\u{2028}'),
        Just('\u{2029}'),
        Just('"'),
        Just('\\'),
        Just('\0'),
        Just('\u{1b}'),
    ];
    let piece = prop_oneof![3 => any::<char>(), 1 => risky];
    prop_oneof![
        1 => Just(String::new()),
        2 => "[a-z._]{1,24}",
        3 => vec(piece, 0..16).prop_map(String::from_iter),
    ]
}

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
