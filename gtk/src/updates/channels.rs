//! Release eligibility and selection for stable and beta subscribers, from
//! the GitHub repository the build's site profile names. A build with a
//! cloud release feed reads its site's signed feed instead (`cloud.rs`); it
//! has no repository, so nothing here makes a request for it.
use super::*;
use crate::build_profile::Profile;

fn eligible(value: &Value, include_beta: bool) -> bool {
    if value.get("draft").is_some_and(crate::truthy) {
        return false;
    }
    let tag = text(value.get("tag_name"));
    let Some((_, label)) = version(tag.strip_prefix('v').unwrap_or(&tag)) else {
        return false;
    };
    if !value.get("prerelease").is_some_and(crate::truthy) {
        return label.is_none();
    }
    include_beta
        && label
            .and_then(|label| label.strip_prefix("beta."))
            .is_some_and(|number| !number.is_empty() && number.bytes().all(|c| c.is_ascii_digit()))
}

/// `profile`'s release for `arch` on `platform`: the assets named with its
/// prefix.
pub(super) fn parse_release_for_channel(
    profile: &Profile,
    data: &[u8],
    arch: &str,
    platform: Platform,
    include_beta: bool,
) -> Result<Release, UpdateError> {
    let unreadable = || UpdateError::new(UNREADABLE);
    let value: Value = serde_json::from_slice(data).map_err(|_| unreadable())?;
    if !eligible(&value, include_beta) {
        return Err(unreadable());
    }
    let Value::Object(value) = value else {
        return Err(unreadable());
    };
    let tag = text(value.get("tag_name"));
    let version = tag.strip_prefix('v').unwrap_or(&tag);
    if self::version(version).is_none() {
        return Err(unreadable());
    }
    let mut assets = HashMap::new();
    if let Some(Value::Array(items)) = value.get("assets") {
        for item in items.iter().filter_map(Value::as_object) {
            assets.insert(
                text(item.get("name")),
                text(item.get("browser_download_url")),
            );
        }
    }
    let asset = |name: &str| -> Result<Option<String>, UpdateError> {
        match assets.get(name).filter(|url| !url.is_empty()) {
            None => Ok(None),
            Some(url) if on_github(url) => Ok(Some(url.clone())),
            Some(_) => Err(unreadable()),
        }
    };
    let required = |name: &str| asset(name)?.ok_or_else(|| UpdateError::new(NO_COMPATIBLE_UPDATE));
    let base = profile.asset_base(version, platform.name(), arch);
    let archive = platform.archive();
    let package = |name: &str| match platform {
        Platform::Linux => asset(name),
        Platform::Windows => Ok(None),
    };
    Ok(Release {
        version: version.to_owned(),
        tarball_url: required(&format!("{base}.{archive}"))?,
        tarball_checksum_url: required(&format!("{base}.{archive}.sha256"))?,
        deb_url: package(&format!("{base}.deb"))?,
        deb_checksum_url: package(&format!("{base}.deb.sha256"))?,
        page_url: text(value.get("html_url")),
        feed: None,
    })
}

/// [`latest`](super::latest) from `profile`'s repository.
pub(super) fn latest_in(
    profile: &Profile,
    arch: &str,
    fetch: &Fetch<'_>,
) -> Result<Release, UpdateError> {
    let url = profile
        .latest_release_url()
        .ok_or_else(|| UpdateError::new(UNREADABLE))?;
    let data = fetch(&url).map_err(|error| match error {
        FetchError::TooLarge => UpdateError::new(TOO_LARGE),
        FetchError::Unreachable(_) => UpdateError::new(UNREADABLE),
    })?;
    parse_release_for_channel(profile, &data, arch, Platform::CURRENT, false)
}

/// Beta subscribers also receive stable releases. Scan every page, choosing by
/// version so an older hotfix published later cannot hide a newer beta.
pub fn latest_for_channel(
    arch: &str,
    include_beta: bool,
    fetch: &Fetch<'_>,
) -> Result<Release, UpdateError> {
    latest_for_channel_in(&Profile::BUILT, arch, include_beta, fetch)
}

