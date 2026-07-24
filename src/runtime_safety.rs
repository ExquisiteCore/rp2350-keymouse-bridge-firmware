//! Pure request and safety transitions shared by the Embassy runtime and host tests.

use crate::coordinator::{Admission, Coordinator, OwnedRequest, SafetyEvent};
use crate::error::ErrorCode;
use crate::protocol::PROTOCOL_VERSION;
use crate::safety::LeaseState;

/// Timestamped refresh emitted by the dispatcher after one request admission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeaseRefresh {
    observed_at_ms: u64,
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
    now_ms: u64,
) -> RuntimeAdmission {
    let version = request.version();
    let admission = coordinator.admit_prepared_with_external_busy(request, external_busy);
    let lease_refresh = refreshes_v2_lease(version, &admission).then_some(LeaseRefresh {
        observed_at_ms: now_ms,
    });
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
pub struct RuntimeSafetyAction {
    clear_session: bool,
    release_inputs: bool,
}

impl RuntimeSafetyAction {
    /// Reports whether queued/cached work from the current session must be invalidated.
    pub const fn clear_session(self) -> bool {
        self.clear_session
    }

    /// Reports whether the emergency executor must release keyboard and mouse state.
    pub const fn release_inputs(self) -> bool {
        self.release_inputs
    }
}

/// Converts DTR, USB, and lease failures into the common fail-safe transition.
pub const fn runtime_safety_action(_event: SafetyEvent) -> RuntimeSafetyAction {
    RuntimeSafetyAction {
        clear_session: true,
        release_inputs: true,
    }
}

/// Pure controller-lease state owned by the Embassy lease task.
pub struct RuntimeLease {
    lease: LeaseState,
}

impl RuntimeLease {
    /// Creates an unarmed runtime lease.
    pub const fn new(duration_ms: u64) -> Self {
        Self {
            lease: LeaseState::new(duration_ms),
        }
    }

    /// Applies a dispatcher-produced refresh event.
    pub fn observe(&mut self, refresh: LeaseRefresh) {
        self.lease.refresh(refresh.observed_at_ms);
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

    /// Polls the exact deadline and emits the common reset/release action when guarded.
    pub fn poll(&mut self, now_ms: u64, guarded_work: bool) -> Option<RuntimeSafetyAction> {
        if !self.lease.should_release(now_ms, true) {
            return None;
        }

        self.lease.clear();
        guarded_work.then_some(runtime_safety_action(SafetyEvent::LeaseExpired))
    }
}

#[cfg(test)]
mod tests {
    use super::{RuntimeLease, admit_runtime_request, runtime_safety_action};
    use crate::coordinator::{Admission, Coordinator, OwnedRequest, SafetyEvent};
    use crate::protocol::{CommandType, FLAG_NO_RESPONSE, Frame, PROTOCOL_VERSION};
    use crate::safety::CONTROL_LEASE_MS;

    fn request(
        sequence: u16,
        flags: u8,
        command_type: CommandType,
        payload: &[u8],
    ) -> OwnedRequest {
        OwnedRequest::from_frame(&Frame {
            version: PROTOCOL_VERSION,
            flags,
            sequence,
            command_type,
            payload,
        })
        .unwrap()
    }

    #[test]
    fn runtime_request_entry_refreshes_no_response_heartbeat_and_expires_exactly() {
        let mut coordinator = Coordinator::new();
        let mut lease = RuntimeLease::new(CONTROL_LEASE_MS);

        let initial = admit_runtime_request(
            &mut coordinator,
            request(47, 0, CommandType::KeyDown, &[0, 0x1a]),
            false,
            100,
        );
        assert!(initial.response_expected());
        lease.observe(initial.lease_refresh().unwrap());
        assert_eq!(lease.deadline_ms(), Some(2_100));

        let heartbeat = admit_runtime_request(
            &mut coordinator,
            request(0, FLAG_NO_RESPONSE, CommandType::Heartbeat, &[]),
            false,
            1_500,
        );
        assert!(matches!(heartbeat.admission(), Admission::NoResponse(_)));
        assert!(!heartbeat.response_expected());
        assert!(!coordinator.has_cached_sequence(0));
        lease.observe(heartbeat.lease_refresh().unwrap());
        assert_eq!(lease.deadline_ms(), Some(3_500));

        assert_eq!(lease.poll(2_100, true), None);
        assert_eq!(lease.poll(3_499, true), None);
        assert_eq!(
            lease.poll(3_500, true),
            Some(runtime_safety_action(SafetyEvent::LeaseExpired))
        );
    }

    #[test]
    fn dtr_loss_requires_session_clear_and_input_release() {
        let action = runtime_safety_action(SafetyEvent::DtrLost);

        assert!(action.clear_session());
        assert!(action.release_inputs());
    }
}
