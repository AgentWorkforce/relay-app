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

/// A placeholder customer's co-brand.
const EXAMPLE_CUSTOMER: CoBrand = CoBrand {
    name: "Example Customer",
    mark_kind: MarkKind::Svg,
    mark: b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\"/>",
};

/// A co-branded build named Agent Relay draws the wordmark with no badge in
/// the lockups that draw the "for" line under it, and keeps its badge
/// everywhere else (the header, the window title), as on macOS.
#[test]
fn a_cobranded_build_named_agent_relay_draws_the_wordmark_over_its_line() {
    let cobranded = Profile {
        product_name: DEFAULT_PRODUCT_NAME,
        cobrand: Some(EXAMPLE_CUSTOMER),
        ..OTHER
    };
    assert!(cobranded.draws_wordmark_over_cobrand_line());
    assert!(cobranded.names_in_text());
    assert_eq!(cobranded.badge(), Some("example-7"));
    assert_eq!(cobranded.window_title(), "Agent Relay (example-7)");
    // Without the co-brand it is named in text with its badge there too.
    let plain = Profile {
        cobrand: None,
        ..cobranded
    };
    assert!(!plain.draws_wordmark_over_cobrand_line());
    assert_eq!(plain.badge(), Some("example-7"));
}

/// A co-branded build with a name of its own is named in text in every
/// lockup, as a branded build with no badge; the default profile draws the
/// wordmark in every lockup.
#[test]
fn only_a_build_named_agent_relay_draws_the_wordmark_over_the_line() {
    for name in ["Example Desktop", DEVELOPMENT_PRODUCT_NAME] {
        let cobranded = Profile {
            product_name: name,
            cobrand: Some(EXAMPLE_CUSTOMER),
            ..OTHER
        };
        assert!(!cobranded.draws_wordmark_over_cobrand_line(), "{name}");
    }
    assert!(Profile::DEFAULT.draws_wordmark_over_cobrand_line());
    // A build without a co-brand draws it wherever it draws it elsewhere.
    for profile in [
        OTHER,
        Profile {
            product_name: DEFAULT_PRODUCT_NAME,
            ..OTHER
        },
    ] {
        assert_eq!(
            profile.draws_wordmark_over_cobrand_line(),
            !profile.names_in_text()
        );
    }
}

/// The co-brand's name is read as macOS's `AppInfo.coBrandName` reads it,
/// and its line is read aloud as "for <name>".
#[test]
fn a_cobrand_name_is_trimmed_and_checked_as_on_macos() {
    assert_eq!(
        CoBrand::name_from("  Example Customer\t"),
        Some("Example Customer")
    );
    assert_eq!(CoBrand::name_from(" \t "), None);
    assert_eq!(CoBrand::name_from("Example\nCustomer"), None);
    assert_eq!(CoBrand::name_from("Example\u{7}Customer"), None);
    let longest = "A".repeat(80);
    assert_eq!(CoBrand::name_from(&longest), Some(longest.as_str()));
    assert_eq!(CoBrand::name_from(&"A".repeat(81)), None);
    // Characters, not bytes, are counted.
    let accented = "é".repeat(80);
    assert_eq!(CoBrand::name_from(&accented), Some(accented.as_str()));
    assert_eq!(EXAMPLE_CUSTOMER.accessible_label(), "for Example Customer");
}
