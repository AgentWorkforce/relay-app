//! The bridge methods Desktop adds (`docs/service-callers.md` sections 4
//! and 8): the person's management methods, and the identity and result
//! inbox a caller's runtime uses, either on a connection bound by
//! `spawn_callers.challenge` and `spawn_callers.authenticate`
//! (`docs/service-callers-auth.md`) or, on a connection that never became
//! external, with its phase-1 name and secret.

mod iterators;

use super::auth::{self, Kind, Resolved, Who};
use super::inbox::{ACK, FAIL};
use super::registry::{Binding, CallerRecord};
use super::{node, peer, Class, ServiceCallers};
use crate::contract::method;
use crate::contract::service_callers::{CallerAuthParams, CallerIdentity};
use agent_relay_node::relay::contract::{answer, params, Done};
use agent_relay_node::relay::daemon::CallContext;
use agent_relay_node::relay::protocol::{ErrorKind, RpcError};
use serde_json::Value;
use std::time::Duration;

/// The longest a `next` waits.
pub(crate) const NEXT_MS: u64 = 30_000;
/// How often a waiting `next` looks again without a wake-up.
const RECHECK: Duration = Duration::from_millis(250);

impl ServiceCallers {
    /// `peer_check_unavailable`, audited, where the calling process cannot
    /// be checked (`peer::available`).
    fn peer_available(&self, method: &str) -> Result<(), RpcError> {
        peer::available().map_err(|class| {
            let binding = Binding::current(&*self.host).ok();
            self.refused(binding.as_ref(), None, method, class)
        })
    }

    /// Answers one of Desktop's service-caller methods, or `None` when
    /// `method` is not one.
    pub async fn call(
        &self,
        context: &CallContext,
        method: &str,
        raw: Value,
    ) -> Option<Result<Value, RpcError>> {
        let kind = auth::kind(method)?;
        // One policy where the calling process cannot be checked (Windows):
        // every service-caller method, `challenge` and `authenticate`
        // included, is `peer_check_unavailable` before anything else, so no
        // client is handed a binding it could not use (`peer::available`).
        Some(match self.peer_available(method) {
            Err(error) => Err(error),
            Ok(()) => self.dispatch(context, kind, method, raw).await,
        })
    }

