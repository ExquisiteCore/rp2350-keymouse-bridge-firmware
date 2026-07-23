//! Pure timing and cancellation state used by the firmware safety layer.
//!
//! Deadline comparisons use wrapping arithmetic. Configured durations and the
//! interval between an observation and its deadline must remain well below
//! half of the `u64` range, as all firmware timing intervals normally do.

const U64_HALF_RANGE: u64 = 1 << 63;

/// Maximum time a partial frame may remain without reported progress.
pub const PARTIAL_FRAME_TIMEOUT_MS: u64 = 250;
/// Recommended interval between controller heartbeat frames.
pub const HEARTBEAT_INTERVAL_MS: u64 = 500;
/// Time controller-owned guarded work may continue without a lease refresh.
pub const CONTROL_LEASE_MS: u64 = 2_000;

const fn assert_valid_duration(duration_ms: u64) {
    assert!(
        duration_ms > 0 && duration_ms < U64_HALF_RANGE,
        "timing duration must be in 1..2^63 milliseconds"
    );
}

#[must_use]
fn deadline_reached(now_ms: u64, deadline_ms: u64) -> bool {
    now_ms.wrapping_sub(deadline_ms) < U64_HALF_RANGE
}

/// Fixed-size state for a controller lease.
///
/// A new lease is unarmed. Once refreshed, it expires at the exact deadline,
/// but requests release only while guarded work exists.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeaseState {
    duration_ms: u64,
    deadline_ms: u64,
    armed: bool,
}

impl LeaseState {
    /// Creates an unarmed lease with a fixed duration.
    ///
    /// # Panics
    ///
    /// Panics unless `duration_ms` is in `1..2^63`, the domain required by
    /// wrapping deadline comparisons.
    #[must_use]
    pub const fn new(duration_ms: u64) -> Self {
        assert_valid_duration(duration_ms);
        Self {
            duration_ms,
            deadline_ms: 0,
            armed: false,
        }
    }

    /// Replaces the current deadline with `now_ms + duration_ms`.
    pub fn refresh(&mut self, now_ms: u64) {
        self.deadline_ms = now_ms.wrapping_add(self.duration_ms);
        self.armed = true;
    }

    /// Disarms the lease until the next refresh.
    pub fn clear(&mut self) {
        self.deadline_ms = 0;
        self.armed = false;
    }

    /// Returns the configured lease duration.
    #[must_use]
    pub const fn duration_ms(&self) -> u64 {
        self.duration_ms
    }

    /// Returns whether the lease has been refreshed and not cleared.
    #[must_use]
    pub const fn is_armed(&self) -> bool {
        self.armed
    }

    /// Returns the active wrapping deadline, or `None` while unarmed.
    #[must_use]
    pub const fn deadline_ms(&self) -> Option<u64> {
        if self.armed {
            Some(self.deadline_ms)
        } else {
            None
        }
    }

    /// Returns whether guarded work must be released at `now_ms`.
    ///
    /// The exact deadline is expired. An unarmed lease or a caller with no
    /// guarded work never requests release.
    #[must_use]
    pub fn should_release(&self, now_ms: u64, guarded_work: bool) -> bool {
        guarded_work && self.armed && deadline_reached(now_ms, self.deadline_ms)
    }
}

/// Fixed-size deadline state for an incomplete stream frame.
///
/// An active deadline expires at its exact deadline. Checking expiration does
/// not refresh it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PartialFrameDeadline {
    timeout_ms: u64,
    deadline_ms: u64,
    active: bool,
}

impl PartialFrameDeadline {
    /// Creates an inactive partial-frame deadline with a fixed timeout.
    ///
    /// # Panics
    ///
    /// Panics unless `timeout_ms` is in `1..2^63`, the domain required by
    /// wrapping deadline comparisons.
    #[must_use]
    pub const fn new(timeout_ms: u64) -> Self {
        assert_valid_duration(timeout_ms);
        Self {
            timeout_ms,
            deadline_ms: 0,
            active: false,
        }
    }

    /// Records explicit progress and refreshes the deadline.
    ///
    /// Call this only after bytes are appended or buffered data advances. An
    /// unchanged `unconsumed_len` can still be real progress when bytes were
    /// consumed and replaced, so this method intentionally trusts the caller.
    /// Polling must use [`Self::expired`] or the stream helper and must not call
    /// `note_bytes`. Passing zero reports an empty buffer and disarms the
    /// deadline.
    pub fn note_bytes(&mut self, now_ms: u64, unconsumed_len: usize) {
        if unconsumed_len == 0 {
            self.clear();
            return;
        }

        self.deadline_ms = now_ms.wrapping_add(self.timeout_ms);
        self.active = true;
    }

    /// Disarms the deadline until progress is reported again.
    pub fn clear(&mut self) {
        self.deadline_ms = 0;
        self.active = false;
    }

