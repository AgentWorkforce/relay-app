use super::tests::OTHER;
use super::*;

/// A renamed build that kept the default asset prefix is identified, never
/// by the default's own prefix, and as a branded build carries no badge.
#[test]
fn a_renamed_build_with_the_default_prefix_is_not_badged_by_it() {
    let renamed = Profile {
        product_name: "Acme Desktop",
        ..Profile::DEFAULT
    };
    let identity = renamed.identity().expect("a renamed build is identified");
    assert_ne!(identity, "AgentRelay");
    assert_eq!(renamed.badge(), None);
    assert_eq!(renamed.window_title(), "Acme Desktop");
}

/// A customer's branded build never shows its opaque profile id beside its
/// name; Agent Relay Dev and a profile still named Agent Relay do.
#[test]
fn a_branded_build_carries_no_badge() {
    let branded = Profile {
        product_name: "Agent Relay for Automation Anywhere",
        ..super::tests::OTHER
    };
    assert!(branded.is_branded());
    assert_eq!(branded.badge(), None);
    assert_eq!(branded.identity(), Some("example-7"));
    assert_eq!(
        branded.window_title(),
        "Agent Relay for Automation Anywhere"
    );
    for name in [DEFAULT_PRODUCT_NAME, DEVELOPMENT_PRODUCT_NAME] {
        let named = Profile {
            product_name: name,
            ..super::tests::OTHER
        };
        assert_eq!(named.badge(), Some("example-7"));
        assert_eq!(named.window_title(), format!("{name} (example-7)"));
    }
}

#[test]
fn a_build_with_another_repository_or_feed_is_marked() {
    let repository = Profile {
        releases_repository: "example-org/other-releases",
        ..Profile::DEFAULT
    };
    assert!(repository.badge().is_some());
    let feed = Profile {
        release_feed: CLOUD_RELEASE_FEED,
        ..Profile::DEFAULT
    };
    assert!(feed.badge().is_some());
    assert_ne!(feed.badge(), Some("AgentRelay"));
}

/// Only the default profile has no identity; a branded one (OTHER) has
/// one but carries no badge.
#[test]
fn only_the_default_profile_is_unidentified() {
    assert_eq!(Profile::DEFAULT.badge(), None);
    assert_eq!(Profile::DEFAULT.identity(), None);
    assert_eq!(Profile::DEFAULT.window_title(), "Agent Relay");
    assert_eq!(OTHER.identity(), Some("example-7"));
    assert_eq!(OTHER.badge(), None);
    let prefixed = Profile {
        asset_prefix: "Acme",
        ..Profile::DEFAULT
    };
    assert_eq!(prefixed.badge(), Some("Acme"));
}
