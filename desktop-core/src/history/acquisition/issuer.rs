//! The issuer: the History service's native acquisition routes. The trait
//! keeps the reconcile testable against a fake; [`HttpIssuer`] is the real
//! client. Only codes are ever logged, never a body, a grant or the proof.
use super::contract::{
    self, ClaimRequest, ClaimResponse, ErrorBody, InstallationRequest, InstallationResponse,
    RebindRequest, RebindResponse, StatusEnvelope, StatusRequest, StatusResponse,
};
use super::store::Secret;
use crate::history::cloud::{self, StoredAuth};
use std::collections::BTreeSet;
use std::io::Read;
use std::time::Duration;

/// Why an issuer call gave no answer to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssuerError {
    /// 409 `acquisition_refused`. Only a `retryable` refusal is retried, and
    /// then with the same operation uuid.
    Refused { reason: String, retryable: bool },
    /// 409 `installation_proof_mismatch`: the registration was replayed
    /// with another proof. Never answered by minting a new proof.
    ProofMismatch,
    /// 403 `installation_not_proven`: no grant material; held sessions stay
    /// held and the proof is not changed.
    NotProven,
    /// 403 `not_authorized`: the bearer check, and only that, on every
    /// native route (claims, rebinds and status included): the signed-in
    /// session lacks `rth:acquire` (or is not a desktop user, or has no
    /// workspace), which the next normal sign-in brings. The only 403 that
    /// asks for sign-in; the service never uses it for a proof or an
    /// ownership refusal, so it is mapped by its code alone.
    NotAuthorized,
    /// 403 `not_owner`: the issuer's owner check refused this one session
    /// (another installation owns it; on a rebind also a grant whose install
    /// was refused or is void, or a rebind without the session's G). Scoped
    /// to that session: it waits on its owner's status like any owner wait,
    /// and no sign-in can cure it.
    NotOwner,
    /// 403 `not_installer`: the grant the operation names was not issued to
    /// this installation (unknown, not an install, or another
    /// installation's), or the service's SQL installer-role guard refused
    /// the call. Scoped to that session, and no sign-in can cure it. A
    /// rebind's grant is quarantined until the scoped status disowns it or
    /// shows it still this installation's; it is not dropped on this alone.
    NotInstaller,
    /// Any other 403 (`target_mismatch`, an unknown code, or no readable
    /// code, then `code` is empty). Fails closed for that session only: it
    /// stays held, without asking for sign-in. `code` is kept only when it
    /// has the shape of a code, never as raw body text.
    Forbidden { code: String },
    /// 400 `invalid_request`, or a status target the service would refuse
    /// as `414 request_uri_too_long`: a client defect, never retried.
    InvalidRequest,
    /// The operation or route is not available (the permanent-revoke stub,
    /// or a service without the native routes).
    Unavailable,
    /// The request did not complete (network, 5xx including every 503, or a
    /// 401 the shared refresh could not cure, which asks for sign-in); try
    /// again later with the same operation. That includes `503
    /// acquisition_unavailable` on a claim whose install has no positive
    /// epoch yet: retryable, so the same operation is replayed.
    Transport(String),
}
impl std::fmt::Display for IssuerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused { reason, retryable } => {
                write!(f, "acquisition refused: {reason} (retryable: {retryable})")
            }
            Self::ProofMismatch => f.write_str(contract::INSTALLATION_PROOF_MISMATCH),
            Self::NotProven => f.write_str(contract::INSTALLATION_NOT_PROVEN),
            Self::NotAuthorized => f.write_str(contract::NOT_AUTHORIZED),
            Self::NotOwner => f.write_str(contract::NOT_OWNER),
            Self::NotInstaller => f.write_str(contract::NOT_INSTALLER),
            Self::Forbidden { code } if code.is_empty() => f.write_str("forbidden"),
            Self::Forbidden { code } => write!(f, "forbidden: {code}"),
            Self::InvalidRequest => f.write_str(contract::INVALID_REQUEST),
            Self::Unavailable => f.write_str("acquisition unavailable"),
            Self::Transport(why) => write!(f, "acquisition request failed: {why}"),
        }
    }
}
impl std::error::Error for IssuerError {}

