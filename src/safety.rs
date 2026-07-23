//! Pure timing and cancellation state used by the firmware safety layer.
//!
//! Deadline comparisons use wrapping arithmetic. Configured durations and the
//! interval between an observation and its deadline must remain well below
//! half of the `u64` range, as all firmware timing intervals normally do.

const U64_HALF_RANGE: u64 = 1 << 63;

pub const PARTIAL_FRAME_TIMEOUT_MS: u64 = 250;
pub const HEARTBEAT_INTERVAL_MS: u64 = 500;
pub const CONTROL_LEASE_MS: u64 = 2_000;

#[must_use]
fn deadline_reached(now_ms: u64, deadline_ms: u64) -> bool {
    now_ms.wrapping_sub(deadline_ms) < U64_HALF_RANGE
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeaseState {
    duration_ms: u64,
    deadline_ms: u64,
    armed: bool,
}

impl LeaseState {
    #[must_use]
    pub const fn new(duration_ms: u64) -> Self {
        Self {
            duration_ms,
            deadline_ms: 0,
            armed: false,
        }
    }

    pub fn refresh(&mut self, now_ms: u64) {
        self.deadline_ms = now_ms.wrapping_add(self.duration_ms);
        self.armed = true;
    }

    pub fn clear(&mut self) {
        self.deadline_ms = 0;
        self.armed = false;
    }

    #[must_use]
    pub const fn duration_ms(&self) -> u64 {
        self.duration_ms
    }

    #[must_use]
    pub const fn is_armed(&self) -> bool {
        self.armed
    }

    #[must_use]
    pub const fn deadline_ms(&self) -> Option<u64> {
        if self.armed {
            Some(self.deadline_ms)
        } else {
            None
        }
    }

    #[must_use]
    pub fn should_release(&self, now_ms: u64, guarded_work: bool) -> bool {
        guarded_work && self.armed && deadline_reached(now_ms, self.deadline_ms)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PartialFrameDeadline {
    timeout_ms: u64,
    deadline_ms: u64,
    active: bool,
}

impl PartialFrameDeadline {
    #[must_use]
    pub const fn new(timeout_ms: u64) -> Self {
        Self {
            timeout_ms,
            deadline_ms: 0,
            active: false,
        }
    }

    pub fn note_bytes(&mut self, now_ms: u64, unconsumed_len: usize) {
        if unconsumed_len == 0 {
            self.clear();
            return;
        }

        self.deadline_ms = now_ms.wrapping_add(self.timeout_ms);
        self.active = true;
    }

    pub fn clear(&mut self) {
        self.deadline_ms = 0;
        self.active = false;
    }

    #[must_use]
    pub const fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }

    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    #[must_use]
    pub const fn deadline_ms(&self) -> Option<u64> {
        if self.active {
            Some(self.deadline_ms)
        } else {
            None
        }
    }

    #[must_use]
    pub fn expired(&self, now_ms: u64) -> bool {
        self.active && deadline_reached(now_ms, self.deadline_ms)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CancellationGeneration {
    generation: u32,
}

impl CancellationGeneration {
    #[must_use]
    pub const fn new() -> Self {
        Self { generation: 1 }
    }

    #[must_use]
    pub const fn current(&self) -> u32 {
        self.generation
    }

    pub fn cancel(&mut self) -> u32 {
        let next = self.generation.wrapping_add(1);
        self.generation = if next == 0 { 1 } else { next };
        self.generation
    }

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
}
