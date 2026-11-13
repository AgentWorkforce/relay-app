//! What the build's site profile fixed in this app (docs/workspace-layout.md,
//! "Site profiles"), as `build.rs` handed it to the compiler: the product
//! name shown for the app, the GitHub repository the updater reads releases
//! from, what those releases' artifact names start with, and the release
//! feed with the key its records are signed with. The default
//! profile's values are built in when `scripts/build-gtk.sh` (or
//! `build-windows.sh`) is given no others; nothing at run time changes
//! them. The site and link scheme are the core's
//! (`agent_relay_probe::relay::links`).

/// The product name: "Agent Relay" for the default profile.
pub const PRODUCT_NAME: &str = env!("DESKTOP_PRODUCT_NAME");
/// The default profile's product name, whose artwork (the wordmark) the
/// app carries; any other profile is named in text.
pub const DEFAULT_PRODUCT_NAME: &str = "Agent Relay";
/// Agent Relay Dev's product name, which is no customer's brand and so
/// still carries its badge, as on macOS.
pub const DEVELOPMENT_PRODUCT_NAME: &str = "Agent Relay Dev";
/// The `owner/name` GitHub repository the updater reads releases from.
pub const RELEASES_REPOSITORY: &str = env!("RELEASES_REPOSITORY");
/// What a release's artifact names start with: `AgentRelay` for the default
/// profile, `Desktop-<id>` for a profile named by its opaque id.
pub const ASSET_PREFIX: &str = env!("DESKTOP_ASSET_PREFIX");
/// Where the updater's releases come from: `github:<owner>/<name>` (by
/// default the releases repository) or `cloud`, the profile's own site.
pub const RELEASE_FEED: &str = env!("RELEASE_FEED");
/// The base64 Ed25519 public key a cloud feed's release records are signed
/// with; empty for a `github:` feed.
pub const FEED_PUBLIC_KEY: &str = env!("DESKTOP_FEED_PUBLIC_KEY");
/// The profile's opaque id, as its asset prefix carries it; empty for the
/// default profile.
pub const PROFILE_ID: &str = env!("DESKTOP_PROFILE_ID");
/// The default profile's release feed.
pub const DEFAULT_RELEASE_FEED: &str = "github:AgentWorkforce/relay-desktop-releases";
/// The release feed of a build whose releases come from its site rather
/// than a GitHub repository.
pub const CLOUD_RELEASE_FEED: &str = "cloud";

/// The release feed this app was built with ([`RELEASE_FEED`]).
pub fn release_feed() -> &'static str {
    RELEASE_FEED
}

/// The key this app's cloud release feed is signed with
/// ([`FEED_PUBLIC_KEY`]); empty for a `github:` feed.
pub fn feed_public_key() -> &'static str {
    FEED_PUBLIC_KEY
}

/// This app's window title ([`Profile::window_title`] of the built profile).
pub fn window_title() -> String {
    Profile::BUILT.window_title()
}

/// This app's site profile id ([`PROFILE_ID`]); empty for the default.
pub fn profile_id() -> &'static str {
    PROFILE_ID
}

pub mod windows_records;

/// Where a profile's updates come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseFeed {
    /// The GitHub releases of the profile's repository.
    GitHub,
    /// The signed feed of the signed-in site (`updates/cloud.rs`).
    Cloud,
}

/// The signed-in site's downloads page, for a person to read. **Proposed**:
/// the Cloud team's draft names this path, which may change.
pub fn cloud_downloads_page(site: &str) -> String {
    format!("{}/cloud/dashboard/downloads", site.trim_end_matches('/'))
}

/// A site profile's own values. The app runs with [`Profile::BUILT`]; code
/// that branches on the profile takes one, so tests can hand it any profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Profile {
    /// The product name shown for the app.
    pub product_name: &'static str,
    /// The `owner/name` GitHub repository the updater reads releases from.
    pub releases_repository: &'static str,
    /// What the release's artifact names start with.
    pub asset_prefix: &'static str,
    /// Where the updater's releases come from: `github:<owner>/<name>` or
    /// [`CLOUD_RELEASE_FEED`].
    pub release_feed: &'static str,
}

