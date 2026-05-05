//! Stable color assignment for spawned subagents.
//!
//! When the lead agent spawns subagents (via `AgentTool` or the
//! `LocalAgent` task executor), each one gets a distinct color so
//! the user can scan `/tasks` and tool output and tell agents apart
//! at a glance.
//!
//! # Determinism
//!
//! Assignment must be stable across a session: the same subagent id
//! always returns the same color, even if [`SubagentColorManager::assign`]
//! is called twice for the same id. The manager allocates colors in
//! declaration order — `Red`, `Blue`, `Green`, `Yellow`, `Purple`,
//! `Orange`, `Pink`, `Cyan` — and wraps after the eighth.
//!
//! # Theme bridge
//!
//! [`SubagentColor`] is a stable identifier. Mapping it to a real
//! terminal color is the renderer's job: it calls
//! `SubagentColor::theme_color` (defined in the CLI crate where the
//! `Theme` struct lives) with the active theme to get the crossterm
//! `Color` to paint with. Because the color is a slot reference (not
//! a literal RGB triple), themes can re-skin subagents without
//! touching this module.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// Stable color slot for a spawned subagent.
///
/// Maps onto the eight `subagent_*` slots that every theme defines.
/// The ordering (`Red`, `Blue`, `Green`, ...) is the assignment
/// order [`SubagentColorManager`] cycles through — adjusting it
/// changes which color the first / second / nth subagent gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentColor {
    Red,
    Blue,
    Green,
    Yellow,
    Purple,
    Orange,
    Pink,
    Cyan,
}

impl SubagentColor {
    /// Every variant in assignment order. The manager indexes into
    /// this list and wraps on overflow.
    pub const ALL: [Self; 8] = [
        Self::Red,
        Self::Blue,
        Self::Green,
        Self::Yellow,
        Self::Purple,
        Self::Orange,
        Self::Pink,
        Self::Cyan,
    ];

    /// Stable, lower-case label for display in `/tasks` and logs.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Red => "red",
            Self::Blue => "blue",
            Self::Green => "green",
            Self::Yellow => "yellow",
            Self::Purple => "purple",
            Self::Orange => "orange",
            Self::Pink => "pink",
            Self::Cyan => "cyan",
        }
    }
}

/// Assigns and remembers a color for each spawned subagent in the
/// current session.
///
/// Cheap to clone via `Arc`; safe to share across tasks. Internally
/// guarded by a single [`tokio::sync::RwLock`] over the assignment
/// map plus a counter — the lock is only held briefly during
/// assignment, never across an `await` of unrelated work.
///
/// # Concurrency
///
/// Two concurrent `assign()` calls for the same id will see one
/// winner: the second writer observes the existing entry and
/// returns it. Two concurrent `assign()` calls for *different* ids
/// each get the next slot in order; the counter under the same
/// write lock guarantees distinct results without racing on hashmap
/// iteration.
pub struct SubagentColorManager {
    inner: RwLock<Inner>,
}

struct Inner {
    /// Lookup by id. Insertion order is implicit in `next_index`.
    assignments: HashMap<String, SubagentColor>,
    /// Next slot to hand out, modulo `SubagentColor::ALL.len()`.
    next_index: usize,
}

impl SubagentColorManager {
    /// Build an empty manager with no assignments.
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Inner {
                assignments: HashMap::new(),
                next_index: 0,
            }),
        }
    }

    /// Convenience: an `Arc`-wrapped manager ready to share.
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::new())
    }

    /// Return the color for `subagent_id`, assigning one if this is
    /// the first time we have seen it.
    ///
    /// Idempotent: calling `assign(id)` twice returns the same color.
    pub async fn assign(&self, subagent_id: &str) -> SubagentColor {
        // Fast path: already assigned.
        {
            let guard = self.inner.read().await;
            if let Some(color) = guard.assignments.get(subagent_id) {
                return *color;
            }
        }

        // Slow path: take the write lock and assign. Re-check under
        // the write lock because another writer may have raced us
        // through the read-lock release.
        let mut guard = self.inner.write().await;
        if let Some(color) = guard.assignments.get(subagent_id) {
            return *color;
        }
        let idx = guard.next_index % SubagentColor::ALL.len();
        let color = SubagentColor::ALL[idx];
        guard.next_index = guard.next_index.wrapping_add(1);
        guard.assignments.insert(subagent_id.to_string(), color);
        color
    }

    /// Look up the color for `subagent_id` without assigning a new
    /// one. Returns `None` if the id has never been passed to
    /// [`Self::assign`].
    pub async fn for_id(&self, subagent_id: &str) -> Option<SubagentColor> {
        let guard = self.inner.read().await;
        guard.assignments.get(subagent_id).copied()
    }

    /// Number of distinct subagents currently tracked. Useful for
    /// tests and diagnostics.
    pub async fn len(&self) -> usize {
        self.inner.read().await.assignments.len()
    }

    /// Whether no subagents have been assigned yet.
    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.assignments.is_empty()
    }
}