    /// Returns the configured partial-frame timeout.
    #[must_use]
    pub const fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }

    /// Returns whether the deadline is armed.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    /// Returns the active wrapping deadline, or `None` while inactive.
    #[must_use]
    pub const fn deadline_ms(&self) -> Option<u64> {
        if self.active {
            Some(self.deadline_ms)
        } else {
            None
        }
    }

    /// Checks expiration without changing or refreshing the deadline.
    ///
    /// The exact deadline is expired.
    #[must_use]
    pub fn expired(&self, now_ms: u64) -> bool {
        self.active && deadline_reached(now_ms, self.deadline_ms)
    }
}

/// Nonzero generation used to invalidate bounded in-flight operations.
///
/// Snapshots are intended for operations whose lifetime is bounded well below
/// a full `u32` generation cycle. After enough cancellations, an old snapshot
/// can exhibit ABA; wrapping skips zero and continues at one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CancellationGeneration {
    generation: u32,
}

impl CancellationGeneration {
    /// Creates a generation initialized to one.
    #[must_use]
    pub const fn new() -> Self {
        Self { generation: 1 }
    }

    /// Returns the current nonzero generation.
    #[must_use]
    pub const fn current(&self) -> u32 {
        self.generation
    }

    /// Advances once, skips zero on wrap, and returns the new generation.
    pub fn cancel(&mut self) -> u32 {
        let next = self.generation.wrapping_add(1);
        self.generation = if next == 0 { 1 } else { next };
        self.generation
    }

    /// Reports whether a bounded-operation snapshot is no longer current.
    #[must_use]
    pub const fn changed_since(&self, snapshot: u32) -> bool {
        self.generation != snapshot
    }
}

impl Default for CancellationGeneration {
    fn default() -> Self {
        Self::new()
    }
}

/// Sticky per-operation observation of a cancellation generation.
///
/// A queued value equal to the baseline is stale and does not cancel. Once a
/// changed generation is observed, later stale values cannot clear it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CancellationObservation {
    baseline: u32,
    cancelled: bool,
}

impl CancellationObservation {
    #[must_use]
    pub const fn new(baseline: u32) -> Self {
        Self {
            baseline,
            cancelled: false,
        }
    }

    #[must_use]
    pub const fn baseline(&self) -> u32 {
        self.baseline
    }

