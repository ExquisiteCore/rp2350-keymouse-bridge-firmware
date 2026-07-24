//! Pure request and safety transitions shared by the Embassy runtime and host tests.

use crate::coordinator::{Admission, Coordinator, OwnedRequest, SafetyEvent};
use crate::error::ErrorCode;
use crate::protocol::PROTOCOL_VERSION;
use crate::safety::LeaseState;

/// Timestamped refresh emitted by the dispatcher after one request admission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeaseRefresh {
    session_generation: u32,
    observed_at_ms: u64,
}

impl LeaseRefresh {
    const fn new(session_generation: u32, observed_at_ms: u64) -> Self {
        Self {
            session_generation,
            observed_at_ms,
        }
    }

    #[cfg(test)]
    const fn session_generation(self) -> u32 {
        self.session_generation
    }
}

/// Keeps a newer refresh that arrived after a selected signal was taken.
pub const fn latest_lease_refresh(
    selected: LeaseRefresh,
    queued: Option<LeaseRefresh>,
) -> LeaseRefresh {
    match queued {
        Some(refresh) => refresh,
        None => selected,
    }
}

/// Admission plus the runtime side effects derived from that exact decision.
pub struct RuntimeAdmission {
    admission: Admission,
    lease_refresh: Option<LeaseRefresh>,
}

impl RuntimeAdmission {
    /// Borrows the coordinator admission used by the dispatcher.
    #[cfg(test)]
    pub const fn admission(&self) -> &Admission {
        &self.admission
    }

    /// Returns the refresh event, if this accepted request refreshes the v2 lease.
    pub const fn lease_refresh(&self) -> Option<LeaseRefresh> {
        self.lease_refresh
    }

    /// Reports whether this admission can produce CDC response traffic.
    #[cfg(test)]
    pub fn response_expected(&self) -> bool {
        !matches!(self.admission, Admission::NoResponse(_))
    }

    /// Consumes the wrapper so the dispatcher can execute the original admission.
    pub fn into_admission(self) -> Admission {
        self.admission
    }
}

/// Applies the production dispatcher admission and derives its lease event once.
pub fn admit_runtime_request(
    coordinator: &mut Coordinator,
    request: OwnedRequest,
    external_busy: bool,
    runtime_session: u32,
    now_ms: u64,
) -> RuntimeAdmission {
    let version = request.version();
    let admission = coordinator.admit_prepared_with_external_busy(request, external_busy);
    let lease_refresh = refreshes_v2_lease(version, &admission)
        .then_some(LeaseRefresh::new(runtime_session, now_ms));
    RuntimeAdmission {
        admission,
        lease_refresh,
    }
}

fn refreshes_v2_lease(version: u8, admission: &Admission) -> bool {
    version == PROTOCOL_VERSION
        && !matches!(
            admission,
            Admission::Reject(
                ErrorCode::BadCommand
                    | ErrorCode::FrameTooLong
                    | ErrorCode::UnsupportedVersion
                    | ErrorCode::UnsupportedFlags
                    | ErrorCode::InvalidSequence
                    | ErrorCode::WaitTooLong
            )
        )
}

/// Required side effects for a safety event entering the firmware runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeSafetyStep {
    /// Invalidate queued and in-flight work before any release is scheduled.
    ResetSession,
    /// Schedule the emergency executor that releases both HID interfaces.
    ReleaseInputs,
}

/// Ordered fail-safe plan applied by the runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeSafetyAction {
    steps: [RuntimeSafetyStep; 2],
}

impl RuntimeSafetyAction {
    /// Returns the fail-safe steps in their required application order.
    pub const fn steps(self) -> [RuntimeSafetyStep; 2] {
        self.steps
    }
}

/// Converts DTR, USB, and lease failures into the common fail-safe transition.
pub const fn runtime_safety_action(_event: SafetyEvent) -> RuntimeSafetyAction {
    RuntimeSafetyAction {
        steps: [
            RuntimeSafetyStep::ResetSession,
            RuntimeSafetyStep::ReleaseInputs,
        ],
    }
}

/// Pure controller-lease state owned by the Embassy lease task.
pub struct RuntimeLease {
    lease: LeaseState,
    armed_session: Option<u32>,
}

impl RuntimeLease {
    /// Creates an unarmed runtime lease.
    pub const fn new(duration_ms: u64) -> Self {
        Self {
            lease: LeaseState::new(duration_ms),
            armed_session: None,
        }
    }

    /// Returns the active deadline for diagnostics and deterministic host tests.
    #[cfg(test)]
    pub const fn deadline_ms(&self) -> Option<u64> {
        self.lease.deadline_ms()
    }

    /// Reports whether a refresh remains armed.
    pub const fn is_armed(&self) -> bool {
        self.lease.is_armed()
    }