impl Default for SubagentColorManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn assigns_eight_distinct_colors_in_order() {
        let mgr = SubagentColorManager::new();
        let mut got = Vec::new();
        for i in 0..8 {
            got.push(mgr.assign(&format!("agent-{i}")).await);
        }
        assert_eq!(
            got,
            vec![
                SubagentColor::Red,
                SubagentColor::Blue,
                SubagentColor::Green,
                SubagentColor::Yellow,
                SubagentColor::Purple,
                SubagentColor::Orange,
                SubagentColor::Pink,
                SubagentColor::Cyan,
            ]
        );
    }

    #[tokio::test]
    async fn ninth_subagent_wraps_to_red() {
        let mgr = SubagentColorManager::new();
        for i in 0..8 {
            mgr.assign(&format!("agent-{i}")).await;
        }
        let ninth = mgr.assign("agent-8").await;
        assert_eq!(ninth, SubagentColor::Red);
        let tenth = mgr.assign("agent-9").await;
        assert_eq!(tenth, SubagentColor::Blue);
    }

    #[tokio::test]
    async fn reassigning_same_id_returns_same_color() {
        let mgr = SubagentColorManager::new();
        let first = mgr.assign("alpha").await;
        // Push some other ids in between to advance the counter.
        let _ = mgr.assign("beta").await;
        let _ = mgr.assign("gamma").await;
        let again = mgr.assign("alpha").await;
        assert_eq!(first, again);
    }

    #[tokio::test]
    async fn for_id_returns_none_for_unknown_ids() {
        let mgr = SubagentColorManager::new();
        assert_eq!(mgr.for_id("nope").await, None);
        mgr.assign("known").await;
        assert!(mgr.for_id("known").await.is_some());
        assert_eq!(mgr.for_id("still-nope").await, None);
    }

    #[tokio::test]
    async fn len_and_is_empty_track_assignments() {
        let mgr = SubagentColorManager::new();
        assert!(mgr.is_empty().await);
        assert_eq!(mgr.len().await, 0);

        mgr.assign("a").await;
        mgr.assign("a").await; // idempotent
        mgr.assign("b").await;

        assert!(!mgr.is_empty().await);
        assert_eq!(mgr.len().await, 2);
    }

    #[test]
    fn manager_is_send_and_sync() {
        // Compile-time marker: if either bound were lost, this
        // wouldn't compile. The functions are unused at runtime.
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<SubagentColorManager>();
        assert_sync::<SubagentColorManager>();
    }

    #[test]
    fn color_as_str_round_trip_lowercase() {
        for c in SubagentColor::ALL {
            assert!(!c.as_str().is_empty());
            assert!(c.as_str().chars().all(|ch| ch.is_ascii_lowercase()));
        }
    }

    #[tokio::test]
    async fn concurrent_assignment_distributes_distinct_colors() {
        // Eight tasks racing on assign() with eight different ids
        // should still each get a unique color: the counter and
        // map mutation share one write lock.
        let mgr = Arc::new(SubagentColorManager::new());
        let mut handles = Vec::new();
        for i in 0..8 {
            let mgr = mgr.clone();
            handles.push(tokio::spawn(
                async move { mgr.assign(&format!("a-{i}")).await },
            ));
        }
        let mut seen = std::collections::HashSet::new();
        for h in handles {
            seen.insert(h.await.unwrap());
        }
        assert_eq!(seen.len(), 8, "all eight slots should be distinct");
    }
}