    /// `call` past the peer-check policy: one known method on a platform
    /// that can check the calling process.
    async fn dispatch(
        &self,
        context: &CallContext,
        kind: Kind,
        method: &str,
        raw: Value,
    ) -> Result<Value, RpcError> {
        // Counted in on its connection and resolved once, before any params
        // are read (`docs/service-callers-auth.md` section 2). A params value
        // that is not an object is answered `invalid_params` before the
        // binding decision, except on `authenticate`, which consumes its
        // challenge first; the closed-connection refusal still comes first.
        let entry = self.enter(context.connection_id);
        let pid = context.peer_pid;
        let legacy = auth::has_legacy_keys(&raw);
        // The error `answer` gives a params value of the wrong shape.
        let shape = (!raw.is_object() && !raw.is_null())
            .then(|| params::<serde_json::Map<String, Value>>(raw.clone()).err())
            .flatten();
        let refuse = |class: Class| {
            let binding = Binding::current(&*self.host).ok();
            Err(self.refused(binding.as_ref(), None, method, class))
        };
        match (kind, shape) {
            // `authenticate` takes and counts its connection's outstanding
            // challenge before any other outcome, a phase-1 credential key
            // included, so a refused attempt can never be retried clean
            // (`docs/service-callers-auth.md` section 3).
            (Kind::Auth, _) if method == method::SpawnCallersAuthenticate::NAME => {
                match context.connection_id {
                    None if legacy => refuse(Class::LegacyCredentialRefused),
                    None => refuse(Class::BindingUnavailable),
                    Some(connection_id) => self
                        .authenticate_connection(connection_id, &raw, legacy)
                        .and_then(|bound| {
                            serde_json::to_value(bound).map_err(|_| {
                                RpcError::new(
                                    ErrorKind::Internal,
                                    "The answer could not be written.",
                                )
                            })
                        }),
                }
            }
            // Outside a dev-attest preview build no per-request credential
            // is read: a call carrying one is refused before anything else
            // (`docs/service-callers-auth.md`, requirements 1 and 9).
            _ if legacy && !cfg!(feature = "service-callers-dev-attest") => {
                refuse(Class::LegacyCredentialRefused)
            }
            // A connection the core reported closed fails closed before
            // anything else, whatever the call carries.
            _ if entry.resolved == Resolved::Refused(Class::ConnectionClosed) => {
                refuse(Class::ConnectionClosed)
            }
            (_, Some(error)) => Err(error),
            (Kind::Auth, None) if legacy => refuse(Class::LegacyCredentialRefused),
            (Kind::Auth, None) => match context.connection_id {
                None => refuse(Class::BindingUnavailable),
                Some(connection_id) => {
                    answer::<method::SpawnCallersChallenge, _>(raw, |p| async move {
                        self.challenge(pid, connection_id, &p)
                    })
                    .await
                }
            },
            (Kind::Management, None) => match (&entry.resolved, legacy) {
                (Resolved::NotExternal, _) => {
                    self.management(pid, context.connection_id, method, raw)
                        .await
                }
                (_, true) => refuse(Class::LegacyCredentialRefused),
                (Resolved::Refused(Class::ConnectionClosed), false) => {
                    refuse(Class::ConnectionClosed)
                }
                (_, false) => refuse(Class::ExternalConnection),
            },
            (Kind::BindingRequired, None) => {
                let who = match (&entry.resolved, legacy, context.connection_id) {
                    (Resolved::NotExternal, true, connection_id) => Who::Legacy {
                        peer_pid: pid,
                        connection_id,
                    },
                    (_, true, _) => return refuse(Class::LegacyCredentialRefused),
                    (Resolved::NotExternal, false, _) => return refuse(Class::CallerUnbound),
                    (Resolved::Bound(binding_id), false, Some(connection_id)) => Who::Bound {
                        connection_id,
                        binding_id: binding_id.clone(),
                    },
                    (Resolved::Refused(class), false, _) => return refuse(*class),
                    (Resolved::Bound(_), false, None) => return refuse(Class::CallerUnbound),
                };
                self.binding_required(&who, method, raw).await
            }
        }
    }

    /// The person's management methods, on a connection that is not
    /// external.
    async fn management(
        &self,
        pid: Option<i32>,
        conn: Option<u64>,
        method: &str,
        raw: Value,
    ) -> Result<Value, RpcError> {
        match method {
            method::SpawnCallersList::NAME => {
                answer::<method::SpawnCallersList, _>(
                    raw,
                    |p| async move { self.list(pid, conn, &p) },
                )
                .await
            }
            method::SpawnCallersProvision::NAME => {
                answer::<method::SpawnCallersProvision, _>(raw, |p| async move {
                    self.provision(pid, conn, &p).await
                })
                .await
            }
            method::SpawnCallersRotate::NAME => {
                answer::<method::SpawnCallersRotate, _>(raw, |p| async move {
                    self.rotate(pid, conn, &p).await
                })
                .await
            }
            method::SpawnCallersSetEnabled::NAME => {
                answer::<method::SpawnCallersSetEnabled, _>(raw, |p| async move {
                    self.set_enabled(pid, conn, &p)
                })
                .await
            }
            method::SpawnCallersRevoke::NAME => {
                answer::<method::SpawnCallersRevoke, _>(raw, |p| async move {
                    self.revoke(pid, conn, &p).await.map(|()| Done {})
                })
                .await
            }
            method::SpawnCallersDecide::NAME => {
                answer::<method::SpawnCallersDecide, _>(raw, |p| async move {
                    self.decide(pid, conn, &p).await
                })
                .await
            }
            _ => Err(RpcError::new(
                ErrorKind::Internal,
                "Not a management method.",
            )),
        }
    }

