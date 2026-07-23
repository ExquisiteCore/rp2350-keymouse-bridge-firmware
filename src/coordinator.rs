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

/// Opaque identity for completing one exact accepted request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompletionToken {
    session_generation: u64,
    admission_id: u64,
    version: u8,
    sequence: u16,
    fingerprint: u64,
}

impl CompletionToken {
    /// Returns the session generation in which this request was admitted.
    pub const fn session_generation(self) -> u64 {
        self.session_generation
    }

    /// Returns the unique admission identifier within the coordinator lifetime.
    pub const fn admission_id(self) -> u64 {
        self.admission_id
    }

    /// Returns the accepted protocol version.
    pub const fn version(self) -> u8 {
        self.version
    }

    /// Returns the accepted request sequence.
    pub const fn sequence(self) -> u16 {
        self.sequence
    }

    /// Returns the fingerprint of the accepted request fields and payload.
    pub const fn fingerprint(self) -> u64 {
        self.fingerprint
    }

    /// Builds a final ACK bound to this request's version and sequence.
    pub fn ack(self) -> CachedResponse {
        CachedResponse::ack(self.version, self.sequence)
    }

    /// Builds a final NACK bound to this request's version and sequence.
    pub fn nack(self, error: ErrorCode) -> CachedResponse {
        CachedResponse::nack(self.version, self.sequence, error)
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

/// Allocation-free request owned by the future dispatcher channel.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnedRequest {
    version: u8,
    flags: u8,
    sequence: u16,
    command_type: CommandType,
    fingerprint: u64,
    request_kind: RequestKind,
    completion_token: Option<CompletionToken>,
    body: OwnedRequestBody,
}

impl OwnedRequest {
    /// Returns the accepted protocol version.
    pub const fn version(&self) -> u8 {
        self.version
    }

    /// Returns the accepted request flags.
    pub const fn flags(&self) -> u8 {
        self.flags
    }

    /// Returns the accepted request sequence.
    pub const fn sequence(&self) -> u16 {
        self.sequence
    }

    /// Returns the accepted command type.
    pub const fn command_type(&self) -> CommandType {
        self.command_type
    }

    /// Returns the deterministic request fingerprint.
    pub const fn fingerprint(&self) -> u64 {
        self.fingerprint
    }

    /// Returns whether this request expects a response.
    pub const fn request_kind(&self) -> RequestKind {
        self.request_kind
    }

    /// Returns the exact completion identity, or `None` for no-response heartbeat.
    pub const fn completion_token(&self) -> Option<CompletionToken> {
        self.completion_token
    }

    /// Borrows the owned command or batch marker.
    pub const fn body(&self) -> &OwnedRequestBody {
        &self.body
    }

    /// Consumes the request and returns its owned command or batch marker.
    pub fn into_body(self) -> OwnedRequestBody {
        self.body
    }
}

/// Coordinator decision that makes dispatcher scheduling semantics explicit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Admission {
    /// Run an exclusive ordinary mutation or execute a completed batch.
    Execute(OwnedRequest),
    /// Append a batchable command to the dispatcher's current collector.
    Collect(OwnedRequest),
    /// Handle a response-bearing query or diagnostic without exclusive ownership.
    Bypass(OwnedRequest),
    /// Handle a sequence-zero heartbeat without caching or responding.
    NoResponse(OwnedRequest),
    /// Urgently cancel and release input state; response completion remains token-bound.
    Stop(OwnedRequest),
    /// Replay an exact final response from the current-session cache.
    Replay(CachedResponse),
    /// Retryable interim refusal that is never cached as final.
    Busy(BusyReason),
    /// Response produced and completed synchronously by the coordinator.
    Immediate(CachedResponse),
    /// Permanent validation or lifecycle rejection.
    Reject(ErrorCode),
}

/// Failure to complete an accepted request safely.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionError {
    /// The token no longer identifies an active cache entry.
    StaleCompletion,
    /// The response sequence does not match the accepted request.
    ResponseSequenceMismatch,
    /// The response version does not match the accepted request.
    ResponseVersionMismatch,
    /// The response is interim, malformed, or not a supported final response type.
    NonFinalResponse,
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
    Executing { owner: CompletionToken },
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
    token: CompletionToken,
    state: CacheState,
}

pub struct Coordinator {
    cache: Vec<CacheEntry, RESPONSE_CACHE_SIZE>,
    next_evict: usize,
    session_generation: u64,
    next_admission_id: u64,
    executor_owner: Option<CompletionToken>,
    batch_state: BatchState,
}

impl Coordinator {
    pub const fn new() -> Self {
        Self {
            cache: Vec::new(),
            next_evict: 0,
            session_generation: 0,
            next_admission_id: 1,
            executor_owner: None,
            batch_state: BatchState::Idle,
        }
    }