impl Profile {
    /// The profile this app was built with.
    pub const BUILT: Self = Self {
        product_name: PRODUCT_NAME,
        releases_repository: RELEASES_REPOSITORY,
        asset_prefix: ASSET_PREFIX,
        release_feed: RELEASE_FEED,
    };
    /// The default profile: Agent Relay.
    pub const DEFAULT: Self = Self {
        product_name: DEFAULT_PRODUCT_NAME,
        releases_repository: "AgentWorkforce/relay-desktop-releases",
        asset_prefix: "AgentRelay",
        release_feed: DEFAULT_RELEASE_FEED,
    };

    /// Whether the profile's releases come from its site's signed feed
    /// rather than a GitHub repository.
    pub fn reads_cloud_feed(&self) -> bool {
        self.release_feed == CLOUD_RELEASE_FEED
    }

    /// Where the profile's updates come from.
    pub fn release_source(&self) -> ReleaseFeed {
        if self.reads_cloud_feed() {
            ReleaseFeed::Cloud
        } else {
            ReleaseFeed::GitHub
        }
    }

    /// The GitHub repository the profile's releases are in: none for a
    /// cloud feed, nor for an empty repository.
    fn github_repository(&self) -> Option<&'static str> {
        Some(self.releases_repository)
            .filter(|repository| !self.reads_cloud_feed() && !repository.is_empty())
    }

    /// The GitHub API's latest published release in the profile's
    /// repository; none for a profile without one.
    pub fn latest_release_url(&self) -> Option<String> {
        self.github_repository()
            .map(|repository| format!("https://api.github.com/repos/{repository}/releases/latest"))
    }

    /// The GitHub API's list of the profile's releases; none for a profile
    /// without a repository.
    pub fn releases_url(&self) -> Option<String> {
        self.github_repository()
            .map(|repository| format!("https://api.github.com/repos/{repository}/releases"))
    }

    /// The profile's releases page, for a person to read: its repository's
    /// releases, or for a cloud feed the built-in site's downloads page
    /// ([`cloud_downloads_page`], a proposed path); none for a profile
    /// without either.
    pub fn releases_page(&self) -> Option<String> {
        if self.reads_cloud_feed() {
            return Some(cloud_downloads_page(
                agent_relay_node::relay::links::built_in_site_url(),
            ));
        }
        self.github_repository()
            .map(|repository| format!("https://github.com/{repository}/releases"))
    }

    /// The start of a release's artifact names for `platform` and `arch`:
    /// `<prefix>-<version>-<platform>-<arch>`.
    pub fn asset_base(&self, version: &str, platform: &str, arch: &str) -> String {
        format!("{}-{version}-{platform}-{arch}", self.asset_prefix)
    }

    /// Whether the app names itself in text: every profile but the default,
    /// whose artwork (the wordmark) the app carries. A profile that kept the
    /// default's name is still told apart by its asset prefix.
    pub fn names_in_text(&self) -> bool {
        self.product_name != DEFAULT_PRODUCT_NAME || self.asset_prefix != Self::DEFAULT.asset_prefix
    }

    /// Whether the profile is named for its own product rather than Agent
    /// Relay (a customer's branded build): any name but "Agent Relay" and
    /// [`DEVELOPMENT_PRODUCT_NAME`]. Its name already tells it apart, so it
    /// carries no badge beside it, as on macOS.
    pub fn is_branded(&self) -> bool {
        self.product_name != DEFAULT_PRODUCT_NAME && self.product_name != DEVELOPMENT_PRODUCT_NAME
    }

    /// The marker a build carries beside its name, in the tray icon and the
    /// window title: its [`identity`](Self::identity), for a build named
    /// Agent Relay (or Agent Relay Dev). None for the default profile and
    /// for a [branded](Self::is_branded) one, whose customers never see the
    /// profile's opaque id beside its name, as on macOS.
    pub fn badge(&self) -> Option<&'static str> {
        self.identity().filter(|_| !self.is_branded())
    }

    /// What tells the build apart in Settings > About, branded or not: the
    /// profile's opaque id; for a build that differs from the default
    /// without one, its asset prefix (less `Desktop-`) if that differs, else
    /// `profile`. None only for the default profile, as on macOS, which
    /// identifies any non-production build.
    pub fn identity(&self) -> Option<&'static str> {
        if *self == Self::BUILT && !PROFILE_ID.is_empty() {
            return Some(PROFILE_ID);
        }
        if *self == Self::DEFAULT {
            return None;
        }
        let prefix = self
            .asset_prefix
            .strip_prefix("Desktop-")
            .unwrap_or(self.asset_prefix);
        Some(
            if self.asset_prefix != Self::DEFAULT.asset_prefix && !prefix.is_empty() {
                prefix
            } else {
                "profile"
            },
        )
    }

    /// The window title: the product name, then the badge in parentheses
    /// for a profile that carries one.
    pub fn window_title(&self) -> String {
        match self.badge() {
            Some(badge) => format!("{} ({badge})", self.product_name),
            None => self.product_name.to_owned(),
        }
    }
}