    /// The binding-required methods, as `who`.
    async fn binding_required(
        &self,
        who: &Who,
        method: &str,
        raw: Value,
    ) -> Result<Value, RpcError> {
        match method {
            method::SpawnCallersIdentity::NAME => {
                answer::<method::SpawnCallersIdentity, _>(raw, |p| async move {
                    self.identity_as(who, &p)
                })
                .await
            }
            method::SpawnCallersInboxOpen::NAME => {
                answer::<method::SpawnCallersInboxOpen, _>(
                    raw,
                    |p| async move { self.open(who, &p) },
                )
                .await
            }
            method::SpawnCallersInboxNext::NAME => {
                answer::<method::SpawnCallersInboxNext, _>(raw, |p| async move {
                    self.next(who, &p).await
                })
                .await
            }
            method::SpawnCallersInboxAck::NAME => {
                answer::<method::SpawnCallersInboxAck, _>(raw, |p| async move {
                    self.settle(who, &p, ACK)
                })
                .await
            }
            method::SpawnCallersInboxFail::NAME => {
                answer::<method::SpawnCallersInboxFail, _>(raw, |p| async move {
                    self.settle(who, &p, FAIL)
                })
                .await
            }
            method::SpawnCallersInboxClose::NAME => {
                answer::<method::SpawnCallersInboxClose, _>(
                    raw,
                    |p| async move { self.close(who, &p) },
                )
                .await
            }
            method::SpawnCallersDisconnect::NAME => {
                answer::<method::SpawnCallersDisconnect, _>(raw, |p| async move {
                    self.disconnect(who, &p)
                })
                .await
            }
            _ => Err(RpcError::new(ErrorKind::Internal, "Not a caller method.")),
        }
    }

    /// Section 6 for a caller's own call: this user's process, a live
    /// secret of a live caller, the current account and workspace.
    fn authenticate(
        &self,
        peer_pid: Option<i32>,
        name: &str,
        secret: &Option<String>,
        method: &str,
    ) -> Result<(Binding, CallerRecord), RpcError> {
        let binding = Binding::current(&*self.host);
        peer::caller(&*self.host, peer_pid)
            .map_err(|class| self.refused(binding.as_ref().ok(), None, method, class))?;
        let binding = binding.map_err(|class| self.refused(None, None, method, class))?;
        let refuse = |class| self.refused(Some(&binding), None, method, class);
        let secret = secret
            .as_deref()
            .ok_or_else(|| refuse(Class::CallerUnknown))?;
        let _files = self.files();
        let file = self.load_current(&binding).map_err(refuse)?;
        let index = file
            .authenticate(name, secret, self.host.now_ms())
            .map_err(refuse)?;
        let record = file.callers[index].clone();
        Ok((binding, record))
    }

    /// The caller `who` acts as, with the current binding: the phase-1
    /// name and secret, or the connection's binding rechecked now.
    fn caller(
        &self,
        who: &Who,
        name: &str,
        secret: &Option<String>,
        method: &str,
    ) -> Result<(Binding, CallerRecord), RpcError> {
        let (connection_id, binding_id) = match who {
            Who::Legacy { peer_pid, .. } => {
                return self.authenticate(*peer_pid, name, secret, method)
            }
            Who::Bound {
                connection_id,
                binding_id,
            } => (*connection_id, binding_id.as_str()),
        };
        let binding = Binding::current(&*self.host).ok();
        let caller_id = self.bound_caller(connection_id, binding_id);
        let record = match (&binding, &caller_id) {
            (Some(binding), Some(caller_id)) => {
                let _files = self.files();
                self.load_current(binding)
                    .ok()
                    .and_then(|file| file.callers.get(file.by_id(caller_id)?).cloned())
            }
            _ => None,
        };
        let checked = {
            let mut live = self.live();
            self.check_bound(
                &mut live,
                connection_id,
                binding_id,
                record.as_ref(),
                binding.as_ref(),
            )
        };
        match (checked, binding, record) {
            (Ok(()), Some(binding), Some(record)) => Ok((binding, record)),
            (checked, binding, _) => Err(self.refused(
                binding.as_ref(),
                caller_id.as_deref(),
                method,
                checked.err().unwrap_or(Class::BindingStale),
            )),
        }
    }

    /// The connection a call came in on, and the binding it acts under.
    fn provenance(who: &Who) -> (Option<u64>, Option<String>) {
        match who {
            Who::Legacy { connection_id, .. } => (*connection_id, None),
            Who::Bound {
                connection_id,
                binding_id,
            } => (Some(*connection_id), Some(binding_id.clone())),
        }
    }

