//! The processes of an elevated WebView2 host's engine, found by the process tree (docs/09 section 12.19).
//!
//! An elevated host does not start its engine: a system service does, and the host is only the parent
//! the new process DECLARES. So the hook in the host never sees the engine start - its processes are
//! in neither the registry of hooked pids nor the ring of children the hook watched being spawned -
//! and two things go missing with them. The debugging port the engine opens belongs to a pid outside
//! the family, so discovery never looks at it, and the processes ran on the real clock without anyone
//! saying so, which the same application under an ordinary token is reported for (`partial`).
//!
//! Walking the tree under the host once a second puts both right: its pids join the family discovery
//! looks in, and each process nobody else named is named the way an unfollowed child is.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use chrono_mech::UncoveredChild;

use crate::output::diag;

/// How often the tree is read. A snapshot of a few hundred processes costs a millisecond or two, and an
/// engine that starts is found a second later at most, well inside the time its port takes to answer.
const EVERY: Duration = Duration::from_secs(1);

/// How many processes one session names. An engine that restarts its helpers for hours would otherwise
/// grow the list without end. Past it a process is counted and not named, so the total stays true.
pub(crate) const NAMED_MAX: usize = 256;

/// What one look at the tree found.
pub(crate) struct Found {
    /// Every process under the host now, known or not: the family a debugging port may belong to.
    pub(crate) pids: HashSet<u32>,
    /// The ones nobody had named, named now.
    pub(crate) named: Vec<UncoveredChild>,
}

/// What the session has seen of the tree so far.
#[derive(Default)]
pub(crate) struct UnhookedTree {
    /// The pids present at the last look that were already dealt with. A pid that has left the tree is
    /// forgotten, because the system may hand it to another process, which is a new one.
    seen: HashSet<u32>,
    named: usize,
    unnamed: u32,
    last: Option<Instant>,
    said_unreadable: bool,
}

impl UnhookedTree {
    /// Look at the tree under `host`, unless one was taken less than a second ago and `forced` does not
    /// say otherwise. `known` is every pid the session already accounts for (the hooked ones and the
    /// children the hook named), so a process is named once however it became known. `tree` and `name`
    /// are the two questions put to the system, so the walk is tested with answers of the test's making.
    ///
    /// `None` is "nothing to add now": not due, or the process list could not be read - which is said
    /// once on stderr, and leaves the session where it was (no engine followed, and the verdict does not
    /// claim one was).
    pub(crate) fn follow_with(
        &mut self,
        host: u32,
        known: &[u32],
        forced: bool,
        now: Instant,
        tree: impl Fn(u32) -> Result<Vec<(u32, u32)>, String>,
        name: impl Fn(u32, u32) -> UncoveredChild,
    ) -> Option<Found> {
        if !forced && self.last.is_some_and(|t| now.saturating_duration_since(t) < EVERY) {
            return None;
        }
        self.last = Some(now);
        let under = match tree(host) {
            Ok(under) => under,
            Err(why) => {
                if !self.said_unreadable {
                    self.said_unreadable = true;
                    diag!("chrono core: the process list could not be read, so the engine of an elevated application is not followed: {why}");
                }
                return None;
            }
        };
        let present: HashSet<u32> = under.iter().map(|&(pid, _)| pid).collect();
        self.seen.retain(|pid| present.contains(pid));
        let known: HashSet<u32> = known.iter().copied().collect();
        let mut named = Vec::new();
        for &(pid, parent) in &under {
            if known.contains(&pid) || !self.seen.insert(pid) {
                continue;
            }
            if self.named < NAMED_MAX {
                self.named += 1;
                named.push(name(pid, parent));
            } else {
                self.unnamed += 1;
            }
        }
        Some(Found { pids: present, named })
    }

