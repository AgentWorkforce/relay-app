//! Local service callers (`docs/service-callers.md`): the management methods
//! the signed-in person uses, and the result-inbox methods a caller's
//! runtime uses with its name and secret.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Shows whether a `caller_secret` was given, never its value.
struct RedactedSecret<'a>(&'a Option<String>);

impl fmt::Debug for RedactedSecret<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(_) => f.write_str("\"<redacted>\""),
            None => f.write_str("None"),
        }
    }
}

/// A caller's publisher identity: its Relaycast agent.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct CallerPublisher {
    pub id: String,
    pub name: String,
}

/// Where a caller stands. A `pending_*` caller waits for the person's
/// decision in Settings (`spawn_callers.decide`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(extend("x-fallback" = "disabled"))]
pub enum CallerState {
    /// Asked for by the probe; nothing registered, no secret, no file.
    PendingProvision,
    /// A rotation asked for by the probe; the current secret still works.
    PendingRotation,
    Enabled,
    Disabled,
    Expired,
}

/// Who asked for a caller's newest provision or rotation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(extend("x-fallback" = "probe"))]
pub enum CallerRequester {
    App,
    Probe,
}

/// One caller as the person sees it. No secret, hash or token.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct CallerSummary {
    pub caller_id: String,
    pub name: String,
    /// The publisher identity; its `id` is empty until a pending provision
    /// is approved.
    pub publisher: CallerPublisher,
    pub harnesses: Vec<String>,
    /// The resolved folder roots.
    pub directory_roots: Vec<String>,
    pub owns_only: bool,
    pub enabled: bool,
    pub state: CallerState,
    pub requested_by: CallerRequester,
    /// RFC 3339.
    pub requested_at: String,
    /// RFC 3339.
    pub created_at: String,
    pub rotated_at: Option<String>,
    pub expires_at: String,
    /// How many secrets are accepted now (none while a provision is
    /// pending, two during an overlap).
    pub secrets_live: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct CallerList {
    pub callers: Vec<CallerSummary>,
}

/// Management params carry `caller_secret` only so that its presence is
/// refused (`management_peer`); it is never read as proof.
#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallerListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_secret: Option<String>,
}

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallerProvisionParams {
    pub name: String,
    pub harnesses: Vec<String>,
    pub directory_roots: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_secret: Option<String>,
}

/// The caller, and where its `caller.json` was written. A provision that
/// waits for the person returns the caller alone: no path, secret or
/// descriptor; its `caller_id` is the key `spawn_callers.decide` takes.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct CallerProvisioned {
    pub caller: CallerSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff_path: Option<String>,
}

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallerRotateParams {
    pub caller_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlap_seconds: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_secret: Option<String>,
}

/// A rotation. While it is pending it returns no path, and
/// `old_secret_retires_at` is null.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct CallerRotated {
    pub caller: CallerSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff_path: Option<String>,
    pub old_secret_retires_at: Option<String>,
}

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallerEnabledParams {
    pub caller_id: String,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_secret: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct CallerChanged {
    pub caller: CallerSummary,
}

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallerIdParams {
    pub caller_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_secret: Option<String>,
}

/// The person's decision on a pending provision or rotation.
#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallerDecideParams {
    pub caller_id: String,
    pub approved: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_secret: Option<String>,
}

/// The person's decision. An approval returns where Desktop wrote
/// `caller.json`, to the app alone; a rejection returns no path.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct CallerDecided {
    pub caller: CallerSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff_path: Option<String>,
}

