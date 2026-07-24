//! Target-independent STOP_ALL cancellation and emergency-release lifecycle.

use heapless::Vec;

use crate::coordinator::{CompletionToken, OwnedRequest, RESPONSE_CACHE_SIZE};
use crate::safety::CancellationGeneration;

/// Adapter work emitted when an emergency release is requested.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EmergencyAction {
    cancel_generation: u32,
    enqueue_emergency: bool,
}

impl EmergencyAction {
    /// Generation that must be published to the active executor.
    pub const fn cancel_generation(self) -> u32 {
        self.cancel_generation
    }

    /// Whether the adapter must enqueue the one coalesced emergency reset.
    pub const fn enqueue_emergency(self) -> bool {
        self.enqueue_emergency
    }
}

/// Production state shared by the dispatcher adapter and deterministic host tests.
pub struct StopCore {
    cancellation: CancellationGeneration,
    pending_stops: Vec<CompletionToken, RESPONSE_CACHE_SIZE>,
    emergency_in_flight: bool,
}

impl StopCore {
    /// Creates an idle emergency lifecycle.
    pub const fn new() -> Self {
        Self {
            cancellation: CancellationGeneration::new(),
            pending_stops: Vec::new(),
            emergency_in_flight: false,
        }
    }

    /// Generation captured when an executor job starts.
    pub const fn current_generation(&self) -> u32 {
        self.cancellation.current()
    }

    /// Records an admitted STOP_ALL and requests cancellation plus a coalesced reset.
    pub fn handle_stop(&mut self, request: OwnedRequest) -> EmergencyAction {
        let token = request
            .completion_token()
            .expect("STOP_ALL admission must carry a completion token");
        let pushed = self.pending_stops.push(token);
        debug_assert!(pushed.is_ok());
        self.schedule_emergency()
    }

    /// Reports whether an emergency reset is queued or executing.
    pub const fn emergency_in_flight(&self) -> bool {
        self.emergency_in_flight
    }

    /// Number of STOP_ALL completions waiting for the coalesced reset.
    #[cfg(test)]
    pub fn pending_stop_count(&self) -> usize {
        self.pending_stops.len()
    }

    /// Finishes the coalesced reset and yields the STOP_ALL tokens now safe to ACK.
    pub fn complete_emergency(&mut self) -> Vec<CompletionToken, RESPONSE_CACHE_SIZE> {
        self.emergency_in_flight = false;
        core::mem::take(&mut self.pending_stops)
    }

    /// Invalidates STOP_ALL completions when their coordinator session is reset.
    pub fn clear_pending_stops(&mut self) {
        self.pending_stops.clear();
    }

    /// Requests cancellation and one coalesced emergency reset without a STOP_ALL token.
    pub fn schedule_emergency(&mut self) -> EmergencyAction {
        let cancel_generation = self.cancellation.cancel();
        let enqueue_emergency = !self.emergency_in_flight;
        self.emergency_in_flight = true;
        EmergencyAction {
            cancel_generation,
            enqueue_emergency,
        }
    }
}

impl Default for StopCore {
    fn default() -> Self {
        Self::new()
    }
}
