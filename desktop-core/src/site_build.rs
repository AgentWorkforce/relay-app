//! Whether this build is made for its own site: `AGENT_RELAY_SITE_URL` named
//! another site than production when the build was made
//! (`links::built_in_site_url`). Such a build uses only its own site's
//! values; where a production build falls back to a public Agent Relay host,
//! it refuses instead.

use agent_relay_node::relay::links::{built_in_site_url, DEFAULT_SITE_URL};

/// Whether this build is made for its own site.
pub fn is_site_build() -> bool {
    is_site_build_for(built_in_site_url())
}

/// [`is_site_build`] for a build made for `built_in`. The build normalizes
/// the site it records, so a production build records exactly
/// [`DEFAULT_SITE_URL`].
pub fn is_site_build_for(built_in: &str) -> bool {
    built_in != DEFAULT_SITE_URL
}

/// The build's refusal of PostHog overrides for a site build (`build.rs`),
/// compiled here so its decision is tested.
#[cfg(test)]
#[path = "../build_site_telemetry.rs"]
mod build_site_telemetry;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_another_site_than_production_makes_a_site_build() {
        assert!(!is_site_build_for(DEFAULT_SITE_URL));
        assert!(is_site_build_for("https://relay.example.test"));
        assert!(is_site_build_for("http://localhost:3100"));
        assert_eq!(is_site_build(), built_in_site_url() != DEFAULT_SITE_URL);
    }

    mod build_site_telemetry {
        use super::super::build_site_telemetry::refused_posthog_override;

        const SITE: &str = "https://site.example";
        const HOST: &str = "https://posthog.example";
        const KEY: &str = "phc_secret_value";

        fn names_both_variables(error: &str) {
            assert!(error.contains("AGENT_RELAY_POSTHOG_HOST"), "{error}");
            assert!(error.contains("AGENT_RELAY_POSTHOG_API_KEY"), "{error}");
        }

        #[test]
        fn a_site_build_refuses_a_posthog_host() {
            let error = refused_posthog_override(Some(SITE), Some(HOST), None).unwrap();
            names_both_variables(&error);
        }

        #[test]
        fn a_site_build_refuses_a_posthog_key() {
            let error = refused_posthog_override(Some(SITE), None, Some(KEY)).unwrap();
            names_both_variables(&error);
        }

        #[test]
        fn a_site_build_refuses_a_posthog_host_and_key() {
            let error = refused_posthog_override(Some(SITE), Some(HOST), Some(KEY)).unwrap();
            names_both_variables(&error);
        }

        #[test]
        fn the_refusal_names_no_value() {
            for (host, key) in [
                (Some(HOST), None),
                (None, Some(KEY)),
                (Some(HOST), Some(KEY)),
            ] {
                let error = refused_posthog_override(Some(SITE), host, key).unwrap();
                for value in [HOST, KEY, SITE, "posthog.example", "site.example"] {
                    assert!(!error.contains(value), "{value} in {error}");
                }
            }
        }

        #[test]
        fn a_site_build_without_posthog_overrides_is_accepted() {
            for (host, key) in [
                (None, None),
                (Some(""), None),
                (None, Some("")),
                (Some(""), Some("")),
            ] {
                assert_eq!(refused_posthog_override(Some(SITE), host, key), None);
            }
            // The empty values are what is accepted: the same site with
            // either value set is refused.
            assert!(refused_posthog_override(Some(SITE), Some(HOST), Some("")).is_some());
            assert!(refused_posthog_override(Some(SITE), Some(""), Some(KEY)).is_some());
        }

        #[test]
        fn a_production_build_keeps_its_posthog_host_and_key() {
            for site in ["https://agentrelay.com", "https://agentrelay.com/"] {
                assert_eq!(
                    refused_posthog_override(Some(site), Some(HOST), Some(KEY)),
                    None
                );
            }
            // Only the production site is accepted with them.
            assert!(refused_posthog_override(
                Some("https://agentrelay.com.example"),
                Some(HOST),
                None
            )
            .is_some());
        }

        #[test]
        fn a_build_without_a_site_keeps_its_posthog_host() {
            for site in [None, Some("")] {
                assert_eq!(refused_posthog_override(site, Some(HOST), None), None);
                assert_eq!(refused_posthog_override(site, Some(HOST), Some(KEY)), None);
            }
            // Naming a site is what refuses the same host.
            assert!(refused_posthog_override(Some(SITE), Some(HOST), None).is_some());
        }
    }
}