#[cfg(test)]
mod marking_tests;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn the_profile_is_well_formed() {
        assert!(!PRODUCT_NAME.trim().is_empty());
        assert!(!PRODUCT_NAME.chars().any(char::is_control));
        if release_feed() == "cloud" {
            // A cloud feed reads no repository, and is signed with a key.
            assert_eq!(RELEASES_REPOSITORY, "");
            assert_eq!(feed_public_key().len(), 44, "{}", feed_public_key());
            assert!(!profile_id().is_empty());
        } else {
            let (owner, name) = RELEASES_REPOSITORY.split_once('/').expect("owner/name");
            assert!(!owner.is_empty() && !name.is_empty() && !name.contains('/'));
            assert!(release_feed()
                .strip_prefix("github:")
                .is_some_and(|named| named.eq_ignore_ascii_case(RELEASES_REPOSITORY)));
            assert_eq!(feed_public_key(), "");
        }
        assert!(
            ASSET_PREFIX == "AgentRelay" || ASSET_PREFIX.starts_with("Desktop-"),
            "{ASSET_PREFIX}"
        );
        assert_eq!(
            profile_id(),
            ASSET_PREFIX.strip_prefix("Desktop-").unwrap_or_default()
        );
        // A build without a profile is the default one.
        if std::env::var_os("DESKTOP_ASSET_PREFIX").is_none() {
            assert_eq!(PRODUCT_NAME, DEFAULT_PRODUCT_NAME);
            assert_eq!(RELEASES_REPOSITORY, "AgentWorkforce/relay-desktop-releases");
            assert_eq!(ASSET_PREFIX, "AgentRelay");
            assert_eq!(Profile::BUILT, Profile::DEFAULT);
            assert_eq!(release_feed(), DEFAULT_RELEASE_FEED);
            assert_eq!(feed_public_key(), "");
            assert_eq!(profile_id(), "");
        }
    }

    /// A profile other than the default, by its opaque id.
    pub(crate) const OTHER: Profile = Profile {
        product_name: "Example Desktop",
        releases_repository: "example-org/example-desktop-releases",
        asset_prefix: "Desktop-example-7",
        release_feed: "github:example-org/example-desktop-releases",
    };

    /// A profile whose releases come from its site's feed.
    pub(crate) const CLOUD: Profile = Profile {
        product_name: "Example Desktop",
        releases_repository: "",
        asset_prefix: "Desktop-example-7",
        release_feed: CLOUD_RELEASE_FEED,
    };

    #[test]
    fn a_cloud_profile_forms_no_github_url() {
        assert!(CLOUD.reads_cloud_feed());
        assert_eq!(CLOUD.release_source(), ReleaseFeed::Cloud);
        assert_eq!(OTHER.release_source(), ReleaseFeed::GitHub);
        assert_eq!(Profile::DEFAULT.release_source(), ReleaseFeed::GitHub);
        assert_eq!(CLOUD.latest_release_url(), None);
        assert_eq!(CLOUD.releases_url(), None);
        let downloads = cloud_downloads_page(agent_relay_node::relay::links::built_in_site_url());
        assert!(
            downloads.ends_with("/cloud/dashboard/downloads"),
            "{downloads}"
        );
        assert_eq!(CLOUD.releases_page(), Some(downloads.clone()));
        assert_eq!(
            cloud_downloads_page("https://site.example/"),
            "https://site.example/cloud/dashboard/downloads"
        );
        // Not even with a repository beside it.
        let named = Profile {
            releases_repository: "example-org/example-desktop-releases",
            ..CLOUD
        };
        assert_eq!(named.latest_release_url(), None);
        assert_eq!(named.releases_url(), None);
        assert_eq!(named.releases_page(), Some(downloads));
        assert!(!OTHER.reads_cloud_feed() && !Profile::DEFAULT.reads_cloud_feed());
        assert_eq!(
            Profile::BUILT.reads_cloud_feed(),
            release_feed() == CLOUD_RELEASE_FEED
        );
    }

    #[test]
    fn a_profile_names_its_own_repository_assets_and_product() {
        assert_eq!(
            OTHER.latest_release_url(),
            Some(
                "https://api.github.com/repos/example-org/example-desktop-releases/releases/latest"
                    .to_owned()
            )
        );
        assert_eq!(
            OTHER.releases_url(),
            Some(
                "https://api.github.com/repos/example-org/example-desktop-releases/releases"
                    .to_owned()
            )
        );
        assert_eq!(
            OTHER.releases_page(),
            Some("https://github.com/example-org/example-desktop-releases/releases".to_owned())
        );
        assert_eq!(
            OTHER.asset_base("2026.10.9", "Linux", "x64"),
            "Desktop-example-7-2026.10.9-Linux-x64"
        );
        assert!(OTHER.names_in_text());
        assert_eq!(
            Profile::DEFAULT.latest_release_url(),
            Some("https://api.github.com/repos/AgentWorkforce/relay-desktop-releases/releases/latest".to_owned())
        );
        assert_eq!(
            Profile::DEFAULT.releases_url(),
            Some(
                "https://api.github.com/repos/AgentWorkforce/relay-desktop-releases/releases"
                    .to_owned()
            )
        );
        assert_eq!(
            Profile::DEFAULT.releases_page(),
            Some("https://github.com/AgentWorkforce/relay-desktop-releases/releases".to_owned())
        );
        assert_eq!(
            Profile::DEFAULT.asset_base("2026.10.9", "Windows", "arm64"),
            "AgentRelay-2026.10.9-Windows-arm64"
        );
        assert!(!Profile::DEFAULT.names_in_text());
    }

    #[test]
    fn a_profile_build_carries_its_id_and_the_default_carries_none() {
        // A branded build is identified in About but carries no badge.
        assert!(OTHER.is_branded());
        assert_eq!(OTHER.identity(), Some("example-7"));
        assert_eq!(OTHER.badge(), None);
        assert_eq!(OTHER.window_title(), "Example Desktop");
        assert_eq!(CLOUD.identity(), Some("example-7"));
        assert_eq!(CLOUD.badge(), None);
        assert_eq!(Profile::DEFAULT.badge(), None);
        assert_eq!(Profile::DEFAULT.identity(), None);
        let dev = Profile {
            product_name: DEVELOPMENT_PRODUCT_NAME,
            ..OTHER
        };
        assert!(!dev.is_branded());
        assert_eq!(dev.window_title(), "Agent Relay Dev (example-7)");
        assert_eq!(Profile::DEFAULT.window_title(), "Agent Relay");
        // A profile that kept the default's name still shows its id.
        let same_name = Profile {
            product_name: DEFAULT_PRODUCT_NAME,
            ..OTHER
        };
        assert!(same_name.names_in_text());
        assert_eq!(same_name.window_title(), "Agent Relay (example-7)");
        if !PROFILE_ID.is_empty() {
            assert_eq!(Profile::BUILT.identity(), Some(PROFILE_ID));
        }
    }
}