    /// Returns milliseconds until the current deadline, clamped to zero once reached.
    pub fn remaining_ms(&self, now_ms: u64) -> Option<u64> {
        let deadline_ms = self.lease.deadline_ms()?;
        if self.lease.should_release(now_ms, true) {
            Some(0)
        } else {
            Some(deadline_ms.wrapping_sub(now_ms))
        }
    }

    /// Arbitrates one wake against the current session, pending refresh, and old deadline.
    ///
    /// A refresh observed before the old deadline wins even if this transition runs late. The
    /// exact old deadline is expired. Refreshes from another session are ignored, and an armed
    /// lease from another session is disarmed without releasing current-session work.
    pub fn transition(
        &mut self,
        current_session: u32,
        now_ms: u64,
        guarded_work: bool,
        pending_refresh: Option<LeaseRefresh>,
    ) -> Option<RuntimeSafetyAction> {
        if matches!(self.armed_session, Some(session) if session != current_session) {
            self.clear();
        }

        if let Some(refresh) = pending_refresh
            && refresh.session_generation == current_session
        {
            if self.armed_session == Some(current_session)
                && self.lease.should_release(refresh.observed_at_ms, true)
            {
                return self.expire(guarded_work);
            }

            self.lease.refresh(refresh.observed_at_ms);
            self.armed_session = Some(current_session);
        }

        if self.lease.should_release(now_ms, true) {
            return self.expire(guarded_work);
        }

        None
    }

    fn clear(&mut self) {
        self.lease.clear();
        self.armed_session = None;
    }

    fn expire(&mut self, guarded_work: bool) -> Option<RuntimeSafetyAction> {
        self.clear();
        guarded_work.then_some(runtime_safety_action(SafetyEvent::LeaseExpired))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LeaseRefresh, RuntimeLease, RuntimeSafetyStep, admit_runtime_request, latest_lease_refresh,
        runtime_safety_action,
    };
    use crate::coordinator::{Admission, Coordinator, OwnedRequest, SafetyEvent};
    use crate::protocol::{
        CommandType, FLAG_NO_RESPONSE, Frame, LEGACY_PROTOCOL_VERSION, PROTOCOL_VERSION,
    };
    use crate::safety::CONTROL_LEASE_MS;

    fn request_for_version(
        version: u8,
        sequence: u16,
        flags: u8,
        command_type: CommandType,
        payload: &[u8],
    ) -> OwnedRequest {
        OwnedRequest::from_frame(&Frame {
            version,
            flags,
            sequence,
            command_type,
            payload,
        })
        .unwrap()
    }

    fn request(
        sequence: u16,
        flags: u8,
        command_type: CommandType,
        payload: &[u8],
    ) -> OwnedRequest {
        request_for_version(PROTOCOL_VERSION, sequence, flags, command_type, payload)
    }

    #[test]
    fn runtime_request_entry_refreshes_no_response_heartbeat_and_expires_exactly() {
        let mut coordinator = Coordinator::new();
        let mut lease = RuntimeLease::new(CONTROL_LEASE_MS);

        let initial = admit_runtime_request(
            &mut coordinator,
            request(47, 0, CommandType::KeyDown, &[0, 0x1a]),
            false,
            1,
            100,
        );
        assert!(initial.response_expected());
        assert_eq!(initial.lease_refresh().unwrap().session_generation(), 1);
        assert_eq!(
            lease.transition(1, 100, true, initial.lease_refresh()),
            None
        );
        assert_eq!(lease.deadline_ms(), Some(2_100));

        let heartbeat = admit_runtime_request(
            &mut coordinator,
            request(0, FLAG_NO_RESPONSE, CommandType::Heartbeat, &[]),
            false,
            1,
            1_500,
        );
        assert!(matches!(heartbeat.admission(), Admission::NoResponse(_)));
        assert!(!heartbeat.response_expected());
        assert!(!coordinator.has_cached_sequence(0));
        assert_eq!(
            lease.transition(1, 1_500, true, heartbeat.lease_refresh()),
            None
        );
        assert_eq!(lease.deadline_ms(), Some(3_500));

        assert_eq!(lease.transition(1, 2_100, true, None), None);
        assert_eq!(lease.transition(1, 3_499, true, None), None);
        assert_eq!(
            lease.transition(1, 3_500, true, None),
            Some(runtime_safety_action(SafetyEvent::LeaseExpired))
        );
    }