    /// How many processes the tree has turned up that nobody else named, named or not.
    pub(crate) fn total(&self) -> u32 {
        self.named as u32 + self.unnamed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn child(pid: u32, parent: u32) -> UncoveredChild {
        UncoveredChild { pid, parent_pid: parent, image: Some(format!("p{pid}.exe")), command_line: None }
    }

    fn pids(found: &Found) -> Vec<u32> {
        let mut pids: Vec<u32> = found.named.iter().map(|c| c.pid).collect();
        pids.sort_unstable();
        pids
    }

    /// The tree under a host: what is named is what nobody else knew, each once, however many looks.
    #[test]
    fn a_process_is_named_once_and_only_when_nobody_else_knew_it() {
        let mut tree = UnhookedTree::default();
        let t0 = Instant::now();
        let under = |_: u32| Ok(vec![(10, 1), (11, 10), (12, 10), (13, 12)]);

        let first = tree.follow_with(1, &[1, 12], false, t0, under, child).expect("the first look is due");
        assert_eq!(pids(&first), [10, 11, 13], "12 was known already: it is in the family and is not named");
        assert_eq!(first.pids.len(), 4, "the family handed to discovery is the whole tree");

        let again = tree.follow_with(1, &[1, 12], true, t0, under, child).expect("a forced look is due");
        assert!(again.named.is_empty(), "nothing new, nothing named twice");
        assert_eq!(tree.total(), 3);
    }

    /// One look a second, and a forced one ignores the cadence. A look that is not due touches nothing.
    #[test]
    fn the_tree_is_read_once_a_second_unless_forced() {
        let reads = RefCell::new(0u32);
        let under = |_: u32| {
            *reads.borrow_mut() += 1;
            Ok(vec![(10, 1)])
        };
        let mut tree = UnhookedTree::default();
        let t0 = Instant::now();
        assert!(tree.follow_with(1, &[1], false, t0, under, child).is_some());
        assert!(tree.follow_with(1, &[1], false, t0 + EVERY / 2, under, child).is_none());
        assert_eq!(*reads.borrow(), 1, "inside the cadence the system is not asked");
        assert!(tree.follow_with(1, &[1], false, t0 + EVERY, under, child).is_some());
        assert!(tree.follow_with(1, &[1], true, t0 + EVERY, under, child).is_some());
        assert_eq!(*reads.borrow(), 3);
    }

    /// A pid that left the tree and came back is a new process: the system reuses pids, and the second
    /// one must be named like any other.
    #[test]
    fn a_pid_that_left_and_came_back_is_a_new_process() {
        let mut tree = UnhookedTree::default();
        let t0 = Instant::now();
        assert_eq!(pids(&tree.follow_with(1, &[1], true, t0, |_| Ok(vec![(10, 1)]), child).unwrap()), [10]);
        assert!(tree.follow_with(1, &[1], true, t0, |_| Ok(vec![]), child).unwrap().named.is_empty());
        assert_eq!(pids(&tree.follow_with(1, &[1], true, t0, |_| Ok(vec![(10, 1)]), child).unwrap()), [10]);
        assert_eq!(tree.total(), 2);
    }

    /// Past the cap a process is counted and not named, so the list stays bounded and the total true.
    #[test]
    fn past_the_cap_processes_are_counted_and_not_named() {
        let mut tree = UnhookedTree::default();
        let many: Vec<(u32, u32)> = (100..100 + NAMED_MAX as u32 + 5).map(|pid| (pid, 1)).collect();
        let found = tree.follow_with(1, &[1], true, Instant::now(), |_| Ok(many.clone()), child).unwrap();
        assert_eq!(found.named.len(), NAMED_MAX);
        assert_eq!(found.pids.len(), many.len(), "discovery still looks at all of them");
        assert_eq!(tree.total(), NAMED_MAX as u32 + 5);
    }

    /// A process list that cannot be read adds nothing and is said once. The cadence is spent all the
    /// same, so a snapshot that keeps failing is asked about once a second and not every turn of the loop.
    #[test]
    fn an_unreadable_process_list_adds_nothing() {
        let mut tree = UnhookedTree::default();
        let t0 = Instant::now();
        assert!(tree.follow_with(1, &[1], false, t0, |_| Err("snapshot failed".into()), child).is_none());
        assert!(tree.follow_with(1, &[1], true, t0, |_| Err("snapshot failed".into()), child).is_none());
        assert_eq!(tree.total(), 0);
    }
}