/// The native acquisition routes. Registration carries the proof in its
/// body ([`contract::PROOF_FIELD`]); every other route carries it in
/// [`contract::PROOF_HEADER`].
pub trait AcquisitionIssuer {
    /// `acquisition.enrolledSources` of the published limits; `None` when
    /// the service does not publish it.
    fn enrolled_sources(&self) -> Result<Option<BTreeSet<String>>, IssuerError>;
    fn register(
        &self,
        proof: &Secret,
        request: &InstallationRequest,
    ) -> Result<InstallationResponse, IssuerError>;
    fn claim(&self, proof: &Secret, request: &ClaimRequest) -> Result<ClaimResponse, IssuerError>;
    fn rebind(
        &self,
        proof: &Secret,
        request: &RebindRequest,
    ) -> Result<RebindResponse, IssuerError>;
    /// The scoped status of [`contract::status_request`]'s sessions.
    fn status(
        &self,
        proof: &Secret,
        request: &StatusRequest,
    ) -> Result<StatusResponse, IssuerError>;
    /// The sign-in the last call ran under, or the one it will run under:
    /// a [`super::sign_in_fingerprint`], never a token.
    fn sign_in(&self) -> String;
}

/// The issuer over the signed-in History session (Bearer, `rth:acquire`).
pub struct HttpIssuer {
    auth: StoredAuth,
    agent: ureq::Agent,
    /// The fingerprint of the access token the last request carried, which
    /// differs from `auth` after a refresh within the call.
    used: std::sync::Mutex<Option<String>>,
}
const TIMEOUT: Duration = Duration::from_secs(20);
const MAX_ERROR_BODY: u64 = 64 * 1024;

impl HttpIssuer {
    pub fn new(auth: StoredAuth) -> Self {
        Self {
            auth,
            agent: cloud::history_agent().redirects(0).timeout(TIMEOUT).build(),
            used: std::sync::Mutex::new(None),
        }
    }

    fn call<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        proof_header: Option<&Secret>,
        body: Option<&str>,
    ) -> Result<T, IssuerError> {
        self.call_mapped(method, path, proof_header, body, refusal)
    }

    /// [`Self::call`] with the route's own refusal mapping.
    fn call_mapped<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        proof_header: Option<&Secret>,
        body: Option<&str>,
        refusal: fn(ureq::Error) -> IssuerError,
    ) -> Result<T, IssuerError> {
        let stage = cloud::normalized_stage(&self.auth.base_url)
            .map_err(|_| IssuerError::Transport("invalid History address".into()))?;
        let url = format!("{stage}{path}");
        let response = cloud::send_with_auth_refresh_checked_with_budget(
            &self.auth,
            |current, _| {
                *self.used.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(super::sign_in_fingerprint(&current.access_token));
                let mut request = self
                    .agent
                    .request(method, &url)
                    .set("Authorization", &format!("Bearer {}", current.access_token));
                if let Some(proof) = proof_header {
                    request = request.set(contract::PROOF_HEADER, proof.expose());
                }
                match body {
                    Some(body) => request
                        .set("Content-Type", "application/json")
                        .send_string(body),
                    None => request.call(),
                }
                .map_err(Box::new)
            },
            |error| anyhow::Error::new(refusal(error)),
            |_| Ok(()),
            None,
        )
        .map_err(|error| match error.downcast::<IssuerError>() {
            Ok(error) => error,
            Err(_) => IssuerError::Transport("request did not complete".into()),
        })?;
        response
            .into_json()
            .map_err(|_| IssuerError::Transport("unreadable response".into()))
    }
}