    #[must_use]
    pub const fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    /// Observes one published generation and returns the sticky state.
    pub fn observe(&mut self, generation: u32) -> bool {
        if generation != self.baseline {
            self.cancelled = true;
        }
        self.cancelled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safety_intervals_match_the_controller_contract() {
        assert_eq!(PARTIAL_FRAME_TIMEOUT_MS, 250);
        assert_eq!(HEARTBEAT_INTERVAL_MS, 500);
        assert_eq!(CONTROL_LEASE_MS, 2_000);
    }

    #[test]
    #[should_panic]
    fn lease_rejects_zero_duration() {
        let _ = LeaseState::new(0);
    }

    #[test]
    #[should_panic]
    fn lease_rejects_half_range_duration() {
        let _ = LeaseState::new(U64_HALF_RANGE);
    }

    #[test]
    fn lease_accepts_valid_duration_boundaries() {
        assert_eq!(LeaseState::new(1).duration_ms(), 1);
        assert_eq!(
            LeaseState::new(U64_HALF_RANGE - 1).duration_ms(),
            U64_HALF_RANGE - 1
        );
    }

    #[test]
    #[should_panic]
    fn partial_deadline_rejects_zero_timeout() {
        let _ = PartialFrameDeadline::new(0);
    }

    #[test]
    #[should_panic]
    fn partial_deadline_rejects_half_range_timeout() {
        let _ = PartialFrameDeadline::new(U64_HALF_RANGE);
    }

    #[test]
    fn partial_deadline_accepts_valid_timeout_boundaries() {
        assert_eq!(PartialFrameDeadline::new(1).timeout_ms(), 1);
        assert_eq!(
            PartialFrameDeadline::new(U64_HALF_RANGE - 1).timeout_ms(),
            U64_HALF_RANGE - 1
        );
    }

    #[test]
    fn lease_expires_only_when_guarded_work_exists() {
        let mut lease = LeaseState::new(2_000);
        lease.refresh(100);
        assert!(!lease.should_release(2_101, false));
        assert!(lease.should_release(2_101, true));
    }

    #[test]
    fn lease_expires_at_the_exact_boundary() {
        let mut lease = LeaseState::new(2_000);
        lease.refresh(100);

        assert!(!lease.should_release(2_099, true));
        assert!(lease.should_release(2_100, true));
    }

    #[test]
    fn lease_refresh_replaces_the_previous_deadline() {
        let mut lease = LeaseState::new(2_000);
        lease.refresh(100);
        lease.refresh(1_500);

        assert_eq!(lease.duration_ms(), 2_000);
        assert_eq!(lease.deadline_ms(), Some(3_500));
        assert!(!lease.should_release(2_100, true));
        assert!(!lease.should_release(3_499, true));
        assert!(lease.should_release(3_500, true));
    }

    #[test]
    fn lease_without_refresh_never_expires() {
        let lease = LeaseState::new(2_000);

        assert!(!lease.is_armed());
        assert_eq!(lease.deadline_ms(), None);
        assert!(!lease.should_release(u64::MAX, true));
    }

    #[test]
    fn clearing_a_lease_disarms_it() {
        let mut lease = LeaseState::new(2_000);
        lease.refresh(100);
        lease.clear();

        assert!(!lease.is_armed());
        assert_eq!(lease.deadline_ms(), None);
        assert!(!lease.should_release(9_999, true));
    }

    #[test]
    fn lease_deadline_comparison_handles_u64_wrap() {
        let mut lease = LeaseState::new(250);
        lease.refresh(u64::MAX - 100);

        assert_eq!(lease.deadline_ms(), Some(149));
        assert!(!lease.should_release(148, true));
        assert!(lease.should_release(149, true));
    }

    #[test]
    fn partial_frame_deadline_clears_stalled_data() {
        let mut deadline = PartialFrameDeadline::new(250);
        deadline.note_bytes(1_000, 4);
        assert!(!deadline.expired(1_249));
        assert!(deadline.expired(1_250));
        deadline.clear();
        assert!(!deadline.expired(9_999));
    }

    #[test]
    fn partial_frame_deadline_refreshes_only_on_progress() {
        let mut deadline = PartialFrameDeadline::new(250);
        deadline.note_bytes(1_000, 1);
        deadline.note_bytes(1_249, 4);

        assert_eq!(deadline.timeout_ms(), 250);
        assert_eq!(deadline.deadline_ms(), Some(1_499));
        assert!(!deadline.expired(1_250));
        assert!(!deadline.expired(1_498));
        assert!(deadline.expired(1_499));
        assert!(deadline.expired(1_500));
    }

    #[test]
    fn stalled_polling_does_not_refresh_unchanged_partial_data() {
        let mut deadline = PartialFrameDeadline::new(250);
        deadline.note_bytes(1_000, 4);

        assert!(!deadline.expired(1_249));
        assert_eq!(deadline.deadline_ms(), Some(1_250));
        assert!(deadline.expired(1_250));
        assert_eq!(deadline.deadline_ms(), Some(1_250));
    }

    #[test]
    fn empty_partial_buffer_disarms_deadline() {
        let mut deadline = PartialFrameDeadline::new(250);
        deadline.note_bytes(1_000, 4);
        deadline.note_bytes(1_100, 0);

        assert!(!deadline.is_active());
        assert_eq!(deadline.deadline_ms(), None);
        assert!(!deadline.expired(9_999));
    }

    #[test]
    fn explicit_partial_clear_disarms_deadline() {
        let mut deadline = PartialFrameDeadline::new(250);
        deadline.note_bytes(1_000, 4);
        deadline.clear();

        assert!(!deadline.is_active());
        assert!(!deadline.expired(9_999));
    }

    #[test]
    fn partial_deadline_comparison_handles_u64_wrap() {
        let mut deadline = PartialFrameDeadline::new(250);
        deadline.note_bytes(u64::MAX - 100, 1);

        assert_eq!(deadline.deadline_ms(), Some(149));
        assert!(!deadline.expired(148));
        assert!(deadline.expired(149));
    }

    #[test]
    fn cancellation_generation_changes_once_per_cancel() {
        let mut generation = CancellationGeneration::new();
        let before = generation.current();
        generation.cancel();
        assert_ne!(generation.current(), before);
    }

    #[test]
    fn successive_cancels_produce_successive_generations() {
        let mut generation = CancellationGeneration::new();

        assert_eq!(generation.current(), 1);
        assert_eq!(generation.cancel(), 2);
        assert_eq!(generation.cancel(), 3);
        assert_eq!(generation.current(), 3);
    }

    #[test]
    fn cancellation_generation_wrap_skips_zero() {
        let mut generation = CancellationGeneration {
            generation: u32::MAX,
        };

        assert_eq!(generation.cancel(), 1);
        assert_eq!(generation.current(), 1);
    }

    #[test]
    fn cancellation_snapshot_reports_changes() {
        let mut generation = CancellationGeneration::default();
        let snapshot = generation.current();

        assert!(!generation.changed_since(snapshot));
        generation.cancel();
        assert!(generation.changed_since(snapshot));
        assert_ne!(generation.current(), 0);
    }

    #[test]
    fn cancellation_observation_ignores_stale_baseline_values() {
        let mut observation = CancellationObservation::new(7);

        assert_eq!(observation.baseline(), 7);
        assert!(!observation.is_cancelled());
        assert!(!observation.observe(7));
        assert!(!observation.is_cancelled());
    }

    #[test]
    fn cancellation_observation_is_sticky_after_a_changed_generation() {
        let mut observation = CancellationObservation::new(7);

        assert!(observation.observe(8));
        assert!(observation.is_cancelled());
        assert!(observation.observe(7));
        assert!(observation.is_cancelled());
    }
}
