//! Regression tests for the Claude review findings that
//! `tests/adversarial_*.rs` do not pin on their own.
#[path = "findings/builders.rs"]
mod builders;

use builders::*;
use session_navigation::decide::decide;
use session_navigation::engine::verify;
use session_navigation::evidence::*;
use session_navigation::model::*;
use session_navigation::plan::*;

fn req() -> OpenRequest {
    OpenRequest {
        // An explicit terminal preference: there is no built-in default.
        prefs: session_navigation::plan::Preferences {
            default_terminal: Some(session_navigation::model::TerminalKind::Ghostty),
            ..Default::default()
        },
        ..OpenRequest::default()
    }
}

// --------------------------------------------------------------- Claude ---

fn shown(
    post_claims: Vec<DesktopClaim>,
    before_reg: Vec<RegistryClaim>,
    after_reg: Vec<RegistryClaim>,
) -> Outcome {
    let table = vec![launchd(), proc_(821, 1, Exe::ClaudeCli, None, &["claude"])];
    let before = snap(
        Provider::Claude,
        CLAUDE_ID,
        table.clone(),
        ProviderEvidence::Claude(claude_ev(before_reg, post_claims.clone(), None)),
    );
    let mut after = snap(
        Provider::Claude,
        CLAUDE_ID,
        table,
        ProviderEvidence::Claude(claude_ev(after_reg, post_claims, None)),
    );
    after.front.app = Fact::Known(Some(App::ClaudeDesktop));
    let post = PostCondition::ConversationShown {
        app: App::ClaudeDesktop,
    };
    verify::check_against(&post, &before, &after, &req())
}

/// Finding 6, 15: A Claude link is verified only by a registry claim that appeared
/// for a record holding the session as its current transcript.
#[test]
fn f15_claude_link_proof_is_a_new_claim_for_a_live_record() {
    let live = || vec![claim(LOCAL_A, ClaimKind::Live, false, CLAUDE_ID)];
    let named = || vec![registry(821, CLAUDE_ID, Some(LOCAL_A))];
    assert_eq!(shown(live(), vec![], named()), Outcome::Verified);
    assert_eq!(
        shown(live(), named(), named()),
        Outcome::DispatchedUnverified
    );
    let stem = vec![claim(LOCAL_A, ClaimKind::Stem, false, CLAUDE_OTHER)];
    assert_eq!(shown(stem, vec![], named()), Outcome::DispatchedUnverified);
    let archived = vec![claim(LOCAL_A, ClaimKind::Live, true, CLAUDE_ID)];
    assert_eq!(
        shown(archived, vec![], vec![]),
        Outcome::Failed(Reason::WrongDestination)
    );
}

/// Finding d: A record of another account gets no link; an unknown account neither.
#[test]
fn fd_claude_record_of_another_account_gets_no_link() {
    let mut other = claim(LOCAL_A, ClaimKind::Live, false, CLAUDE_ID);
    other.account = "acct2/org".into();
    let ev = claude_ev(vec![], vec![other.clone()], Some(Entrypoint::Desktop));
    let s = snap(
        Provider::Claude,
        CLAUDE_ID,
        vec![launchd()],
        ProviderEvidence::Claude(ev),
    );
    assert_eq!(
        decide(&s, &req()).plan,
        Plan::Unavailable(Reason::OtherAccount)
    );
    other.archived = true;
    let ev = claude_ev(vec![], vec![other], Some(Entrypoint::Desktop));
    let s = snap(
        Provider::Claude,
        CLAUDE_ID,
        vec![launchd()],
        ProviderEvidence::Claude(ev),
    );
    assert_eq!(
        decide(&s, &req()).plan,
        Plan::Unavailable(Reason::OtherAccount)
    );
    let mut ev = claude_ev(
        vec![],
        vec![claim(LOCAL_A, ClaimKind::Live, false, CLAUDE_ID)],
        None,
    );
    ev.account = Fact::Unknown;
    let s = snap(
        Provider::Claude,
        CLAUDE_ID,
        vec![launchd()],
        ProviderEvidence::Claude(ev),
    );
    assert_eq!(
        decide(&s, &req()).plan,
        Plan::Unavailable(Reason::EvidenceIncomplete)
    );
}

/// Reviving a desktop record restarts its current conversation: not while
/// that conversation runs in a terminal.
#[test]
fn claude_revival_never_duplicates_the_records_current_conversation() {
    let table = in_ghostty("/dev/ttys001", 900, Exe::ClaudeCli, &["claude"]);
    let ev = claude_ev(
        vec![registry(900, CLAUDE_OTHER, None)],
        vec![claim(LOCAL_A, ClaimKind::Prior, true, CLAUDE_OTHER)],
        Some(Entrypoint::Desktop),
    );
    let mut s = snap(
        Provider::Claude,
        CLAUDE_ID,
        table,
        ProviderEvidence::Claude(ev),
    );
    s.capabilities = vec![Capability::ClaudeRegistryFollowsSession {
        cli_version: "2.1.321".into(),
    }];
    let request = OpenRequest {
        prefs: Preferences {
            allow_import: true,
            ..req().prefs
        },
        ..req()
    };
    assert_eq!(
        decide(&s, &request).plan,
        Plan::Unavailable(Reason::RevivalCollides)
    );
}

fn claude_closed_next_to_other_cli(capabilities: Vec<Capability>) -> Snapshot {
    let table = in_ghostty("/dev/ttys001", 900, Exe::ClaudeCli, &["claude"]);
    let ev = claude_ev(
        vec![registry(900, CLAUDE_OTHER, None)],
        vec![],
        Some(Entrypoint::Cli),
    );
    let mut s = snap(
        Provider::Claude,
        CLAUDE_ID,
        table,
        ProviderEvidence::Claude(ev),
    );
    s.capabilities = capabilities;
    s
}

/// Finding 19: Another CLI in the repo: confirm (naming it), then resume; with the
/// natively verified capability for this CLI version, resume directly.
#[test]
fn f19_unrelated_cli_needs_confirmation_unless_the_registry_is_authoritative() {
    let s = claude_closed_next_to_other_cli(vec![]);
    let Plan::Confirm(c) = decide(&s, &req()).plan else {
        panic!("expected a confirmation");
    };
    assert_eq!(c.reason, Reason::PossiblyOpenElsewhere);
    assert_eq!(c.elsewhere.len(), 1);
    assert_eq!(c.elsewhere[0].tty.as_deref(), Some("/dev/ttys001"));
    assert_eq!(c.then.kind, ActionKind::Resume);
    let confirmed = OpenRequest {
        confirm_not_open_elsewhere: true,
        ..req()
    };
    assert_eq!(
        decide(&s, &confirmed).action().map(|a| a.kind),
        Some(ActionKind::Resume)
    );
    let verified = |v: &str| {
        vec![Capability::ClaudeRegistryFollowsSession {
            cli_version: v.into(),
        }]
    };
    let s = claude_closed_next_to_other_cli(verified("2.1.321"));
    assert_eq!(
        decide(&s, &req()).action().map(|a| a.kind),
        Some(ActionKind::Resume)
    );
    let s = claude_closed_next_to_other_cli(verified("2.0.0"));
    assert!(
        matches!(decide(&s, &req()).plan, Plan::Confirm(_)),
        "capability is version-gated"
    );
}
