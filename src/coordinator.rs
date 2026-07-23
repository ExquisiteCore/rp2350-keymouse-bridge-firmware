use crate::commands::{CommandError, decode_command};
use crate::error::ErrorCode;
use crate::owned_command::OwnedCommand;
use crate::protocol::{CommandType, Frame, RequestKind, validate_request};
use heapless::Vec;

pub const RESPONSE_CACHE_SIZE: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum BusyReason {
    ActiveDuplicate = 1,
    BatchExecuting = 2,
    ExecutorOccupied = 3,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CachedResponse {
    pub version: u8,
    pub sequence: u16,
    pub command_type: CommandType,
    pub payload: Vec<u8, 16>,
}

impl CachedResponse {
    pub fn ack(version: u8, sequence: u16) -> Self {
        Self {
            version,
            sequence,
            command_type: CommandType::Ack,
            payload: Vec::new(),
        }
    }

    pub fn nack(version: u8, sequence: u16, error: ErrorCode) -> Self {
        let mut payload = Vec::new();
        let pushed = payload.push(error as u8);
        debug_assert!(pushed.is_ok());
        Self {
            version,
            sequence,
            command_type: CommandType::Nack,
            payload,
        }
    }

    pub fn busy(version: u8, sequence: u16, reason: BusyReason, retry_ms: u16) -> Self {
        let mut payload = Vec::new();
        let pushed = payload.extend_from_slice(&[
            reason as u8,
            retry_ms.to_be_bytes()[0],
            retry_ms.to_be_bytes()[1],
        ]);
        debug_assert!(pushed.is_ok());
        Self {
            version,
            sequence,
            command_type: CommandType::Busy,
            payload,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
// Commands stay inline so requests remain allocation-free under `no_std`.
#[allow(clippy::large_enum_variant)]
pub enum OwnedRequestBody {
    Command(OwnedCommand),
    BatchBegin,
    BatchEnd,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnedRequest {
    pub version: u8,
    pub flags: u8,
    pub sequence: u16,
    pub command_type: CommandType,
    pub fingerprint: u64,
    pub request_kind: RequestKind,
    pub body: OwnedRequestBody,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Admission {
    Execute(OwnedRequest),
    Collect(OwnedRequest),
    Replay(CachedResponse),
    Busy(BusyReason),
    Immediate(CachedResponse),
    Reject(ErrorCode),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionError {
    MissingActive,
    ResponseSequenceMismatch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BatchMode {
    Idle,
    Collecting,
    Executing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BatchState {
    Idle,
    Collecting,
    Executing { owner: u16 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RequestClass {
    BatchBegin,
    BatchEnd,
    StopAll,
    Bypass,
    Mutating,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum CacheState {
    Active,
    Completed(CachedResponse),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CacheEntry {
    sequence: u16,
    fingerprint: u64,
    state: CacheState,
}

pub struct Coordinator {
    cache: Vec<CacheEntry, RESPONSE_CACHE_SIZE>,
    next_evict: usize,
    executor_owner: Option<u16>,
    batch_state: BatchState,
}

impl Coordinator {
    pub const fn new() -> Self {
        Self {
            cache: Vec::new(),
            next_evict: 0,
            executor_owner: None,
            batch_state: BatchState::Idle,
        }
    }

    pub fn has_cached_sequence(&self, sequence: u16) -> bool {
        self.cache.iter().any(|entry| entry.sequence == sequence)
    }

    pub const fn is_executor_occupied(&self) -> bool {
        self.executor_owner.is_some()
    }

    pub const fn batch_mode(&self) -> BatchMode {
        match self.batch_state {
            BatchState::Idle => BatchMode::Idle,
            BatchState::Collecting => BatchMode::Collecting,
            BatchState::Executing { .. } => BatchMode::Executing,
        }
    }

    pub fn cancel_batch(&mut self) {
        self.batch_state = BatchState::Idle;
    }

    pub fn clear_session(&mut self) {
        self.cache.clear();
        self.next_evict = 0;
        self.executor_owner = None;
        self.batch_state = BatchState::Idle;
    }

    pub fn admit(&mut self, frame: &Frame<'_>) -> Admission {
        let request_kind = match validate_request(frame) {
            Ok(kind) => kind,
            Err(error) => return Admission::Reject(error.into()),
        };
        let fingerprint = fingerprint(frame);
        if request_kind == RequestKind::ResponseExpected
            && let Some(entry) = self
                .cache
                .iter()
                .find(|entry| entry.sequence == frame.sequence)
        {
            if entry.fingerprint != fingerprint {
                return Admission::Reject(ErrorCode::SequenceConflict);
            }
            return match &entry.state {
                CacheState::Active => Admission::Busy(BusyReason::ActiveDuplicate),
                CacheState::Completed(response) => Admission::Replay(response.clone()),
            };
        }

        let body = match own_request_body(frame) {
            Ok(body) => body,
            Err(error) => return Admission::Reject(error.into()),
        };
        let request_class = classify_request(&body);
        let request = OwnedRequest {
            version: frame.version,
            flags: frame.flags,
            sequence: frame.sequence,
            command_type: frame.command_type,
            fingerprint,
            request_kind,
            body,
        };

        match request_class {
            RequestClass::BatchBegin => self.admit_batch_begin(request),
            RequestClass::BatchEnd => self.admit_batch_end(request),
            RequestClass::StopAll => {
                if self.batch_state == BatchState::Collecting {
                    self.batch_state = BatchState::Idle;
                }
                self.admit_bypass(request, true)
            }
            RequestClass::Bypass => self.admit_bypass(request, false),
            RequestClass::Mutating => self.admit_mutating(request),
        }
    }

    pub fn complete(
        &mut self,
        sequence: u16,
        response: CachedResponse,
    ) -> Result<(), CompletionError> {
        if response.sequence != sequence {
            return Err(CompletionError::ResponseSequenceMismatch);
        }
        let entry = self
            .cache
            .iter_mut()
            .find(|entry| entry.sequence == sequence && entry.state == CacheState::Active)
            .ok_or(CompletionError::MissingActive)?;
        entry.state = CacheState::Completed(response);
        if self.executor_owner == Some(sequence) {
            self.executor_owner = None;
        }
        if matches!(
            self.batch_state,
            BatchState::Executing { owner } if owner == sequence
        ) {
            self.batch_state = BatchState::Idle;
        }
        Ok(())
    }

    fn admit_batch_begin(&mut self, request: OwnedRequest) -> Admission {
        if self.batch_state != BatchState::Idle {
            return Admission::Reject(ErrorCode::BatchState);
        }
        if self.executor_owner.is_some() {
            return Admission::Busy(BusyReason::ExecutorOccupied);
        }
        if !self.insert_active(request.sequence, request.fingerprint, false) {
            return Admission::Busy(BusyReason::ExecutorOccupied);
        }

        self.batch_state = BatchState::Collecting;
        let response = CachedResponse::ack(request.version, request.sequence);
        let completed = self.complete(request.sequence, response.clone());
        debug_assert_eq!(completed, Ok(()));
        Admission::Immediate(response)
    }

    fn admit_batch_end(&mut self, request: OwnedRequest) -> Admission {
        if self.batch_state != BatchState::Collecting {
            return Admission::Reject(ErrorCode::BatchState);
        }
        if !self.insert_active(request.sequence, request.fingerprint, false) {
            return Admission::Busy(BusyReason::ExecutorOccupied);
        }

        self.batch_state = BatchState::Executing {
            owner: request.sequence,
        };
        Admission::Execute(request)
    }

    fn admit_bypass(&mut self, request: OwnedRequest, urgent: bool) -> Admission {
        if request.request_kind == RequestKind::NoResponseHeartbeat {
            return Admission::Execute(request);
        }
        if !self.insert_active(request.sequence, request.fingerprint, urgent) {
            return Admission::Busy(BusyReason::ExecutorOccupied);
        }
        Admission::Execute(request)
    }

    fn admit_mutating(&mut self, request: OwnedRequest) -> Admission {
        match self.batch_state {
            BatchState::Executing { .. } => Admission::Busy(BusyReason::BatchExecuting),
            BatchState::Collecting => {
                if !self.insert_active(request.sequence, request.fingerprint, false) {
                    return Admission::Busy(BusyReason::ExecutorOccupied);
                }
                Admission::Collect(request)
            }
            BatchState::Idle if self.executor_owner.is_some() => {
                Admission::Busy(BusyReason::ExecutorOccupied)
            }
            BatchState::Idle => {
                if !self.insert_active(request.sequence, request.fingerprint, false) {
                    return Admission::Busy(BusyReason::ExecutorOccupied);
                }
                self.executor_owner = Some(request.sequence);
                Admission::Execute(request)
            }
        }
    }

    fn insert_active(&mut self, sequence: u16, fingerprint: u64, urgent: bool) -> bool {
        let entry = CacheEntry {
            sequence,
            fingerprint,
            state: CacheState::Active,
        };
        if !urgent
            && self.cache.len() == RESPONSE_CACHE_SIZE - 1
            && self
                .cache
                .iter()
                .all(|entry| entry.state == CacheState::Active)
        {
            return false;
        }
        if self.cache.len() < RESPONSE_CACHE_SIZE {
            return self.cache.push(entry).is_ok();
        }

        for offset in 0..RESPONSE_CACHE_SIZE {
            let index = (self.next_evict + offset) % RESPONSE_CACHE_SIZE;
            if matches!(self.cache[index].state, CacheState::Completed(_)) {
                self.cache[index] = entry;
                self.next_evict = (index + 1) % RESPONSE_CACHE_SIZE;
                return true;
            }
        }

        false
    }
}

fn own_request_body(frame: &Frame<'_>) -> Result<OwnedRequestBody, CommandError> {
    match frame.command_type {
        CommandType::BatchBegin => {
            decode_command(frame)?;
            Ok(OwnedRequestBody::BatchBegin)
        }
        CommandType::BatchEnd => {
            decode_command(frame)?;
            Ok(OwnedRequestBody::BatchEnd)
        }
        _ => OwnedCommand::from_frame(frame).map(OwnedRequestBody::Command),
    }
}

fn classify_request(body: &OwnedRequestBody) -> RequestClass {
    match body {
        OwnedRequestBody::BatchBegin => RequestClass::BatchBegin,
        OwnedRequestBody::BatchEnd => RequestClass::BatchEnd,
        OwnedRequestBody::Command(OwnedCommand::StopAll) => RequestClass::StopAll,
        OwnedRequestBody::Command(
            OwnedCommand::Ping
            | OwnedCommand::GetInfo
            | OwnedCommand::GetCaps
            | OwnedCommand::Heartbeat,
        ) => RequestClass::Bypass,
        OwnedRequestBody::Command(_) => RequestClass::Mutating,
    }
}

impl Default for Coordinator {
    fn default() -> Self {
        Self::new()
    }
}

fn fingerprint(frame: &Frame<'_>) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    let payload_len = (frame.payload.len() as u16).to_be_bytes();
    [frame.version, frame.flags, frame.command_type.as_byte()]
        .iter()
        .chain(payload_len.iter())
        .chain(frame.payload.iter())
        .fold(OFFSET_BASIS, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(PRIME)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{CommandType, FLAG_NO_RESPONSE, Frame, MAX_WAIT_MS};

    fn request<'a>(
        version: u8,
        sequence: u16,
        command_type: CommandType,
        payload: &'a [u8],
    ) -> Frame<'a> {
        Frame {
            version,
            flags: 0,
            sequence,
            command_type,
            payload,
        }
    }

    fn request_with_flags<'a>(
        version: u8,
        flags: u8,
        sequence: u16,
        command_type: CommandType,
        payload: &'a [u8],
    ) -> Frame<'a> {
        Frame {
            version,
            flags,
            sequence,
            command_type,
            payload,
        }
    }

    #[test]
    fn active_duplicate_is_busy_then_completed_duplicate_replays() {
        let request = request(2, 41, CommandType::MouseClick, &[1]);
        let mut coordinator = Coordinator::new();
        assert!(matches!(coordinator.admit(&request), Admission::Execute(_)));
        assert!(matches!(
            coordinator.admit(&request),
            Admission::Busy(BusyReason::ActiveDuplicate)
        ));
        coordinator
            .complete(41, CachedResponse::ack(2, 41))
            .unwrap();
        assert_eq!(
            coordinator.admit(&request),
            Admission::Replay(CachedResponse::ack(2, 41))
        );
    }

    #[test]
    fn reused_sequence_with_different_payload_conflicts() {
        let mut coordinator = Coordinator::new();
        let first = request(2, 9, CommandType::MouseMoveRel, &[0, 1, 0, 1]);
        let second = request(2, 9, CommandType::MouseMoveRel, &[0, 2, 0, 1]);
        assert!(matches!(coordinator.admit(&first), Admission::Execute(_)));
        assert_eq!(
            coordinator.admit(&second),
            Admission::Reject(crate::error::ErrorCode::SequenceConflict)
        );
    }

    #[test]
    fn validation_and_command_errors_reject_with_precise_codes() {
        let excessive_wait = (MAX_WAIT_MS + 1).to_be_bytes();
        let invalid_requests = [
            (
                request(3, 1, CommandType::Ping, &[]),
                ErrorCode::UnsupportedVersion,
            ),
            (
                request_with_flags(2, FLAG_NO_RESPONSE, 1, CommandType::Heartbeat, &[]),
                ErrorCode::UnsupportedFlags,
            ),
            (
                request(2, 0, CommandType::Ping, &[]),
                ErrorCode::InvalidSequence,
            ),
            (
                request(2, 2, CommandType::WaitMs, &excessive_wait),
                ErrorCode::WaitTooLong,
            ),
            (
                request(2, 3, CommandType::MouseClick, &[]),
                ErrorCode::BadCommand,
            ),
            (
                request(2, 4, CommandType::Unknown(0x55), &[]),
                ErrorCode::BadCommand,
            ),
        ];

        for (invalid, expected) in invalid_requests {
            assert_eq!(
                Coordinator::new().admit(&invalid),
                Admission::Reject(expected)
            );
        }
    }

    #[test]
    fn response_payloads_and_busy_reason_ids_are_wire_stable() {
        let ack = CachedResponse::ack(2, 0x1234);
        assert_eq!(ack.command_type, CommandType::Ack);
        assert!(ack.payload.is_empty());

        let nack = CachedResponse::nack(2, 0x1234, ErrorCode::WaitTooLong);
        assert_eq!(nack.command_type, CommandType::Nack);
        assert_eq!(nack.payload.as_slice(), &[ErrorCode::WaitTooLong as u8]);

        let busy = CachedResponse::busy(2, 0x1234, BusyReason::ExecutorOccupied, 0x3456);
        assert_eq!(busy.command_type, CommandType::Busy);
        assert_eq!(busy.payload.as_slice(), &[3, 0x34, 0x56]);

        assert_eq!(BusyReason::ActiveDuplicate as u8, 1);
        assert_eq!(BusyReason::BatchExecuting as u8, 2);
        assert_eq!(BusyReason::ExecutorOccupied as u8, 3);
    }

    #[test]
    fn fingerprint_has_a_known_wire_vector_and_covers_header_fields() {
        let base = request(2, 7, CommandType::MouseMoveRel, &[0, 1, 0, 1]);
        assert_eq!(fingerprint(&base), 0x2724_b501_1926_1401);

        let changed_version = request(1, 7, CommandType::MouseMoveRel, &[0, 1, 0, 1]);
        let changed_flags = request_with_flags(
            2,
            FLAG_NO_RESPONSE,
            7,
            CommandType::MouseMoveRel,
            &[0, 1, 0, 1],
        );
        let changed_command = request(2, 7, CommandType::WaitMs, &[0, 1, 0, 1]);
        let changed_sequence = request(2, 8, CommandType::MouseMoveRel, &[0, 1, 0, 1]);

        assert_ne!(fingerprint(&base), fingerprint(&changed_version));
        assert_ne!(fingerprint(&base), fingerprint(&changed_flags));
        assert_ne!(fingerprint(&base), fingerprint(&changed_command));
        assert_eq!(fingerprint(&base), fingerprint(&changed_sequence));
    }

    #[test]
    fn admitted_request_owns_command_and_wire_metadata() {
        let frame = request(2, 17, CommandType::TypeAscii, b"abc");
        let expected_fingerprint = fingerprint(&frame);
        let Admission::Execute(owned) = Coordinator::new().admit(&frame) else {
            panic!("ordinary command should execute");
        };

        assert_eq!(owned.version, 2);
        assert_eq!(owned.flags, 0);
        assert_eq!(owned.sequence, 17);
        assert_eq!(owned.command_type, CommandType::TypeAscii);
        assert_eq!(owned.fingerprint, expected_fingerprint);
        assert_eq!(owned.request_kind, RequestKind::ResponseExpected);
        assert_eq!(
            owned.body,
            OwnedRequestBody::Command(OwnedCommand::type_ascii(b"abc").unwrap())
        );
    }

    #[test]
    fn completion_rejects_response_mismatch_and_missing_active_sequence() {
        let mut coordinator = Coordinator::new();
        let frame = request(2, 21, CommandType::MouseClick, &[1]);
        assert!(matches!(coordinator.admit(&frame), Admission::Execute(_)));

        assert_eq!(
            coordinator.complete(21, CachedResponse::ack(2, 22)),
            Err(CompletionError::ResponseSequenceMismatch)
        );
        assert_eq!(
            coordinator.admit(&frame),
            Admission::Busy(BusyReason::ActiveDuplicate)
        );
        assert_eq!(
            coordinator.complete(99, CachedResponse::ack(2, 99)),
            Err(CompletionError::MissingActive)
        );

        coordinator
            .complete(21, CachedResponse::nack(2, 21, ErrorCode::Cancelled))
            .unwrap();
        assert_eq!(
            coordinator.complete(21, CachedResponse::ack(2, 21)),
            Err(CompletionError::MissingActive)
        );
        assert_eq!(
            coordinator.admit(&frame),
            Admission::Replay(CachedResponse::nack(2, 21, ErrorCode::Cancelled))
        );
    }

    #[test]
    fn cache_evicts_oldest_completed_entry_and_clear_session_allows_reuse() {
        let mut coordinator = Coordinator::new();
        assert_eq!(RESPONSE_CACHE_SIZE, 64);

        for sequence in 1..=RESPONSE_CACHE_SIZE as u16 {
            let frame = request(2, sequence, CommandType::Ping, &[]);
            assert!(matches!(coordinator.admit(&frame), Admission::Execute(_)));
            coordinator
                .complete(sequence, CachedResponse::ack(2, sequence))
                .unwrap();
        }
        assert!(coordinator.has_cached_sequence(1));

        let newest = request(2, 65, CommandType::Ping, &[]);
        assert!(matches!(coordinator.admit(&newest), Admission::Execute(_)));
        assert!(!coordinator.has_cached_sequence(1));
        assert!(coordinator.has_cached_sequence(65));

        let reused = request(2, 1, CommandType::MouseClick, &[1]);
        assert!(matches!(coordinator.admit(&reused), Admission::Execute(_)));
        assert!(coordinator.is_executor_occupied());

        coordinator.clear_session();
        assert!(!coordinator.has_cached_sequence(1));
        assert!(!coordinator.has_cached_sequence(65));
        assert!(!coordinator.is_executor_occupied());
        assert!(matches!(coordinator.admit(&newest), Admission::Execute(_)));
    }

    #[test]
    fn cache_churn_never_evicts_active_entry_when_completed_entry_exists() {
        let mut coordinator = Coordinator::new();
        let active = request(2, 1, CommandType::Ping, &[]);
        assert!(matches!(coordinator.admit(&active), Admission::Execute(_)));

        for sequence in 2..=RESPONSE_CACHE_SIZE as u16 {
            let frame = request(2, sequence, CommandType::GetInfo, &[]);
            assert!(matches!(coordinator.admit(&frame), Admission::Execute(_)));
            coordinator
                .complete(sequence, CachedResponse::ack(2, sequence))
                .unwrap();
        }

        let churn = request(2, 65, CommandType::GetCaps, &[]);
        assert!(matches!(coordinator.admit(&churn), Admission::Execute(_)));
        assert!(coordinator.has_cached_sequence(1));
        assert_eq!(
            coordinator.admit(&active),
            Admission::Busy(BusyReason::ActiveDuplicate)
        );
    }

    #[test]
    fn occupied_ordinary_command_is_not_cached_and_retry_executes_after_owner_completes() {
        let mut coordinator = Coordinator::new();
        let owner = request(2, 31, CommandType::MouseMoveRel, &[0, 1, 0, 1]);
        let waiting = request(2, 32, CommandType::MouseClick, &[1]);

        assert!(matches!(coordinator.admit(&owner), Admission::Execute(_)));
        assert!(coordinator.is_executor_occupied());
        assert_eq!(
            coordinator.admit(&waiting),
            Admission::Busy(BusyReason::ExecutorOccupied)
        );
        assert!(!coordinator.has_cached_sequence(32));

        coordinator
            .complete(31, CachedResponse::ack(2, 31))
            .unwrap();
        assert!(!coordinator.is_executor_occupied());
        assert!(matches!(coordinator.admit(&waiting), Admission::Execute(_)));
        assert!(coordinator.has_cached_sequence(32));
    }

    #[test]
    fn occupied_executor_allows_every_bypass_without_releasing_owner() {
        let mut coordinator = Coordinator::new();
        let owner = request(2, 40, CommandType::WaitMs, &[0, 0, 0, 1]);
        assert!(matches!(coordinator.admit(&owner), Admission::Execute(_)));

        for (sequence, command_type) in [
            (41, CommandType::Ping),
            (42, CommandType::GetInfo),
            (43, CommandType::GetCaps),
            (44, CommandType::Heartbeat),
            (45, CommandType::StopAll),
        ] {
            let bypass = request(2, sequence, command_type, &[]);
            assert!(matches!(coordinator.admit(&bypass), Admission::Execute(_)));
            coordinator
                .complete(sequence, CachedResponse::ack(2, sequence))
                .unwrap();
            assert!(coordinator.is_executor_occupied());

            let ordinary = request(2, sequence + 100, CommandType::MouseClick, &[1]);
            assert_eq!(
                coordinator.admit(&ordinary),
                Admission::Busy(BusyReason::ExecutorOccupied)
            );
            assert!(!coordinator.has_cached_sequence(sequence + 100));
        }

        coordinator
            .complete(40, CachedResponse::ack(2, 40))
            .unwrap();
        assert!(!coordinator.is_executor_occupied());
    }

    #[test]
    fn no_response_heartbeat_bypasses_cache_but_diagnostic_heartbeat_replays() {
        let mut coordinator = Coordinator::new();
        let no_response = request_with_flags(2, FLAG_NO_RESPONSE, 0, CommandType::Heartbeat, &[]);

        for _ in 0..2 {
            let Admission::Execute(owned) = coordinator.admit(&no_response) else {
                panic!("no-response heartbeat should dispatch without a response entry");
            };
            assert_eq!(owned.request_kind, RequestKind::NoResponseHeartbeat);
            assert_eq!(owned.sequence, 0);
            assert!(!coordinator.has_cached_sequence(0));
        }

        let diagnostic = request(2, 46, CommandType::Heartbeat, &[]);
        assert!(matches!(
            coordinator.admit(&diagnostic),
            Admission::Execute(_)
        ));
        assert!(coordinator.has_cached_sequence(46));
        coordinator
            .complete(46, CachedResponse::ack(2, 46))
            .unwrap();
        assert_eq!(
            coordinator.admit(&diagnostic),
            Admission::Replay(CachedResponse::ack(2, 46))
        );
    }

    #[test]
    fn invalid_no_response_uses_remain_rejected() {
        let cases = [
            (
                request_with_flags(2, FLAG_NO_RESPONSE, 1, CommandType::Heartbeat, &[]),
                ErrorCode::UnsupportedFlags,
            ),
            (
                request_with_flags(2, FLAG_NO_RESPONSE, 0, CommandType::Ping, &[]),
                ErrorCode::UnsupportedFlags,
            ),
            (
                request(2, 0, CommandType::Heartbeat, &[]),
                ErrorCode::InvalidSequence,
            ),
        ];

        for (frame, expected) in cases {
            assert_eq!(
                Coordinator::new().admit(&frame),
                Admission::Reject(expected)
            );
        }
    }

    fn begin_batch(coordinator: &mut Coordinator, sequence: u16) {
        let begin = request(2, sequence, CommandType::BatchBegin, &[]);
        assert_eq!(
            coordinator.admit(&begin),
            Admission::Immediate(CachedResponse::ack(2, sequence))
        );
        assert_eq!(coordinator.batch_mode(), BatchMode::Collecting);
    }

    #[test]
    fn batch_begin_replays_duplicate_and_rejects_nested_or_unmatched_end() {
        let mut coordinator = Coordinator::new();
        let unmatched_end = request(2, 70, CommandType::BatchEnd, &[]);
        assert_eq!(
            coordinator.admit(&unmatched_end),
            Admission::Reject(ErrorCode::BatchState)
        );

        let begin = request(2, 71, CommandType::BatchBegin, &[]);
        assert_eq!(
            coordinator.admit(&begin),
            Admission::Immediate(CachedResponse::ack(2, 71))
        );
        assert_eq!(
            coordinator.admit(&begin),
            Admission::Replay(CachedResponse::ack(2, 71))
        );

        let nested = request(2, 72, CommandType::BatchBegin, &[]);
        assert_eq!(
            coordinator.admit(&nested),
            Admission::Reject(ErrorCode::BatchState)
        );
        assert!(!coordinator.has_cached_sequence(72));
        assert_eq!(coordinator.batch_mode(), BatchMode::Collecting);
    }

    #[test]
    fn every_batchable_action_collects_and_completed_retry_never_recollects() {
        let mut coordinator = Coordinator::new();
        begin_batch(&mut coordinator, 80);

        let cases: &[(u16, CommandType, &[u8])] = &[
            (81, CommandType::KeyDown, &[0, 4]),
            (82, CommandType::KeyUp, &[0, 4]),
            (83, CommandType::KeyTap, &[0, 5]),
            (84, CommandType::TypeAscii, b"hi"),
            (85, CommandType::MouseMoveRel, &[0, 1, 0, 2]),
            (86, CommandType::MouseButtonDown, &[1]),
            (87, CommandType::MouseButtonUp, &[1]),
            (88, CommandType::MouseClick, &[1]),
            (89, CommandType::MouseWheel, &[1]),
            (90, CommandType::WaitMs, &[0, 0, 0, 1]),
        ];

        for (index, (sequence, command_type, payload)) in cases.iter().enumerate() {
            let frame = request(2, *sequence, *command_type, payload);
            let Admission::Collect(owned) = coordinator.admit(&frame) else {
                panic!("batchable command should be returned to the dispatcher collector");
            };
            assert_eq!(owned.sequence, *sequence);
            assert_eq!(owned.command_type, *command_type);
            assert!(matches!(owned.body, OwnedRequestBody::Command(_)));

            if index == 0 {
                assert_eq!(
                    coordinator.admit(&frame),
                    Admission::Busy(BusyReason::ActiveDuplicate)
                );
            }
            coordinator
                .complete(*sequence, CachedResponse::ack(2, *sequence))
                .unwrap();
            assert_eq!(
                coordinator.admit(&frame),
                Admission::Replay(CachedResponse::ack(2, *sequence))
            );
            assert_eq!(coordinator.batch_mode(), BatchMode::Collecting);
        }
    }

    #[test]
    fn batch_end_owns_exclusive_execution_and_only_owner_completion_exits() {
        let mut coordinator = Coordinator::new();
        begin_batch(&mut coordinator, 100);
        let end = request(2, 101, CommandType::BatchEnd, &[]);
        let Admission::Execute(owned_end) = coordinator.admit(&end) else {
            panic!("batch end should transfer the collected batch to execution");
        };
        assert_eq!(owned_end.body, OwnedRequestBody::BatchEnd);
        assert_eq!(coordinator.batch_mode(), BatchMode::Executing);

        let ordinary = request(2, 102, CommandType::MouseClick, &[1]);
        assert_eq!(
            coordinator.admit(&ordinary),
            Admission::Busy(BusyReason::BatchExecuting)
        );
        assert!(!coordinator.has_cached_sequence(102));

        let query = request(2, 103, CommandType::GetInfo, &[]);
        assert!(matches!(coordinator.admit(&query), Admission::Execute(_)));
        coordinator
            .complete(103, CachedResponse::ack(2, 103))
            .unwrap();
        assert_eq!(coordinator.batch_mode(), BatchMode::Executing);
        assert_eq!(
            coordinator.complete(101, CachedResponse::ack(2, 999)),
            Err(CompletionError::ResponseSequenceMismatch)
        );
        assert_eq!(coordinator.batch_mode(), BatchMode::Executing);

        coordinator
            .complete(101, CachedResponse::ack(2, 101))
            .unwrap();
        assert_eq!(coordinator.batch_mode(), BatchMode::Idle);
        assert_eq!(
            coordinator.admit(&end),
            Admission::Replay(CachedResponse::ack(2, 101))
        );
    }

    #[test]
    fn batch_execution_allows_every_bypass_and_explicit_cancellation_clears_mode() {
        let mut coordinator = Coordinator::new();
        begin_batch(&mut coordinator, 110);
        let end = request(2, 111, CommandType::BatchEnd, &[]);
        assert!(matches!(coordinator.admit(&end), Admission::Execute(_)));

        for (sequence, command_type) in [
            (112, CommandType::Ping),
            (113, CommandType::GetInfo),
            (114, CommandType::GetCaps),
            (115, CommandType::Heartbeat),
            (116, CommandType::StopAll),
        ] {
            let bypass = request(2, sequence, command_type, &[]);
            assert!(matches!(coordinator.admit(&bypass), Admission::Execute(_)));
            coordinator
                .complete(sequence, CachedResponse::ack(2, sequence))
                .unwrap();
            assert_eq!(coordinator.batch_mode(), BatchMode::Executing);
        }

        coordinator.cancel_batch();
        assert_eq!(coordinator.batch_mode(), BatchMode::Idle);
        coordinator
            .complete(111, CachedResponse::nack(2, 111, ErrorCode::Cancelled))
            .unwrap();
        assert_eq!(coordinator.batch_mode(), BatchMode::Idle);
    }

    #[test]
    fn collecting_bypasses_dispatch_and_stop_all_closes_collection() {
        let mut coordinator = Coordinator::new();
        begin_batch(&mut coordinator, 120);

        for (sequence, command_type) in [
            (121, CommandType::Ping),
            (122, CommandType::GetInfo),
            (123, CommandType::GetCaps),
            (124, CommandType::Heartbeat),
        ] {
            let bypass = request(2, sequence, command_type, &[]);
            assert!(matches!(coordinator.admit(&bypass), Admission::Execute(_)));
            coordinator
                .complete(sequence, CachedResponse::ack(2, sequence))
                .unwrap();
            assert_eq!(coordinator.batch_mode(), BatchMode::Collecting);
        }

        let stop = request(2, 125, CommandType::StopAll, &[]);
        assert!(matches!(coordinator.admit(&stop), Admission::Execute(_)));
        assert_eq!(coordinator.batch_mode(), BatchMode::Idle);
        coordinator
            .complete(125, CachedResponse::ack(2, 125))
            .unwrap();

        let stale_end = request(2, 126, CommandType::BatchEnd, &[]);
        assert_eq!(
            coordinator.admit(&stale_end),
            Admission::Reject(ErrorCode::BatchState)
        );

        begin_batch(&mut coordinator, 127);
        coordinator.clear_session();
        assert_eq!(coordinator.batch_mode(), BatchMode::Idle);
        assert!(!coordinator.has_cached_sequence(127));
        begin_batch(&mut coordinator, 127);
    }

    #[test]
    fn batch_begin_waits_for_active_ordinary_executor_without_caching() {
        let mut coordinator = Coordinator::new();
        let owner = request(2, 130, CommandType::MouseClick, &[1]);
        let begin = request(2, 131, CommandType::BatchBegin, &[]);
        assert!(matches!(coordinator.admit(&owner), Admission::Execute(_)));
        assert_eq!(
            coordinator.admit(&begin),
            Admission::Busy(BusyReason::ExecutorOccupied)
        );
        assert!(!coordinator.has_cached_sequence(131));

        coordinator
            .complete(130, CachedResponse::ack(2, 130))
            .unwrap();
        assert_eq!(
            coordinator.admit(&begin),
            Admission::Immediate(CachedResponse::ack(2, 131))
        );
    }

    #[test]
    fn active_cache_saturation_preserves_an_urgent_stop_all_slot() {
        let mut coordinator = Coordinator::new();
        let owner = request(2, 200, CommandType::WaitMs, &[0, 0, 0, 1]);
        assert!(matches!(coordinator.admit(&owner), Admission::Execute(_)));

        for sequence in 201..263 {
            let query = request(2, sequence, CommandType::Ping, &[]);
            assert!(matches!(coordinator.admit(&query), Admission::Execute(_)));
        }

        let saturated_query = request(2, 263, CommandType::GetInfo, &[]);
        assert_eq!(
            coordinator.admit(&saturated_query),
            Admission::Busy(BusyReason::ExecutorOccupied)
        );
        assert!(!coordinator.has_cached_sequence(263));

        let stop = request(2, 264, CommandType::StopAll, &[]);
        assert!(matches!(coordinator.admit(&stop), Admission::Execute(_)));
        assert!(coordinator.has_cached_sequence(264));
        assert!(coordinator.has_cached_sequence(200));
        assert!(coordinator.is_executor_occupied());
    }

    #[test]
    fn completed_entry_replays_exact_response_and_still_rejects_sequence_conflict() {
        let mut coordinator = Coordinator::new();
        let original = request(2, 300, CommandType::MouseMoveRel, &[0, 1, 0, 1]);
        let conflicting = request(2, 300, CommandType::MouseMoveRel, &[0, 2, 0, 1]);
        assert!(matches!(
            coordinator.admit(&original),
            Admission::Execute(_)
        ));

        let mut payload = Vec::<u8, 16>::new();
        payload.extend_from_slice(b"done").unwrap();
        let response = CachedResponse {
            version: 2,
            sequence: 300,
            command_type: CommandType::Status,
            payload,
        };
        coordinator.complete(300, response.clone()).unwrap();

        assert_eq!(coordinator.admit(&original), Admission::Replay(response));
        assert_eq!(
            coordinator.admit(&conflicting),
            Admission::Reject(ErrorCode::SequenceConflict)
        );
    }

    #[test]
    fn owned_request_body_represents_both_batch_markers() {
        assert_eq!(
            own_request_body(&request(2, 310, CommandType::BatchBegin, &[])),
            Ok(OwnedRequestBody::BatchBegin)
        );
        assert_eq!(
            own_request_body(&request(2, 311, CommandType::BatchEnd, &[])),
            Ok(OwnedRequestBody::BatchEnd)
        );
        assert_eq!(
            own_request_body(&request(2, 312, CommandType::BatchBegin, &[1])),
            Err(CommandError::InvalidPayloadLength)
        );
    }
}
