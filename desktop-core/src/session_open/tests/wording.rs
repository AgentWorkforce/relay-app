//! Every reason has its own stable code and one sentence, and the button
//! vocabulary is the one the apps decode.
use std::collections::BTreeSet;

use session_navigation::model::{Reason, TerminalKind};

use crate::session_open::contract::{Action, Outcome, Precision, State, TERMINAL_IDS};
use crate::session_open::wording::{self, Local};

/// Every core reason. The `match` below stops compiling when the core adds
/// one, so this list cannot fall behind.
fn all_reasons() -> Vec<Reason> {
    fn listed(reason: Reason) -> bool {
        match reason {
            Reason::InvalidSessionId
            | Reason::EvidenceIncomplete
            | Reason::SessionNotFound
            | Reason::Archived
            | Reason::ChildSession
            | Reason::CwdUnknown
            | Reason::CwdMissing
            | Reason::AppNotInstalled
            | Reason::ToolNotInstalled
            | Reason::SchemeHandlerMismatch
            | Reason::UnverifiedVersion
            | Reason::ServerVersionMismatch
            | Reason::LaunchPending
            | Reason::LiveNotLocatable
            | Reason::RunningHeadless
            | Reason::AmbiguousIdentity
            | Reason::PriorIdentityOnly
            | Reason::PossiblyOpenElsewhere
            | Reason::DesktopRecordMissing
            | Reason::NoSessionDeepLink
            | Reason::SelectionGone
            | Reason::PermissionDenied
            | Reason::TakeOverRaced
            | Reason::ReservationStoreFailed
            | Reason::TargetGone
            | Reason::WrongDestination
            | Reason::DispatchFailed
            | Reason::EnvironmentUnknown
            | Reason::CwdConflict
            | Reason::OtherAccount
            | Reason::SessionBusy
            | Reason::RevivalCollides
            | Reason::PossiblySecondView
            | Reason::PlatformUnsupported
            | Reason::HostUnsupported
            | Reason::HostSettingDisabled
            | Reason::PermissionRequired
            | Reason::WaylandNoRaise
            | Reason::WindowAmbiguous
            | Reason::PaneSelectionUnavailable
            | Reason::ForeignRuntime
            | Reason::NoDefaultTerminal => true,
        }
    }
    let all = vec![
        Reason::InvalidSessionId,
        Reason::EvidenceIncomplete,
        Reason::SessionNotFound,
        Reason::Archived,
        Reason::ChildSession,
        Reason::CwdUnknown,
        Reason::CwdMissing,
        Reason::AppNotInstalled,
        Reason::ToolNotInstalled,
        Reason::SchemeHandlerMismatch,
        Reason::UnverifiedVersion,
        Reason::ServerVersionMismatch,
        Reason::LaunchPending,
        Reason::LiveNotLocatable,
        Reason::RunningHeadless,
        Reason::AmbiguousIdentity,
        Reason::PriorIdentityOnly,
        Reason::PossiblyOpenElsewhere,
        Reason::DesktopRecordMissing,
        Reason::NoSessionDeepLink,
        Reason::SelectionGone,
        Reason::PermissionDenied,
        Reason::TakeOverRaced,
        Reason::ReservationStoreFailed,
        Reason::TargetGone,
        Reason::WrongDestination,
        Reason::DispatchFailed,
        Reason::EnvironmentUnknown,
        Reason::CwdConflict,
        Reason::OtherAccount,
        Reason::SessionBusy,
        Reason::RevivalCollides,
        Reason::PossiblySecondView,
        Reason::PlatformUnsupported,
        Reason::HostUnsupported,
        Reason::HostSettingDisabled,
        Reason::PermissionRequired,
        Reason::WaylandNoRaise,
        Reason::WindowAmbiguous,
        Reason::PaneSelectionUnavailable,
        Reason::ForeignRuntime,
        Reason::NoDefaultTerminal,
    ];
    assert!(all.iter().all(|r| listed(*r)));
    all
}

const LOCALS: [Local; 7] = [
    Local::InvalidRequest,
    Local::UnsupportedSource,
    Local::UnknownTerminal,
    Local::AlreadyOpening,
    Local::DesktopIdUnmapped,
    Local::SeveralLocations,
    Local::HomeUnavailable,
];