    pub fn has_cached_sequence(&self, sequence: u16) -> bool {
        self.cache.iter().any(|entry| entry.sequence == sequence)
    }

    /// Reports whether an ordinary mutation or batch execution owns exclusive dispatch.
    pub const fn is_executor_occupied(&self) -> bool {
        self.executor_owner.is_some() || matches!(self.batch_state, BatchState::Executing { .. })
    }

    pub const fn batch_mode(&self) -> BatchMode {
        match self.batch_state {
            BatchState::Idle => BatchMode::Idle,
            BatchState::Collecting => BatchMode::Collecting,
            BatchState::Executing { .. } => BatchMode::Executing,
        }
    }

    /// Starts a fresh session and invalidates all outstanding completion tokens.
    pub fn clear_session(&mut self) {
        self.session_generation = self
            .session_generation
            .checked_add(1)
            .expect("session generation exhausted");
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
            completion_token: None,
            body,
        };

        match request_class {
            RequestClass::BatchBegin => self.admit_batch_begin(request),
            RequestClass::BatchEnd => self.admit_batch_end(request),
            RequestClass::StopAll => self.admit_stop(request),
            RequestClass::Bypass => self.admit_bypass(request),
            RequestClass::Mutating => self.admit_mutating(request),
        }
    }

    /// Stores a validated final response for the exact active admission token.
    pub fn complete(
        &mut self,
        token: CompletionToken,
        response: CachedResponse,
    ) -> Result<(), CompletionError> {
        if response.sequence != token.sequence {
            return Err(CompletionError::ResponseSequenceMismatch);
        }
        if response.version != token.version {
            return Err(CompletionError::ResponseVersionMismatch);
        }
        if !is_final_response(&response) {
            return Err(CompletionError::NonFinalResponse);
        }
        let entry = self
            .cache
            .iter_mut()
            .find(|entry| entry.token == token && entry.state == CacheState::Active)
            .ok_or(CompletionError::StaleCompletion)?;
        entry.state = CacheState::Completed(response);
        if self.executor_owner == Some(token) {
            self.executor_owner = None;
        }
        if matches!(
            self.batch_state,
            BatchState::Executing { owner } if owner == token
        ) {
            self.batch_state = BatchState::Idle;
        }
        Ok(())
    }

    fn admit_batch_begin(&mut self, mut request: OwnedRequest) -> Admission {
        if self.batch_state != BatchState::Idle {
            return Admission::Reject(ErrorCode::BatchState);
        }
        if self.executor_owner.is_some() {
            return Admission::Busy(BusyReason::ExecutorOccupied);
        }
        if !self.insert_active(&mut request, false) {
            return Admission::Busy(BusyReason::ExecutorOccupied);
        }

        self.batch_state = BatchState::Collecting;
        let token = request
            .completion_token
            .expect("response-bearing begin has a completion token");
        let response = token.ack();
        let completed = self.complete(token, response.clone());
        debug_assert_eq!(completed, Ok(()));
        Admission::Immediate(response)
    }

    fn admit_batch_end(&mut self, mut request: OwnedRequest) -> Admission {
        if self.batch_state != BatchState::Collecting {
            return Admission::Reject(ErrorCode::BatchState);
        }
        if !self.insert_active(&mut request, false) {
            return Admission::Busy(BusyReason::ExecutorOccupied);
        }

        let owner = request
            .completion_token
            .expect("response-bearing batch end has a completion token");
        self.batch_state = BatchState::Executing { owner };
        Admission::Execute(request)
    }

    fn admit_bypass(&mut self, mut request: OwnedRequest) -> Admission {
        if request.request_kind == RequestKind::NoResponseHeartbeat {
            return Admission::NoResponse(request);
        }
        if !self.insert_active(&mut request, false) {
            return Admission::Busy(BusyReason::ExecutorOccupied);
        }
        Admission::Bypass(request)
    }

    fn admit_stop(&mut self, mut request: OwnedRequest) -> Admission {
        if !self.insert_active(&mut request, true) {
            return Admission::Busy(BusyReason::ExecutorOccupied);
        }
        if self.batch_state == BatchState::Collecting {
            self.batch_state = BatchState::Idle;
        }
        Admission::Stop(request)
    }

    fn admit_mutating(&mut self, mut request: OwnedRequest) -> Admission {
        match self.batch_state {
            BatchState::Executing { .. } => Admission::Busy(BusyReason::BatchExecuting),
            BatchState::Collecting => {
                if !self.insert_active(&mut request, false) {
                    return Admission::Busy(BusyReason::ExecutorOccupied);
                }
                Admission::Collect(request)
            }
            BatchState::Idle if self.executor_owner.is_some() => {
                Admission::Busy(BusyReason::ExecutorOccupied)
            }
            BatchState::Idle => {
                if !self.insert_active(&mut request, false) {
                    return Admission::Busy(BusyReason::ExecutorOccupied);
                }
                self.executor_owner = request.completion_token;
                Admission::Execute(request)
            }
        }
    }

    fn insert_active(&mut self, request: &mut OwnedRequest, urgent: bool) -> bool {
        let active_entries = self
            .cache
            .iter()
            .filter(|entry| entry.state == CacheState::Active)
            .count();
        if !urgent && active_entries >= RESPONSE_CACHE_SIZE - 1 {
            return false;
        }

        let replacement_index = if self.cache.len() < RESPONSE_CACHE_SIZE {
            None
        } else {
            let mut completed_index = None;
            for offset in 0..RESPONSE_CACHE_SIZE {
                let index = (self.next_evict + offset) % RESPONSE_CACHE_SIZE;
                if matches!(self.cache[index].state, CacheState::Completed(_)) {
                    completed_index = Some(index);
                    break;
                }
            }
            let Some(index) = completed_index else {
                return false;
            };
            Some(index)
        };

        let Some(token) = self.next_completion_token(request) else {
            return false;
        };
        let entry = CacheEntry {
            sequence: request.sequence,
            fingerprint: request.fingerprint,
            token,
            state: CacheState::Active,
        };

        if let Some(index) = replacement_index {
            self.cache[index] = entry;
            self.next_evict = (index + 1) % RESPONSE_CACHE_SIZE;
        } else if self.cache.push(entry).is_err() {
            return false;
        }
        request.completion_token = Some(token);
        true
    }

    fn next_completion_token(&mut self, request: &OwnedRequest) -> Option<CompletionToken> {
        let admission_id = self.next_admission_id;
        self.next_admission_id = admission_id.checked_add(1)?;
        Some(CompletionToken {
            session_generation: self.session_generation,
            admission_id,
            version: request.version,
            sequence: request.sequence,
            fingerprint: request.fingerprint,
        })
    }
}