    /// `identity` with the phase-1 name and secret, on no connection.
    #[cfg(test)]
    pub(crate) fn identity(
        &self,
        peer_pid: Option<i32>,
        params: &CallerAuthParams,
    ) -> Result<CallerIdentity, RpcError> {
        let who = Who::Legacy {
            peer_pid,
            connection_id: None,
        };
        self.identity_as(&who, params)
    }

    pub(crate) fn identity_as(
        &self,
        who: &Who,
        params: &CallerAuthParams,
    ) -> Result<CallerIdentity, RpcError> {
        const METHOD: &str = "spawn_callers.identity";
        let (binding, record) =
            self.caller(who, &params.caller_name, &params.caller_secret, METHOD)?;
        let node = node::enrolled(&*self.host, &binding.workspace).map_err(|class| {
            self.refused(Some(&binding), Some(&record.caller_id), METHOD, class)
        })?;
        self.audit(&binding, Some(&record.caller_id), METHOD, "ok", None);
        Ok(CallerIdentity {
            publisher: record.publisher(),
            node,
            base_url: binding.workspace.base_url.clone(),
            workspace_id: binding.workspace_id.clone(),
            relaycast_workspace_id: binding.relaycast_workspace_id.clone(),
        })
    }

    /// The core reports that a bridge connection ended: every iterator it
    /// opened is closed and their delivered, unsettled leases are released
    /// at once, as `inbox.close` does, so the items are deliverable to the
    /// next `open`. A connection the core did not identify, or one that
    /// opened no iterator, changes nothing. No I/O under the live lock; the
    /// audit follows once it is released.
    pub fn connection_closed(&self, context: &CallContext) {
        let Some(connection_id) = context.connection_id else {
            return;
        };
        let mut closed: Vec<(String, u64)> = Vec::new();
        {
            let mut live = self.live();
            // An external connection's challenge or binding goes, and the
            // `Closed` marker stays while its calls drain; an untouched one
            // keeps the phase-1 behaviour below.
            self.close_connection(&mut live, connection_id);
            live.closed.insert(connection_id);
            let super::Live {
                iterators, leases, ..
            } = &mut *live;
            iterators.retain(|_, iterator| {
                if iterator.connection_id != Some(connection_id) {
                    return true;
                }
                match closed
                    .iter_mut()
                    .find(|(caller_id, _)| *caller_id == iterator.caller_id)
                {
                    Some((_, count)) => *count += 1,
                    None => closed.push((iterator.caller_id.clone(), 1)),
                }
                false
            });
            if closed.is_empty() {
                return;
            }
            leases.retain(|_, lease| iterators.contains_key(&lease.iterator_id));
        }
        self.changed.notify_waiters();
        let Ok(binding) = Binding::current(&*self.host) else {
            return;
        };
        for (caller_id, count) in closed {
            self.audit(
                &binding,
                Some(&caller_id),
                "spawn_callers.inbox.close",
                "connection_closed",
                Some(count),
            );
        }
    }

    pub(crate) fn disconnect(
        &self,
        who: &Who,
        params: &CallerAuthParams,
    ) -> Result<Done, RpcError> {
        const METHOD: &str = "spawn_callers.disconnect";
        let (binding, record) =
            self.caller(who, &params.caller_name, &params.caller_secret, METHOD)?;
        let caller_id = record.caller_id.as_str();
        self.sweep(self.host.now_ms());
        let open = self
            .live()
            .iterators
            .values()
            .any(|iterator| iterator.caller_id == caller_id);
        if open {
            return Err(self.refused(Some(&binding), Some(caller_id), METHOD, Class::IteratorOpen));
        }
        if let Who::Bound { connection_id, .. } = who {
            // The binding goes stale; the connection can neither call nor
            // authenticate again.
            let _files = self.files();
            let mut live = self
                .gate(who, &binding)
                .map_err(|class| self.refused(Some(&binding), Some(caller_id), METHOD, class))?;
            self.unbind(&mut live, *connection_id);
        }
        self.audit(&binding, Some(caller_id), METHOD, "ok", None);
        Ok(Done {})
    }
}