fn snake(code: &str) -> bool {
    !code.is_empty() && code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
}

#[test]
fn every_reason_has_its_own_words() {
    let reasons = all_reasons();
    assert_eq!(reasons.len(), 42);
    let mut codes = BTreeSet::new();
    for reason in reasons {
        let (code, sentence) = wording::reason(reason);
        assert_ne!(code, "unlisted", "{reason:?} reached a group's fallback");
        assert!(snake(code), "{code}");
        assert!(codes.insert(code), "{code} used twice");
        assert!(sentence.ends_with('.'), "{reason:?}: {sentence}");
        assert!(!sentence.contains("  "), "{sentence}");
        // The serde name, snake-cased, is the code: the apps may log either.
        let name = serde_json::to_value(reason).unwrap();
        let name = name.as_str().unwrap();
        let mut expected = String::new();
        for (i, c) in name.char_indices() {
            if c.is_ascii_uppercase() && i > 0 {
                expected.push('_');
            }
            expected.push(c.to_ascii_lowercase());
        }
        assert_eq!(code, expected);
    }
    for local in LOCALS {
        let (code, sentence) = wording::local(local);
        assert!(snake(code));
        assert!(codes.insert(code), "{code} used twice");
        assert!(sentence.ends_with('.'));
        assert_eq!(local.to_string(), sentence);
    }
}

#[test]
fn refusals_keep_the_apps_existing_sentences() {
    let said = |r| wording::reason(r).1;
    assert_eq!(
        said(Reason::LaunchPending),
        "A resume request was already sent. The provider has not confirmed it opened. Check the earlier launch before trying again."
    );
    assert_eq!(
        said(Reason::LiveNotLocatable),
        "This session is running, but its exact window cannot be located. No new session was started."
    );
    assert_eq!(
        wording::local(Local::AlreadyOpening).1,
        "This session is already being opened."
    );
    for reason in [
        Reason::LiveNotLocatable,
        Reason::NoSessionDeepLink,
        Reason::HostUnsupported,
    ] {
        assert!(said(reason).ends_with(wording::NOTHING_STARTED));
    }
}

#[test]
fn terminal_ids_round_trip_and_name_every_terminal() {
    let ids: Vec<_> = TerminalKind::ALL
        .into_iter()
        .map(wording::terminal_id)
        .collect();
    assert_eq!(ids, TERMINAL_IDS);
    for terminal in TerminalKind::ALL {
        assert!(!wording::terminal_name(terminal).is_empty());
        assert_eq!(
            crate::session_open::session::terminal(Some(wording::terminal_id(terminal))),
            Ok(Some(terminal))
        );
    }
    assert_eq!(
        crate::session_open::session::terminal(Some("Ghostty")),
        Err(Local::UnknownTerminal)
    );
    assert_eq!(crate::session_open::session::terminal(None), Ok(None));
}

#[test]
fn the_vocabulary_is_snake_case_json() {
    let text = |v: serde_json::Value| v.as_str().unwrap().to_owned();
    let actions = [
        (Action::Focus, "focus"),
        (Action::ShowApplication, "show_application"),
        (Action::OpenLink, "open_link"),
        (Action::Resume, "resume"),
        (Action::TakeOver, "take_over"),
        (Action::Attach, "attach"),
        (Action::Choose, "choose"),
        (Action::Confirm, "confirm"),
        (Action::Unavailable, "unavailable"),
    ];
    for (action, name) in actions {
        assert_eq!(text(serde_json::to_value(action).unwrap()), name);
    }
    for (outcome, name) in [
        (Outcome::Verified, "verified"),
        (Outcome::DispatchedUnverified, "dispatched_unverified"),
        (Outcome::Refused, "refused"),
        (Outcome::Failed, "failed"),
    ] {
        assert_eq!(text(serde_json::to_value(outcome).unwrap()), name);
    }
    for (state, name) in [
        (State::Live, "live"),
        (State::LiveMultiple, "live_multiple"),
        (State::NotFound, "not_found"),
        (State::LaunchPending, "launch_pending"),
        (State::Unsupported, "unsupported"),
    ] {
        assert_eq!(text(serde_json::to_value(state).unwrap()), name);
    }
    assert_eq!(
        text(serde_json::to_value(Precision::ExactConversation).unwrap()),
        "exact_conversation"
    );
}