fn is_final_response(response: &CachedResponse) -> bool {
    match response.command_type {
        CommandType::Ack => response.payload.is_empty(),
        CommandType::Nack => response.payload.len() == 1,
        CommandType::Status => true,
        _ => false,
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

    fn response_for(
        token: CompletionToken,
        command_type: CommandType,
        payload: &[u8],
    ) -> CachedResponse {
        let mut owned_payload = Vec::<u8, 16>::new();
        owned_payload.extend_from_slice(payload).unwrap();
        CachedResponse {
            version: token.version(),
            sequence: token.sequence(),
            command_type,
            payload: owned_payload,
        }
    }

    fn response_token(admission: Admission) -> CompletionToken {
        let request = match admission {
            Admission::Execute(request)
            | Admission::Collect(request)
            | Admission::Bypass(request)
            | Admission::Stop(request) => request,
            other => panic!("expected response-bearing admission, got {other:?}"),
        };
        request
            .completion_token()
            .expect("response-bearing admission must carry a token")
    }

    fn unadmitted_request(frame: &Frame<'_>) -> OwnedRequest {
        OwnedRequest {
            version: frame.version,
            flags: frame.flags,
            sequence: frame.sequence,
            command_type: frame.command_type,
            fingerprint: fingerprint(frame),
            request_kind: validate_request(frame).unwrap(),
            completion_token: None,
            body: own_request_body(frame).unwrap(),
        }
    }

    #[test]
    fn active_duplicate_is_busy_then_completed_duplicate_replays() {
        let request = request(2, 41, CommandType::MouseClick, &[1]);
        let mut coordinator = Coordinator::new();
        let token = response_token(coordinator.admit(&request));
        assert!(matches!(
            coordinator.admit(&request),
            Admission::Busy(BusyReason::ActiveDuplicate)
        ));
        coordinator.complete(token, token.ack()).unwrap();
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

        assert_eq!(owned.version(), 2);
        assert_eq!(owned.flags(), 0);
        assert_eq!(owned.sequence(), 17);
        assert_eq!(owned.command_type(), CommandType::TypeAscii);
        assert_eq!(owned.fingerprint(), expected_fingerprint);
        assert_eq!(owned.request_kind(), RequestKind::ResponseExpected);
        assert!(owned.completion_token().is_some());
        assert_eq!(
            owned.body(),
            &OwnedRequestBody::Command(OwnedCommand::type_ascii(b"abc").unwrap())
        );
    }

    #[test]
    fn completion_rejects_response_mismatch_and_stale_token() {
        let mut coordinator = Coordinator::new();
        let frame = request(2, 21, CommandType::MouseClick, &[1]);
        let token = response_token(coordinator.admit(&frame));

        assert_eq!(
            coordinator.complete(token, CachedResponse::ack(2, 22)),
            Err(CompletionError::ResponseSequenceMismatch)
        );
        assert_eq!(
            coordinator.admit(&frame),
            Admission::Busy(BusyReason::ActiveDuplicate)
        );
        coordinator
            .complete(token, token.nack(ErrorCode::Cancelled))
            .unwrap();
        assert_eq!(
            coordinator.complete(token, token.ack()),
            Err(CompletionError::StaleCompletion)
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
            let token = response_token(coordinator.admit(&frame));
            coordinator.complete(token, token.ack()).unwrap();
        }
        assert!(coordinator.has_cached_sequence(1));

        let newest = request(2, 65, CommandType::Ping, &[]);
        assert!(matches!(coordinator.admit(&newest), Admission::Bypass(_)));
        assert!(!coordinator.has_cached_sequence(1));
        assert!(coordinator.has_cached_sequence(65));

        let reused = request(2, 1, CommandType::MouseClick, &[1]);
        assert!(matches!(coordinator.admit(&reused), Admission::Execute(_)));
        assert!(coordinator.is_executor_occupied());

        coordinator.clear_session();
        assert!(!coordinator.has_cached_sequence(1));
        assert!(!coordinator.has_cached_sequence(65));
        assert!(!coordinator.is_executor_occupied());
        assert!(matches!(coordinator.admit(&newest), Admission::Bypass(_)));
    }

    #[test]
    fn cache_churn_never_evicts_active_entry_when_completed_entry_exists() {
        let mut coordinator = Coordinator::new();
        let active = request(2, 1, CommandType::Ping, &[]);
        assert!(matches!(coordinator.admit(&active), Admission::Bypass(_)));

        for sequence in 2..=RESPONSE_CACHE_SIZE as u16 {
            let frame = request(2, sequence, CommandType::GetInfo, &[]);
            let token = response_token(coordinator.admit(&frame));
            coordinator.complete(token, token.ack()).unwrap();
        }

        let churn = request(2, 65, CommandType::GetCaps, &[]);
        assert!(matches!(coordinator.admit(&churn), Admission::Bypass(_)));
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

        let owner_token = response_token(coordinator.admit(&owner));
        assert!(coordinator.is_executor_occupied());
        assert_eq!(
            coordinator.admit(&waiting),
            Admission::Busy(BusyReason::ExecutorOccupied)
        );
        assert!(!coordinator.has_cached_sequence(32));

        coordinator
            .complete(owner_token, owner_token.ack())
            .unwrap();
        assert!(!coordinator.is_executor_occupied());
        assert!(matches!(coordinator.admit(&waiting), Admission::Execute(_)));
        assert!(coordinator.has_cached_sequence(32));
    }

    #[test]
    fn occupied_executor_allows_every_bypass_without_releasing_owner() {
        let mut coordinator = Coordinator::new();
        let owner = request(2, 40, CommandType::WaitMs, &[0, 0, 0, 1]);
        let owner_token = response_token(coordinator.admit(&owner));

        for (sequence, command_type) in [
            (41, CommandType::Ping),
            (42, CommandType::GetInfo),
            (43, CommandType::GetCaps),
            (44, CommandType::Heartbeat),
            (45, CommandType::StopAll),
        ] {
            let bypass = request(2, sequence, command_type, &[]);
            let token = response_token(coordinator.admit(&bypass));
            coordinator.complete(token, token.ack()).unwrap();
            assert!(coordinator.is_executor_occupied());

            let ordinary = request(2, sequence + 100, CommandType::MouseClick, &[1]);
            assert_eq!(
                coordinator.admit(&ordinary),
                Admission::Busy(BusyReason::ExecutorOccupied)
            );
            assert!(!coordinator.has_cached_sequence(sequence + 100));
        }

        coordinator
            .complete(owner_token, owner_token.ack())
            .unwrap();
        assert!(!coordinator.is_executor_occupied());
    }

    #[test]
    fn no_response_heartbeat_bypasses_cache_but_diagnostic_heartbeat_replays() {
        let mut coordinator = Coordinator::new();
        let no_response = request_with_flags(2, FLAG_NO_RESPONSE, 0, CommandType::Heartbeat, &[]);

        for _ in 0..2 {
            let Admission::NoResponse(owned) = coordinator.admit(&no_response) else {
                panic!("no-response heartbeat should dispatch without a response entry");
            };
            assert_eq!(owned.request_kind(), RequestKind::NoResponseHeartbeat);
            assert_eq!(owned.sequence(), 0);
            assert_eq!(owned.completion_token(), None);
            assert!(!coordinator.has_cached_sequence(0));
        }

        let diagnostic = request(2, 46, CommandType::Heartbeat, &[]);
        let token = response_token(coordinator.admit(&diagnostic));
        assert!(coordinator.has_cached_sequence(46));
        coordinator.complete(token, token.ack()).unwrap();
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
            let token = owned.completion_token().unwrap();
            assert_eq!(owned.sequence(), *sequence);
            assert_eq!(owned.command_type(), *command_type);
            assert!(matches!(owned.body(), OwnedRequestBody::Command(_)));

            if index == 0 {
                assert_eq!(
                    coordinator.admit(&frame),
                    Admission::Busy(BusyReason::ActiveDuplicate)
                );
            }
            coordinator.complete(token, token.ack()).unwrap();
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
        let end_token = owned_end.completion_token().unwrap();
        assert_eq!(owned_end.body(), &OwnedRequestBody::BatchEnd);
        assert_eq!(coordinator.batch_mode(), BatchMode::Executing);

        let ordinary = request(2, 102, CommandType::MouseClick, &[1]);
        assert_eq!(
            coordinator.admit(&ordinary),
            Admission::Busy(BusyReason::BatchExecuting)
        );
        assert!(!coordinator.has_cached_sequence(102));

        let query = request(2, 103, CommandType::GetInfo, &[]);
        let query_token = response_token(coordinator.admit(&query));
        coordinator
            .complete(query_token, query_token.ack())
            .unwrap();
        assert_eq!(coordinator.batch_mode(), BatchMode::Executing);
        assert_eq!(
            coordinator.complete(end_token, CachedResponse::ack(2, 999)),
            Err(CompletionError::ResponseSequenceMismatch)
        );
        assert_eq!(coordinator.batch_mode(), BatchMode::Executing);

        coordinator.complete(end_token, end_token.ack()).unwrap();
        assert_eq!(coordinator.batch_mode(), BatchMode::Idle);
        assert_eq!(
            coordinator.admit(&end),
            Admission::Replay(CachedResponse::ack(2, 101))
        );
    }

    #[test]
    fn batch_execution_allows_every_bypass_and_owner_completion_clears_mode() {
        let mut coordinator = Coordinator::new();
        begin_batch(&mut coordinator, 110);
        let end = request(2, 111, CommandType::BatchEnd, &[]);
        let end_token = response_token(coordinator.admit(&end));

        for (sequence, command_type) in [
            (112, CommandType::Ping),
            (113, CommandType::GetInfo),
            (114, CommandType::GetCaps),
            (115, CommandType::Heartbeat),
            (116, CommandType::StopAll),
        ] {
            let bypass = request(2, sequence, command_type, &[]);
            let token = response_token(coordinator.admit(&bypass));
            coordinator.complete(token, token.ack()).unwrap();
            assert_eq!(coordinator.batch_mode(), BatchMode::Executing);
        }

        coordinator
            .complete(end_token, end_token.nack(ErrorCode::Cancelled))
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
            let token = response_token(coordinator.admit(&bypass));
            coordinator.complete(token, token.ack()).unwrap();
            assert_eq!(coordinator.batch_mode(), BatchMode::Collecting);
        }

        let stop = request(2, 125, CommandType::StopAll, &[]);
        let stop_token = response_token(coordinator.admit(&stop));
        assert_eq!(coordinator.batch_mode(), BatchMode::Idle);
        coordinator.complete(stop_token, stop_token.ack()).unwrap();

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
        let owner_token = response_token(coordinator.admit(&owner));
        assert_eq!(
            coordinator.admit(&begin),
            Admission::Busy(BusyReason::ExecutorOccupied)
        );
        assert!(!coordinator.has_cached_sequence(131));

        coordinator
            .complete(owner_token, owner_token.ack())
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
            assert!(matches!(coordinator.admit(&query), Admission::Bypass(_)));
        }

        let saturated_query = request(2, 263, CommandType::GetInfo, &[]);
        assert_eq!(
            coordinator.admit(&saturated_query),
            Admission::Busy(BusyReason::ExecutorOccupied)
        );
        assert!(!coordinator.has_cached_sequence(263));

        let stop = request(2, 264, CommandType::StopAll, &[]);
        assert!(matches!(coordinator.admit(&stop), Admission::Stop(_)));
        assert!(coordinator.has_cached_sequence(264));
        assert!(coordinator.has_cached_sequence(200));
        assert!(coordinator.is_executor_occupied());
    }

    #[test]
    fn completed_entry_replays_exact_response_and_still_rejects_sequence_conflict() {
        let mut coordinator = Coordinator::new();
        let original = request(2, 300, CommandType::MouseMoveRel, &[0, 1, 0, 1]);
        let conflicting = request(2, 300, CommandType::MouseMoveRel, &[0, 2, 0, 1]);
        let token = response_token(coordinator.admit(&original));

        let mut payload = Vec::<u8, 16>::new();
        payload.extend_from_slice(b"done").unwrap();
        let response = CachedResponse {
            version: 2,
            sequence: 300,
            command_type: CommandType::Status,
            payload,
        };
        coordinator.complete(token, response.clone()).unwrap();

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

    #[test]
    fn mixed_cache_saturation_reserves_capacity_for_stop_all() {
        let mut coordinator = Coordinator::new();
        begin_batch(&mut coordinator, 1);

        for sequence in 2..=63 {
            let query = request(2, sequence, CommandType::Ping, &[]);
            assert!(matches!(coordinator.admit(&query), Admission::Bypass(_)));
        }

        let last_normal = request(2, 64, CommandType::Ping, &[]);
        assert!(matches!(
            coordinator.admit(&last_normal),
            Admission::Bypass(_)
        ));

        let overflow = request(2, 65, CommandType::Ping, &[]);
        assert_eq!(
            coordinator.admit(&overflow),
            Admission::Busy(BusyReason::ExecutorOccupied)
        );
        assert!(!coordinator.has_cached_sequence(65));
        assert_eq!(coordinator.batch_mode(), BatchMode::Collecting);

        let stop = request(2, 66, CommandType::StopAll, &[]);
        assert!(matches!(coordinator.admit(&stop), Admission::Stop(_)));
        assert!(coordinator.has_cached_sequence(66));
        assert!(coordinator.has_cached_sequence(2));
        assert!(coordinator.has_cached_sequence(63));
        assert!(coordinator.has_cached_sequence(64));
        assert_eq!(coordinator.batch_mode(), BatchMode::Idle);
    }

    #[test]
    fn failed_urgent_stop_admission_preserves_collection_state() {
        let mut coordinator = Coordinator::new();
        coordinator.batch_state = BatchState::Collecting;
        for sequence in 1..=RESPONSE_CACHE_SIZE as u16 {
            let frame = request(2, sequence, CommandType::Ping, &[]);
            let mut request = unadmitted_request(&frame);
            assert!(coordinator.insert_active(&mut request, true));
        }

        let stop = request(2, 100, CommandType::StopAll, &[]);
        assert_eq!(
            coordinator.admit(&stop),
            Admission::Busy(BusyReason::ExecutorOccupied)
        );
        assert!(!coordinator.has_cached_sequence(100));
        assert_eq!(coordinator.batch_mode(), BatchMode::Collecting);
    }

    #[test]
    fn late_old_session_completion_cannot_complete_reused_sequence() {
        let frame = request(2, 400, CommandType::MouseClick, &[1]);
        let mut coordinator = Coordinator::new();
        let Admission::Execute(old_request) = coordinator.admit(&frame) else {
            panic!("ordinary mutation should execute");
        };
        let old_token = old_request.completion_token().unwrap();

        coordinator.clear_session();
        let Admission::Execute(new_request) = coordinator.admit(&frame) else {
            panic!("same sequence should be reusable in a new session");
        };
        let new_token = new_request.completion_token().unwrap();
        assert_ne!(old_token, new_token);
        assert_ne!(
            old_token.session_generation(),
            new_token.session_generation()
        );

        assert_eq!(
            coordinator.complete(old_token, old_token.ack()),
            Err(CompletionError::StaleCompletion)
        );
        assert!(coordinator.is_executor_occupied());
        assert_eq!(
            coordinator.admit(&frame),
            Admission::Busy(BusyReason::ActiveDuplicate)
        );

        coordinator.complete(new_token, new_token.ack()).unwrap();
        assert_eq!(
            coordinator.admit(&frame),
            Admission::Replay(new_token.ack())
        );
    }

    #[test]
    fn evicted_admission_token_is_stale_after_same_session_sequence_reuse() {
        let mut coordinator = Coordinator::new();
        let original = request(2, 401, CommandType::Ping, &[]);
        let Admission::Bypass(old_request) = coordinator.admit(&original) else {
            panic!("ping should use bypass dispatch");
        };
        let old_token = old_request.completion_token().unwrap();
        coordinator.complete(old_token, old_token.ack()).unwrap();

        for sequence in 402..=465 {
            let frame = request(2, sequence, CommandType::Ping, &[]);
            let Admission::Bypass(request) = coordinator.admit(&frame) else {
                panic!("query should use bypass dispatch");
            };
            let token = request.completion_token().unwrap();
            coordinator.complete(token, token.ack()).unwrap();
        }
        assert!(!coordinator.has_cached_sequence(401));

        let Admission::Bypass(new_request) = coordinator.admit(&original) else {
            panic!("evicted sequence should be admitted again");
        };
        let new_token = new_request.completion_token().unwrap();
        assert_eq!(
            old_token.session_generation(),
            new_token.session_generation()
        );
        assert_ne!(old_token.admission_id(), new_token.admission_id());
        assert_eq!(old_token.fingerprint(), new_token.fingerprint());

        assert_eq!(
            coordinator.complete(old_token, old_token.ack()),
            Err(CompletionError::StaleCompletion)
        );
        assert_eq!(
            coordinator.admit(&original),
            Admission::Busy(BusyReason::ActiveDuplicate)
        );
        coordinator.complete(new_token, new_token.ack()).unwrap();
    }

    #[test]
    fn completion_validates_version_and_final_response_shape_before_mutation() {
        let mut coordinator = Coordinator::new();
        let frame = request(2, 470, CommandType::MouseClick, &[1]);
        let Admission::Execute(request) = coordinator.admit(&frame) else {
            panic!("mouse click should execute exclusively");
        };
        let token = request.completion_token().unwrap();

        assert_eq!(
            coordinator.complete(token, CachedResponse::ack(1, token.sequence())),
            Err(CompletionError::ResponseVersionMismatch)
        );
        assert_eq!(
            coordinator.complete(
                token,
                CachedResponse::busy(
                    token.version(),
                    token.sequence(),
                    BusyReason::ExecutorOccupied,
                    1,
                ),
            ),
            Err(CompletionError::NonFinalResponse)
        );
        for malformed in [
            response_for(token, CommandType::Ack, &[1]),
            response_for(token, CommandType::Nack, &[]),
            response_for(token, CommandType::Nack, &[1, 2]),
            response_for(token, CommandType::Ping, &[]),
        ] {
            assert_eq!(
                coordinator.complete(token, malformed),
                Err(CompletionError::NonFinalResponse)
            );
        }
        assert!(coordinator.is_executor_occupied());
        assert_eq!(
            coordinator.admit(&frame),
            Admission::Busy(BusyReason::ActiveDuplicate)
        );

        let status = response_for(token, CommandType::Status, b"done");
        coordinator.complete(token, status.clone()).unwrap();
        assert!(!coordinator.is_executor_occupied());
        assert_eq!(coordinator.admit(&frame), Admission::Replay(status));
    }

    #[test]
    fn admission_variants_expose_dispatch_mode_and_token_contract() {
        let mut ordinary_coordinator = Coordinator::new();
        let ordinary = request(2, 480, CommandType::MouseClick, &[1]);
        let Admission::Execute(ordinary_request) = ordinary_coordinator.admit(&ordinary) else {
            panic!("ordinary mutation should use exclusive execution");
        };
        assert_eq!(ordinary_request.version(), 2);
        assert_eq!(ordinary_request.flags(), 0);
        assert_eq!(ordinary_request.sequence(), 480);
        assert_eq!(ordinary_request.command_type(), CommandType::MouseClick);
        assert_eq!(
            ordinary_request.request_kind(),
            RequestKind::ResponseExpected
        );
        assert!(ordinary_request.completion_token().is_some());
        assert!(matches!(
            ordinary_request.body(),
            OwnedRequestBody::Command(OwnedCommand::MouseClick(_))
        ));

        let mut collect_coordinator = Coordinator::new();
        begin_batch(&mut collect_coordinator, 481);
        let collected = request(2, 482, CommandType::WaitMs, &[0, 0, 0, 1]);
        let Admission::Collect(collected_request) = collect_coordinator.admit(&collected) else {
            panic!("batchable mutation should use collection dispatch");
        };
        assert!(collected_request.completion_token().is_some());

        let mut end_coordinator = Coordinator::new();
        begin_batch(&mut end_coordinator, 483);
        let end = request(2, 484, CommandType::BatchEnd, &[]);
        let Admission::Execute(end_request) = end_coordinator.admit(&end) else {
            panic!("batch end should use exclusive execution");
        };
        assert_eq!(end_request.body(), &OwnedRequestBody::BatchEnd);
        assert!(end_request.completion_token().is_some());

        for command_type in [
            CommandType::Ping,
            CommandType::GetInfo,
            CommandType::GetCaps,
            CommandType::Heartbeat,
        ] {
            let mut coordinator = Coordinator::new();
            let frame = request(2, 485, command_type, &[]);
            let Admission::Bypass(request) = coordinator.admit(&frame) else {
                panic!("read-only and diagnostic commands should bypass");
            };
            assert!(request.completion_token().is_some());
        }

        let mut heartbeat_coordinator = Coordinator::new();
        let heartbeat = request_with_flags(2, FLAG_NO_RESPONSE, 0, CommandType::Heartbeat, &[]);
        let Admission::NoResponse(heartbeat_request) = heartbeat_coordinator.admit(&heartbeat)
        else {
            panic!("sequence-zero heartbeat should use no-response dispatch");
        };
        assert_eq!(heartbeat_request.completion_token(), None);

        let mut stop_coordinator = Coordinator::new();
        let stop = request(2, 486, CommandType::StopAll, &[]);
        let Admission::Stop(stop_request) = stop_coordinator.admit(&stop) else {
            panic!("stop should use urgent cancellation dispatch");
        };
        assert!(stop_request.completion_token().is_some());
        assert_eq!(
            stop_request.into_body(),
            OwnedRequestBody::Command(OwnedCommand::StopAll)
        );
    }

    #[test]
    fn executor_query_includes_batch_but_excludes_bypass_only_activity() {
        let mut bypass_coordinator = Coordinator::new();
        let ping = request(2, 490, CommandType::Ping, &[]);
        assert!(matches!(
            bypass_coordinator.admit(&ping),
            Admission::Bypass(_)
        ));
        assert!(!bypass_coordinator.is_executor_occupied());

        let mut ordinary_coordinator = Coordinator::new();
        let ordinary = request(2, 491, CommandType::WaitMs, &[0, 0, 0, 1]);
        assert!(matches!(
            ordinary_coordinator.admit(&ordinary),
            Admission::Execute(_)
        ));
        assert!(ordinary_coordinator.is_executor_occupied());

        let mut batch_coordinator = Coordinator::new();
        begin_batch(&mut batch_coordinator, 492);
        let end = request(2, 493, CommandType::BatchEnd, &[]);
        assert!(matches!(
            batch_coordinator.admit(&end),
            Admission::Execute(_)
        ));
        assert!(batch_coordinator.is_executor_occupied());
    }

    #[test]
    fn stop_during_batch_execution_waits_for_exact_end_completion() {
        let mut coordinator = Coordinator::new();
        begin_batch(&mut coordinator, 500);
        let end = request(2, 501, CommandType::BatchEnd, &[]);
        let Admission::Execute(end_request) = coordinator.admit(&end) else {
            panic!("batch end should execute");
        };
        let end_token = end_request.completion_token().unwrap();

        let stop = request(2, 502, CommandType::StopAll, &[]);
        let Admission::Stop(stop_request) = coordinator.admit(&stop) else {
            panic!("stop should bypass active batch execution");
        };
        let stop_token = stop_request.completion_token().unwrap();
        assert_eq!(coordinator.batch_mode(), BatchMode::Executing);
        assert!(coordinator.is_executor_occupied());

        coordinator.complete(stop_token, stop_token.ack()).unwrap();
        assert_eq!(coordinator.batch_mode(), BatchMode::Executing);
        assert_eq!(
            coordinator.complete(stop_token, stop_token.nack(ErrorCode::Cancelled)),
            Err(CompletionError::StaleCompletion)
        );
        assert_eq!(coordinator.batch_mode(), BatchMode::Executing);

        coordinator
            .complete(end_token, end_token.nack(ErrorCode::Cancelled))
            .unwrap();
        assert_eq!(coordinator.batch_mode(), BatchMode::Idle);
        assert!(!coordinator.is_executor_occupied());
    }

    #[test]
    fn late_old_batch_end_token_cannot_clear_new_session_batch_owner() {
        let mut coordinator = Coordinator::new();
        begin_batch(&mut coordinator, 510);
        let old_end = request(2, 511, CommandType::BatchEnd, &[]);
        let Admission::Execute(old_request) = coordinator.admit(&old_end) else {
            panic!("old batch end should execute");
        };
        let old_token = old_request.completion_token().unwrap();

        coordinator.clear_session();
        begin_batch(&mut coordinator, 510);
        let new_end = request(2, 511, CommandType::BatchEnd, &[]);
        let Admission::Execute(new_request) = coordinator.admit(&new_end) else {
            panic!("new batch end should execute");
        };
        let new_token = new_request.completion_token().unwrap();
        assert_ne!(old_token, new_token);

        assert_eq!(
            coordinator.complete(old_token, old_token.nack(ErrorCode::Cancelled)),
            Err(CompletionError::StaleCompletion)
        );
        assert_eq!(coordinator.batch_mode(), BatchMode::Executing);
        assert!(coordinator.is_executor_occupied());

        coordinator
            .complete(new_token, new_token.nack(ErrorCode::Cancelled))
            .unwrap();
        assert_eq!(coordinator.batch_mode(), BatchMode::Idle);
    }
}