/// The `message` of every service-caller error: one class per rule, never
/// any text of the request. `data.kind` and `data.retryable` come with it,
/// and `data.reason` where the core's `RefusalReason` names the rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(extend("x-fallback" = "other"))]
pub enum CallerErrorClass {
    /// No such caller, or a wrong secret; the two read alike.
    CallerUnknown,
    CallerDisabled,
    CallerExpired,
    SecretRetired,
    AccountMismatch,
    WorkspaceMismatch,
    GenerationMoved,
    NotInLedger,
    RequesterOther,
    ItemNotOwned,
    HarnessOutside,
    DirectoryOutside,
    PeerUnreadable,
    PeerUidMismatch,
    PeerInSessionTree,
    SecretOnManagement,
    /// This platform cannot check the calling process (Windows).
    PeerCheckUnavailable,
    /// An app-only change from a peer the operating system has not attested
    /// as this app.
    AppAttestationUnavailable,
    NotSignedIn,
    RegistryUnreadable,
    StoreFailed,
    NodeUnavailable,
    NodeOther,
    RateLimited,
    LiveAgentCap,
    ReservationFull,
    /// The item was settled the other way; nothing changed.
    SettlementConflict,
    SettlementInvalid,
    NotFound,
    IteratorOpen,
    NameTaken,
    /// `spawn_callers.decide` on a caller with nothing pending.
    NotPending,
    InvalidInput,
    PublisherRegistrationFailed,
    /// A wrong or absent challenge answer, an unknown caller, an expired,
    /// consumed or foreign challenge, a retired secret or a scope change;
    /// all read alike (`docs/service-callers-auth.md` section 5).
    AuthFailed,
    /// A caller method on a connection that has not authenticated.
    CallerUnbound,
    /// The connection's binding no longer holds: a rotation, revocation,
    /// disable, expiry, account change or `disconnect`.
    BindingStale,
    /// The bridge connection closed while the call was in flight.
    ConnectionClosed,
    /// `challenge` or `authenticate` on a bound or stale connection.
    AlreadyBound,
    /// `challenge` while the connection holds an unexpired challenge.
    ChallengeOutstanding,
    /// Too many challenges outstanding for this caller id, or in total.
    ChallengeCapacity,
    /// Three attempts on this connection ended without a binding.
    AuthLocked,
    /// `challenge` or `authenticate` from a call with no bridge connection.
    BindingUnavailable,
    /// `caller_name` or `caller_secret` on a connection that has
    /// challenged or bound.
    LegacyCredentialRefused,
    /// A management method on a connection that has challenged or bound.
    ExternalConnection,
    /// A rotation could not write `caller.json` nor undo its registry
    /// write; the old record is put back at the next check.
    RotationRollbackPending,
    /// A class this build does not know.
    #[serde(other)]
    Other,
}

/// The proof every caller call carries.
#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallerAuthParams {
    /// The per-request credential. Omitted on a connection bound by
    /// `spawn_callers.authenticate`, which refuses it.
    #[serde(default)]
    pub caller_name: String,
    #[serde(default)]
    pub caller_secret: Option<String>,
}

/// `spawn_callers.challenge`: the caller id from `caller.json`, `sc_` and
/// 16 lowercase hex characters. It is not a credential.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallerChallengeParams {
    pub caller_id: String,
}

/// A challenge bound to the calling connection. The client builds the
/// transcript (`docs/service-callers-auth.md` section 3) from these fields
/// and its own `caller.json`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct CallerChallenge {
    /// Exactly 1: the transcript version byte.
    pub version: u8,
    /// `ch_` and 32 lowercase hex characters.
    pub challenge_id: String,
    /// 32 random bytes, base64url without padding (43 characters).
    pub nonce: String,
    pub issued_at_ms: i64,
    /// `issued_at_ms + 30000`.
    pub expires_at_ms: i64,
}

/// `spawn_callers.authenticate`: the challenge answered on the connection
/// that asked for it.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallerAuthenticateParams {
    /// `ch_` and 32 lowercase hex characters.
    pub challenge_id: String,
    /// HMAC-SHA256 of the transcript, base64url without padding (43
    /// characters).
    pub authenticator: String,
}

/// The caller this connection now acts as.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct CallerBound {
    pub caller_id: String,
    pub caller_name: String,
    pub authority_generation: u64,
}