    #[test]
    fn old_session_lease_cannot_release_guarded_legacy_work_in_a_new_session() {
        let mut coordinator = Coordinator::new();
        let mut lease = RuntimeLease::new(CONTROL_LEASE_MS);
        let s1 = 7;
        let s2 = 8;

        let s1_request = admit_runtime_request(
            &mut coordinator,
            request(47, 0, CommandType::KeyDown, &[0, 0x1a]),
            false,
            s1,
            100,
        );
        assert_eq!(
            lease.transition(s1, 100, true, s1_request.lease_refresh()),
            None
        );
        assert_eq!(lease.deadline_ms(), Some(2_100));

        coordinator.clear_session();
        let legacy = admit_runtime_request(
            &mut coordinator,
            request_for_version(
                LEGACY_PROTOCOL_VERSION,
                48,
                0,
                CommandType::KeyDown,
                &[0, 0x1b],
            ),
            false,
            s2,
            500,
        );
        assert_eq!(legacy.lease_refresh(), None);

        assert_eq!(lease.transition(s2, 2_100, true, None), None);
        assert!(!lease.is_armed());
    }

    #[test]
    fn stale_refresh_consumed_only_after_reset_is_ignored() {
        let mut lease = RuntimeLease::new(CONTROL_LEASE_MS);

        assert_eq!(
            lease.transition(2, 200, true, Some(LeaseRefresh::new(1, 100))),
            None
        );
        assert!(!lease.is_armed());
        assert_eq!(lease.deadline_ms(), None);
    }

    #[test]
    fn new_session_refresh_rearms_and_expires_at_the_exact_deadline() {
        let mut lease = RuntimeLease::new(CONTROL_LEASE_MS);

        assert_eq!(
            lease.transition(2, 100, true, Some(LeaseRefresh::new(2, 100))),
            None
        );
        assert_eq!(lease.deadline_ms(), Some(2_100));
        assert_eq!(lease.transition(2, 2_099, true, None), None);
        assert_eq!(
            lease.transition(2, 2_100, true, None),
            Some(runtime_safety_action(SafetyEvent::LeaseExpired))
        );
        assert!(!lease.is_armed());
    }

    #[test]
    fn queued_refresh_observed_before_deadline_wins_over_late_timer_wake() {
        let mut lease = RuntimeLease::new(CONTROL_LEASE_MS);
        assert_eq!(
            lease.transition(1, 100, true, Some(LeaseRefresh::new(1, 100))),
            None
        );

        assert_eq!(
            lease.transition(1, 2_101, true, Some(LeaseRefresh::new(1, 2_099))),
            None
        );
        assert_eq!(lease.deadline_ms(), Some(4_099));
        assert!(lease.is_armed());
    }

    #[test]
    fn refresh_observed_at_exact_deadline_does_not_rescue_guarded_work() {
        let mut lease = RuntimeLease::new(CONTROL_LEASE_MS);
        assert_eq!(
            lease.transition(1, 100, true, Some(LeaseRefresh::new(1, 100))),
            None
        );

        assert_eq!(
            lease.transition(1, 2_101, true, Some(LeaseRefresh::new(1, 2_100))),
            Some(runtime_safety_action(SafetyEvent::LeaseExpired))
        );
        assert!(!lease.is_armed());
    }

    #[test]
    fn queued_signal_refresh_replaces_the_already_selected_value() {
        let selected = LeaseRefresh::new(1, 2_098);
        let queued = LeaseRefresh::new(1, 2_099);

        assert_eq!(latest_lease_refresh(selected, Some(queued)), queued);
        assert_eq!(latest_lease_refresh(selected, None), selected);
    }

    #[test]
    fn unguarded_expiry_disarms_without_requesting_release() {
        let mut lease = RuntimeLease::new(CONTROL_LEASE_MS);
        assert_eq!(
            lease.transition(1, 100, false, Some(LeaseRefresh::new(1, 100))),
            None
        );

        assert_eq!(lease.transition(1, 2_100, false, None), None);
        assert!(!lease.is_armed());
    }

    #[test]
    fn remaining_time_wraps_across_u64_max() {
        let mut lease = RuntimeLease::new(250);
        assert_eq!(
            lease.transition(
                1,
                u64::MAX - 100,
                false,
                Some(LeaseRefresh::new(1, u64::MAX - 100)),
            ),
            None
        );

        assert_eq!(lease.deadline_ms(), Some(149));
        assert_eq!(lease.remaining_ms(u64::MAX - 50), Some(200));
        assert_eq!(lease.remaining_ms(148), Some(1));
        assert_eq!(lease.remaining_ms(149), Some(0));
    }

    #[test]
    fn dtr_loss_resets_session_before_releasing_inputs() {
        let action = runtime_safety_action(SafetyEvent::DtrLost);

        assert_eq!(
            action.steps(),
            [
                RuntimeSafetyStep::ResetSession,
                RuntimeSafetyStep::ReleaseInputs,
            ]
        );
    }
}
