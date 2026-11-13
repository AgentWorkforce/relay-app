//! The design's steps 3 and 4 on a verified manifest: pick the one asset
//! for this machine and check its name, then check a download URL's host.

use super::manifest::{valid_host, Arch, Asset, Kind, Manifest, Platform};
use super::FeedError;

impl Manifest {
    /// The asset for `platform`, `arch`, `kind` and `os_floor` (none for an
    /// unfloored asset). Exactly one may match, and its name must be the
    /// one a build with `asset_prefix` gives that asset of this version.
    pub fn select(
        &self,
        asset_prefix: &str,
        platform: Platform,
        arch: Arch,
        kind: Kind,
        os_floor: Option<u8>,
    ) -> Result<&Asset, FeedError> {
        let mut matching = self.assets.iter().filter(|asset| {
            asset.platform == platform
                && asset.arch == arch
                && asset.kind == kind
                && asset.os_floor == os_floor
        });
        let asset = matching.next().ok_or(FeedError::NoAsset)?;
        if matching.next().is_some() {
            return Err(FeedError::AmbiguousAsset);
        }
        if asset.name != expected_name(asset_prefix, &self.version, asset) {
            return Err(FeedError::AssetNameMismatch);
        }
        Ok(asset)
    }
}

/// The name a build with `asset_prefix` gives `asset` of `version`:
/// `<prefix>-<version>-<platform>[floor]-<arch>` with the kind's ending, or
/// the install script's fixed name.
pub fn expected_name(asset_prefix: &str, version: &str, asset: &Asset) -> String {
    let platform = match asset.os_floor {
        Some(floor) => format!("{}{floor}", asset.platform.name()),
        None => asset.platform.name().to_owned(),
    };
    let base = format!("{asset_prefix}-{version}-{platform}-{}", asset.arch.name());
    let windows = asset.platform == Platform::Windows;
    match asset.kind {
        Kind::Image => format!("{base}.dmg"),
        Kind::Tarball => format!("{base}.tar.gz"),
        Kind::Zip => format!("{base}.zip"),
        Kind::Deb => format!("{base}.deb"),
        Kind::Setup => format!("{base}-Setup.exe"),
        Kind::Probe if windows => format!("{base}-probe.zip"),
        Kind::Probe => format!("{base}-probe.tar.gz"),
        Kind::InstallScript if windows => "install.ps1".to_owned(),
        Kind::InstallScript => "install.sh".to_owned(),
    }
}

/// Whether a download may come from `url`: `https`, no user info, no
/// explicit port (not even `:443`), and a host that is one of the
/// manifest's `download_hosts` exactly or the host of `site` (a streamed
/// download).
pub fn download_host_allowed(url: &str, manifest: &Manifest, site: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    // A DNS name only: no user info, port, IP literal or percent escape.
    if !valid_host(authority) {
        return false;
    }
    // The URL as a whole must parse, to the same host.
    let parsed = url::Url::parse(url).ok();
    if parsed.as_ref().and_then(url::Url::host_str) != Some(authority) {
        return false;
    }
    let site_host = url::Url::parse(site)
        .ok()
        .filter(|site| site.scheme() == "https")
        .and_then(|site| site.host_str().map(str::to_owned));
    manifest.download_hosts.iter().any(|host| host == authority)
        || site_host.as_deref() == Some(authority)
}