/// Map a failed response to its typed refusal by status and error code.
fn refusal(error: ureq::Error) -> IssuerError {
    let ureq::Error::Status(status, response) = error else {
        return IssuerError::Transport("no response".into());
    };
    let mut text = String::new();
    let _ = response
        .into_reader()
        .take(MAX_ERROR_BODY)
        .read_to_string(&mut text);
    let detail = serde_json::from_str::<ErrorBody>(&text)
        .ok()
        .map(|body| body.error);
    let code = detail.as_ref().map(|detail| detail.code.as_str());
    match (status, code) {
        (409, Some(contract::INSTALLATION_PROOF_MISMATCH)) => IssuerError::ProofMismatch,
        // The code came from the detail, so it is always there.
        (409, Some(contract::ACQUISITION_REFUSED)) => detail.map_or_else(
            || IssuerError::Transport(format!("HTTP {status}")),
            |detail| IssuerError::Refused {
                reason: detail.reason.unwrap_or_default(),
                retryable: detail.retryable,
            },
        ),
        (403, Some(contract::INSTALLATION_NOT_PROVEN)) => IssuerError::NotProven,
        (403, Some(contract::NOT_AUTHORIZED)) => IssuerError::NotAuthorized,
        (403, Some(contract::NOT_OWNER)) => IssuerError::NotOwner,
        (403, Some(contract::NOT_INSTALLER)) => IssuerError::NotInstaller,
        (403, code) => IssuerError::Forbidden {
            code: code
                .filter(|code| code_shaped(code))
                .unwrap_or_default()
                .to_owned(),
        },
        (400, _) => IssuerError::InvalidRequest,
        (404, _) => IssuerError::Unavailable,
        (status, _) => IssuerError::Transport(format!("HTTP {status}")),
    }
}

/// A refusal code as the contract writes them (lowercase, digits and `_`,
/// at most 64 bytes); anything else is not kept from the body.
fn code_shaped(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= 64
        && code
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// The status route's mapping: a `414 request_uri_too_long` is a target
/// [`contract::status_request`] should have refused, so a client defect;
/// everything else maps as on every other route.
fn status_refusal(error: ureq::Error) -> IssuerError {
    match error {
        ureq::Error::Status(414, _) => IssuerError::InvalidRequest,
        error => refusal(error),
    }
}

fn json<T: serde::Serialize>(value: &T) -> Result<String, IssuerError> {
    serde_json::to_string(value).map_err(|_| IssuerError::InvalidRequest)
}

impl AcquisitionIssuer for HttpIssuer {
    fn enrolled_sources(&self) -> Result<Option<BTreeSet<String>>, IssuerError> {
        let limits: serde_json::Value = self.call("GET", contract::LIMITS_PATH, None, None)?;
        Ok(contract::enrolled_sources(&limits))
    }
    fn register(
        &self,
        proof: &Secret,
        request: &InstallationRequest,
    ) -> Result<InstallationResponse, IssuerError> {
        let mut body = serde_json::to_value(request).map_err(|_| IssuerError::InvalidRequest)?;
        body[contract::PROOF_FIELD] = serde_json::Value::String(proof.expose().to_owned());
        let body = Secret::new(json(&body)?);
        self.call(
            "POST",
            contract::INSTALLATIONS_PATH,
            None,
            Some(body.expose()),
        )
    }
    fn claim(&self, proof: &Secret, request: &ClaimRequest) -> Result<ClaimResponse, IssuerError> {
        self.call(
            "POST",
            contract::CLAIMS_PATH,
            Some(proof),
            Some(&json(request)?),
        )
    }
    fn rebind(
        &self,
        proof: &Secret,
        request: &RebindRequest,
    ) -> Result<RebindResponse, IssuerError> {
        let body = Secret::new(json(request)?);
        self.call(
            "POST",
            contract::REBINDS_PATH,
            Some(proof),
            Some(body.expose()),
        )
    }
    fn status(
        &self,
        proof: &Secret,
        request: &StatusRequest,
    ) -> Result<StatusResponse, IssuerError> {
        let target = request.target();
        if target.len() > contract::STATUS_MAX_TARGET {
            return Err(IssuerError::InvalidRequest);
        }
        let envelope: StatusEnvelope =
            self.call_mapped("GET", &target, Some(proof), None, status_refusal)?;
        Ok(envelope.result)
    }
    fn sign_in(&self) -> String {
        self.used
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or_else(|| super::sign_in_fingerprint(&self.auth.access_token))
    }
}