/// [`latest_for_channel`] from `profile`'s repository.
pub(super) fn latest_for_channel_in(
    profile: &Profile,
    arch: &str,
    include_beta: bool,
    fetch: &Fetch<'_>,
) -> Result<Release, UpdateError> {
    if !include_beta {
        return latest_in(profile, arch, fetch);
    }
    let releases = profile
        .releases_url()
        .ok_or_else(|| UpdateError::new(UNREADABLE))?;
    let mut newest: Option<Value> = None;
    for page in 1.. {
        let data = fetch(&format!("{releases}?per_page=100&page={page}")).map_err(|error| {
            UpdateError::new(match error {
                FetchError::TooLarge => TOO_LARGE,
                FetchError::Unreachable(_) => UNREADABLE,
            })
        })?;
        let releases: Vec<Value> =
            serde_json::from_slice(&data).map_err(|_| UpdateError::new(UNREADABLE))?;
        let count = releases.len();
        for release in releases.into_iter().filter(|value| eligible(value, true)) {
            let tag = text(release.get("tag_name"));
            if newest.as_ref().is_none_or(|old| {
                let old_tag = text(old.get("tag_name"));
                is_newer(tag.trim_start_matches('v'), old_tag.trim_start_matches('v'))
            }) {
                newest = Some(release);
            }
        }
        if count < 100 {
            break;
        }
    }
    let newest = newest.ok_or_else(|| UpdateError::new(UNREADABLE))?;
    let data = serde_json::to_vec(&newest).map_err(|_| UpdateError::new(UNREADABLE))?;
    parse_release_for_channel(profile, &data, arch, Platform::CURRENT, true)
}

#[cfg(test)]
mod tests {
    use super::super::release_channel_tests::release_of;
    use super::*;
    use crate::build_profile::tests::{CLOUD, OTHER};

    #[test]
    fn the_github_reader_makes_no_request_for_a_cloud_feed() {
        let calls = std::cell::RefCell::new(Vec::<String>::new());
        let fetch = |url: &str| -> Result<Vec<u8>, FetchError> {
            calls.borrow_mut().push(url.to_owned());
            Err(FetchError::Unreachable("no network".into()))
        };
        for include_beta in [false, true] {
            assert_eq!(
                latest_for_channel_in(&CLOUD, "x64", include_beta, &fetch).unwrap_err(),
                UpdateError::new(UNREADABLE)
            );
        }
        assert_eq!(
            latest_in(&CLOUD, "x64", &fetch).unwrap_err(),
            UpdateError::new(UNREADABLE)
        );
        assert!(calls.borrow().is_empty(), "{:?}", calls.borrow());
    }

    #[test]
    fn a_profile_reads_its_own_repository_for_its_own_assets() {
        for (profile, prefix, repository) in [
            (
                OTHER,
                "Desktop-example-7-",
                "example-org/example-desktop-releases",
            ),
            (
                Profile::DEFAULT,
                "AgentRelay-",
                "AgentWorkforce/relay-desktop-releases",
            ),
        ] {
            let other = if profile == OTHER {
                Profile::DEFAULT
            } else {
                OTHER
            };
            let stable = serde_json::to_vec(&release_of(&profile, "2026.10.2", false)).unwrap();
            for platform in [Platform::Linux, Platform::Windows] {
                for arch in ["arm64", "x64"] {
                    let found = parse_release_for_channel(&profile, &stable, arch, platform, false)
                        .unwrap();
                    let name = found.tarball_url.rsplit('/').next().unwrap();
                    assert_eq!(
                        name,
                        format!(
                            "{prefix}2026.10.2-{}-{arch}.{}",
                            platform.name(),
                            platform.archive()
                        )
                    );
                    assert!(found.tarball_url.starts_with(&format!(
                        "https://github.com/{repository}/releases/download/"
                    )));
                    // Another profile's images are not this profile's update.
                    assert_eq!(
                        parse_release_for_channel(&other, &stable, arch, platform, false)
                            .unwrap_err()
                            .to_string(),
                        NO_COMPATIBLE_UPDATE
                    );
                }
            }
            let calls = std::cell::RefCell::new(Vec::new());
            let found = latest_in(&profile, "x64", &|url| {
                calls.borrow_mut().push(url.to_owned());
                Ok(stable.clone())
            })
            .unwrap();
            assert_eq!(found.version, "2026.10.2");
            let beta = release_of(&profile, "2026.10.3-beta.1", true);
            let selected = latest_for_channel_in(&profile, "x64", true, &|url| {
                calls.borrow_mut().push(url.to_owned());
                Ok(serde_json::to_vec(&vec![beta.clone()]).unwrap())
            })
            .unwrap();
            assert_eq!(selected.version, "2026.10.3-beta.1");
            assert!(selected
                .tarball_url
                .rsplit('/')
                .next()
                .unwrap()
                .starts_with(prefix));
            assert_eq!(
                *calls.borrow(),
                [
                    format!("https://api.github.com/repos/{repository}/releases/latest"),
                    format!(
                        "https://api.github.com/repos/{repository}/releases?per_page=100&page=1"
                    ),
                ]
            );
        }
    }
}