/// This computer's own spawn node, as `spawn.invoke` accepts it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct CallerNode {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct CallerIdentity {
    pub publisher: CallerPublisher,
    pub node: CallerNode,
    pub base_url: String,
    pub workspace_id: String,
    pub relaycast_workspace_id: String,
}

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InboxOpenParams {
    /// The per-request credential. Omitted on a connection bound by
    /// `spawn_callers.authenticate`, which refuses it.
    #[serde(default)]
    pub caller_name: String,
    #[serde(default)]
    pub caller_secret: Option<String>,
    pub invocation_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct InboxOpened {
    pub iterator_id: String,
}

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InboxIteratorParams {
    /// The per-request credential. Omitted on a connection bound by
    /// `spawn_callers.authenticate`, which refuses it.
    #[serde(default)]
    pub caller_name: String,
    #[serde(default)]
    pub caller_secret: Option<String>,
    pub iterator_id: String,
}

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InboxNextParams {
    /// The per-request credential. Omitted on a connection bound by
    /// `spawn_callers.authenticate`, which refuses it.
    #[serde(default)]
    pub caller_name: String,
    #[serde(default)]
    pub caller_secret: Option<String>,
    pub iterator_id: String,
    /// At most 30000.
    pub timeout_ms: u64,
}

/// The sender of a result: the spawned agent.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct InboxSender {
    pub id: String,
    pub name: String,
}

/// The typed code of a terminal failure delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(extend("x-fallback" = "result_write_failed"))]
pub enum InboxFailureCode {
    /// The report was over the invocation's 512 KiB slot.
    ResultTooLarge,
    /// The reserved slot could not be written.
    ResultWriteFailed,
}

/// Why an invocation's terminal result is a failure, not text.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct InboxFailure {
    pub code: InboxFailureCode,
    pub bytes: u64,
    pub limit: u64,
}

/// One invocation's terminal result: `text`, or `failure` and no text.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct InboxDelivery {
    pub item_id: String,
    pub message_id: String,
    pub sender: InboxSender,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<InboxFailure>,
    pub invocation_id: String,
    /// RFC 3339.
    pub sent_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct InboxNext {
    pub delivery: Option<InboxDelivery>,
}

/// Exactly `{"relayflows_result": "journaled"}` (ack) or
/// `{"relayflows_result": "rejected"}` (fail).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InboxSettleMetadata {
    pub relayflows_result: String,
}

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InboxSettleParams {
    /// The per-request credential. Omitted on a connection bound by
    /// `spawn_callers.authenticate`, which refuses it.
    #[serde(default)]
    pub caller_name: String,
    #[serde(default)]
    pub caller_secret: Option<String>,
    pub item_id: String,
    pub metadata: InboxSettleMetadata,
}

/// The method that settled an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InboxSettleAction {
    Ack,
    Fail,
}

/// The item's terminal state after a settlement: `ack` gives `acked`,
/// `fail` gives `failed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InboxSettleState {
    Acked,
    Failed,
}

/// The terminal settlement receipt of `spawn_callers.inbox.ack` and
/// `spawn_callers.inbox.fail`. An identical repeat (same caller, item,
/// action and metadata) returns the same receipt, `settled_at_ms`
/// included, for the item's 7-day tombstone.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InboxSettled {
    pub item_id: String,
    pub message_id: String,
    pub action: InboxSettleAction,
    pub state: InboxSettleState,
    /// Unix epoch milliseconds of the first settlement.
    pub settled_at_ms: i64,
}

macro_rules! redacted_debug {
    ($($ty:ident { $($field:ident),* })*) => {
        $(impl fmt::Debug for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($ty))
                    $(.field(stringify!($field), &self.$field))*
                    .field("caller_secret", &RedactedSecret(&self.caller_secret))
                    .finish()
            }
        })*
    };
}

redacted_debug! {
    CallerListParams {}
    CallerProvisionParams { name, harnesses, directory_roots, handoff_path, expires_in_days }
    CallerRotateParams { caller_id, overlap_seconds, handoff_path }
    CallerEnabledParams { caller_id, enabled }
    CallerIdParams { caller_id }
    CallerDecideParams { caller_id, approved }
    CallerAuthParams { caller_name }
    InboxOpenParams { caller_name, invocation_id }
    InboxIteratorParams { caller_name, iterator_id }
    InboxNextParams { caller_name, iterator_id, timeout_ms }
    InboxSettleParams { caller_name, item_id, metadata }
}
