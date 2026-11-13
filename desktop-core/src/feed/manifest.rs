//! The signed manifest, decoded from the `manifest` member once its
//! signature has verified, with every rule of the draft's
//! `desktop-feed-manifest.schema.json` (schema 1) enforced. Asset selection
//! and the download host rule are in `select.rs`.

use serde_json::{Map, Value};

use super::strict_json::MAX_SAFE_INTEGER;
use super::FeedError;

/// The only manifest schema this client knows.
pub const SCHEMA: u64 = 1;
const MAX_NOTES_CHARS: usize = 65_536;
const MAX_DOWNLOAD_HOSTS: usize = 8;
const MAX_HOST_LEN: usize = 253;
const MAX_ASSETS: usize = 64;
const MAX_NAME_LEN: usize = 255;

/// A release channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    /// `stable`: versions `YYYY.M.N`.
    Stable,
    /// `beta`: versions `YYYY.M.N-beta.N`.
    Beta,
}

/// The platform an asset is for, named as in asset names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    /// `macOS`.
    MacOs,
    /// `Linux`.
    Linux,
    /// `Windows`.
    Windows,
}

impl Platform {
    /// The platform's name in the manifest and in asset names.
    pub fn name(self) -> &'static str {
        match self {
            Self::MacOs => "macOS",
            Self::Linux => "Linux",
            Self::Windows => "Windows",
        }
    }
}

/// The processor an asset is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    /// `arm64`.
    Arm64,
    /// `x64`.
    X64,
}

impl Arch {
    /// The architecture's name in the manifest and in asset names.
    pub fn name(self) -> &'static str {
        match self {
            Self::Arm64 => "arm64",
            Self::X64 => "x64",
        }
    }
}

/// What an asset is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `image`: a macOS disk image.
    Image,
    /// `tarball`: the Linux per-user archive.
    Tarball,
    /// `zip`: the Windows per-user archive.
    Zip,
    /// `deb`: the Debian package.
    Deb,
    /// `setup`: the Windows installer.
    Setup,
    /// `probe`: the bare core.
    Probe,
    /// `install-script`: the install script.
    InstallScript,
}

impl Kind {
    /// The kind's name in the manifest.
    pub fn name(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Tarball => "tarball",
            Self::Zip => "zip",
            Self::Deb => "deb",
            Self::Setup => "setup",
            Self::Probe => "probe",
            Self::InstallScript => "install-script",
        }
    }
}

/// One downloadable file of the release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    /// Opaque id the download endpoints take.
    pub id: String,
    /// The file name, one path segment.
    pub name: String,
    pub platform: Platform,
    pub arch: Arch,
    /// The macOS major version floor, on floored macOS assets only.
    pub os_floor: Option<u8>,
    pub kind: Kind,
    /// The exact size in bytes.
    pub size: u64,
    /// Lower-case hex SHA-256 of the bytes.
    pub sha256: String,
}

/// A release as its signed manifest describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub schema: u64,
    /// The opaque profile id.
    pub profile: String,
    pub channel: Channel,
    pub version: String,
    /// RFC 3339 UTC, whole seconds.
    pub published_at: String,
    /// Markdown release notes.
    pub notes: String,
    /// The only hosts a signed download URL may name.
    pub download_hosts: Vec<String>,
    pub assets: Vec<Asset>,
}

const MANIFEST_FIELDS: [&str; 8] = [
    "schema",
    "profile",
    "channel",
    "version",
    "published_at",
    "notes",
    "download_hosts",
    "assets",
];
const ASSET_REQUIRED: [&str; 7] = ["id", "name", "platform", "arch", "kind", "size", "sha256"];
const ASSET_OPTIONAL: [&str; 1] = ["os_floor"];

impl Manifest {
    /// Decodes and schema-checks a manifest. Cross-field rules other than
    /// the schema's own (`uniqueItems`) are checked by
    /// [`Manifest::check_unique_assets`].
    pub fn from_value(value: &Value) -> Result<Self, FeedError> {
        let object = fields(value, &MANIFEST_FIELDS, &[])?;
        let get = |name: &str| object.get(name).ok_or(FeedError::SchemaInvalid);
        let schema = get("schema")?.as_u64().filter(|&schema| schema == SCHEMA);
        let channel = match string(get("channel")?)? {
            "stable" => Channel::Stable,
            "beta" => Channel::Beta,
            _ => return Err(FeedError::SchemaInvalid),
        };
        let manifest = Self {
            schema: schema.ok_or(FeedError::SchemaInvalid)?,
            profile: checked(get("profile")?, valid_profile)?,
            channel,
            version: checked(get("version")?, |version| valid_version(version, channel))?,
            published_at: checked(get("published_at")?, valid_published_at)?,
            notes: checked(get("notes")?, |notes| {
                notes.chars().count() <= MAX_NOTES_CHARS
            })?,
            download_hosts: download_hosts(get("download_hosts")?)?,
            assets: assets(get("assets")?)?,
        };
        Ok(manifest)
    }

    /// The draft's cross-field rule: asset ids and names are unique.
    pub fn check_unique_assets(&self) -> Result<(), FeedError> {
        let mut ids = std::collections::HashSet::new();
        let mut names = std::collections::HashSet::new();
        for asset in &self.assets {
            if !ids.insert(asset.id.as_str()) || !names.insert(asset.name.as_str()) {
                return Err(FeedError::DuplicateAsset);
            }
        }
        Ok(())
    }
}

/// The object's members, if it has every `required` one and nothing but
/// those and the `optional` ones.
fn fields<'a>(
    value: &'a Value,
    required: &[&str],
    optional: &[&str],
) -> Result<&'a Map<String, Value>, FeedError> {
    let object = value.as_object().ok_or(FeedError::SchemaInvalid)?;
    let known = |name: &str| required.contains(&name) || optional.contains(&name);
    if object.keys().all(|name| known(name))
        && required.iter().all(|name| object.contains_key(*name))
    {
        Ok(object)
    } else {
        Err(FeedError::SchemaInvalid)
    }
}

fn string(value: &Value) -> Result<&str, FeedError> {
    value.as_str().ok_or(FeedError::SchemaInvalid)
}

fn checked(value: &Value, valid: impl Fn(&str) -> bool) -> Result<String, FeedError> {
    let text = string(value)?;
    if valid(text) {
        Ok(text.to_owned())
    } else {
        Err(FeedError::SchemaInvalid)
    }
}

/// `^[a-z0-9]{1,32}$`, the profile id (also the `key_id` profile part).
pub(crate) fn valid_profile(profile: &str) -> bool {
    (1..=32).contains(&profile.len())
        && profile
            .bytes()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9'))
}

/// `0|[1-9][0-9]*`.
fn plain_integer(text: &str) -> bool {
    !text.is_empty()
        && text.bytes().all(|byte| byte.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'))
}

/// `YYYY.M.N` on stable, `YYYY.M.N-beta.N` on beta.
fn valid_version(version: &str, channel: Channel) -> bool {
    let (release, beta) = match version.split_once('-') {
        Some((release, suffix)) => match suffix.strip_prefix("beta.") {
            Some(number) if plain_integer(number) => (release, true),
            _ => return false,
        },
        None => (version, false),
    };
    let mut parts = release.split('.');
    let (Some(year), Some(month), Some(patch), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    let month_ok = plain_integer(month)
        && month
            .parse::<u8>()
            .is_ok_and(|month| (1..=12).contains(&month));
    year.len() == 4
        && year.bytes().all(|byte| byte.is_ascii_digit())
        && month_ok
        && plain_integer(patch)
        && beta == (channel == Channel::Beta)
}

/// `YYYY-MM-DDTHH:MM:SSZ`, a real date and time (a leap second allowed).
fn valid_published_at(text: &str) -> bool {
    let bytes = text.as_bytes();
    let shape = bytes.len() == 20
        && bytes.iter().enumerate().all(|(index, &byte)| match index {
            4 | 7 => byte == b'-',
            10 => byte == b'T',
            13 | 16 => byte == b':',
            19 => byte == b'Z',
            _ => byte.is_ascii_digit(),
        });
    let number =
        |range: std::ops::Range<usize>| text.get(range).and_then(|part| part.parse::<u32>().ok());
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        number(0..4),
        number(5..7),
        number(8..10),
        number(11..13),
        number(14..16),
        number(17..19),
    ) else {
        return false;
    };
    let date = i32::try_from(year)
        .ok()
        .and_then(|year| chrono::NaiveDate::from_ymd_opt(year, month, day));
    shape && date.is_some() && hour <= 23 && minute <= 59 && second <= 60
}

/// A lower-case DNS name of at least two labels whose last label starts
/// with a letter, so no IP literal, port or upper case.
pub(crate) fn valid_host(host: &str) -> bool {
    let labels: Vec<&str> = host.split('.').collect();
    let label_ok = |label: &str| {
        (1..=63).contains(&label.len())
            && label
                .bytes()
                .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'-'))
            && !label.starts_with('-')
            && !label.ends_with('-')
    };
    host.len() <= MAX_HOST_LEN
        && labels.len() >= 2
        && labels.iter().all(|label| label_ok(label))
        && labels
            .last()
            .is_some_and(|last| last.starts_with(|first: char| first.is_ascii_lowercase()))
}

fn download_hosts(value: &Value) -> Result<Vec<String>, FeedError> {
    let items = value.as_array().ok_or(FeedError::SchemaInvalid)?;
    if items.len() > MAX_DOWNLOAD_HOSTS {
        return Err(FeedError::SchemaInvalid);
    }
    let hosts = items
        .iter()
        .map(|item| checked(item, valid_host))
        .collect::<Result<Vec<_>, _>>()?;
    unique(&hosts)?;
    Ok(hosts)
}

/// The schema's `uniqueItems`.
fn unique<T: PartialEq>(items: &[T]) -> Result<(), FeedError> {
    let repeated = items
        .iter()
        .enumerate()
        .any(|(index, item)| items.iter().skip(index + 1).any(|other| other == item));
    if repeated {
        Err(FeedError::SchemaInvalid)
    } else {
        Ok(())
    }
}

fn assets(value: &Value) -> Result<Vec<Asset>, FeedError> {
    let items = value.as_array().ok_or(FeedError::SchemaInvalid)?;
    if items.is_empty() || items.len() > MAX_ASSETS {
        return Err(FeedError::SchemaInvalid);
    }
    let assets = items.iter().map(asset).collect::<Result<Vec<_>, _>>()?;
    unique(&assets)?;
    Ok(assets)
}

fn asset(value: &Value) -> Result<Asset, FeedError> {
    let object = fields(value, &ASSET_REQUIRED, &ASSET_OPTIONAL)?;
    let get = |name: &str| object.get(name).ok_or(FeedError::SchemaInvalid);
    let platform = platform(string(get("platform")?)?)?;
    Ok(Asset {
        id: checked(get("id")?, valid_asset_id)?,
        name: checked(get("name")?, valid_asset_name)?,
        platform,
        arch: arch(string(get("arch")?)?)?,
        os_floor: os_floor(object.get("os_floor"), platform)?,
        kind: kind(string(get("kind")?)?)?,
        size: size(get("size")?)?,
        sha256: checked(get("sha256")?, valid_sha256)?,
    })
}

fn platform(name: &str) -> Result<Platform, FeedError> {
    Ok(match name {
        "macOS" => Platform::MacOs,
        "Linux" => Platform::Linux,
        "Windows" => Platform::Windows,
        _ => return Err(FeedError::SchemaInvalid),
    })
}

fn arch(name: &str) -> Result<Arch, FeedError> {
    Ok(match name {
        "arm64" => Arch::Arm64,
        "x64" => Arch::X64,
        _ => return Err(FeedError::SchemaInvalid),
    })
}

/// 10..=99, and (the schema's dependentSchemas) only on a macOS asset.
fn os_floor(value: Option<&Value>, platform: Platform) -> Result<Option<u8>, FeedError> {
    let Some(floor) = value else {
        return Ok(None);
    };
    if platform != Platform::MacOs {
        return Err(FeedError::SchemaInvalid);
    }
    floor
        .as_u64()
        .filter(|floor| (10..=99).contains(floor))
        .and_then(|floor| u8::try_from(floor).ok())
        .map(Some)
        .ok_or(FeedError::SchemaInvalid)
}

fn size(value: &Value) -> Result<u64, FeedError> {
    value
        .as_u64()
        .filter(|size| (1..=MAX_SAFE_INTEGER).contains(size))
        .ok_or(FeedError::SchemaInvalid)
}

/// `^[0-9a-f]{64}$`.
fn valid_sha256(hex: &str) -> bool {
    hex.len() == 64
        && hex
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn kind(name: &str) -> Result<Kind, FeedError> {
    Ok(match name {
        "image" => Kind::Image,
        "tarball" => Kind::Tarball,
        "zip" => Kind::Zip,
        "deb" => Kind::Deb,
        "setup" => Kind::Setup,
        "probe" => Kind::Probe,
        "install-script" => Kind::InstallScript,
        _ => return Err(FeedError::SchemaInvalid),
    })
}

/// `^a_[0-9A-Za-z]{1,64}$`.
fn valid_asset_id(id: &str) -> bool {
    id.strip_prefix("a_").is_some_and(|rest| {
        (1..=64).contains(&rest.len()) && rest.bytes().all(|byte| byte.is_ascii_alphanumeric())
    })
}

/// One path segment: dot-separated non-empty runs of `[A-Za-z0-9_+-]`,
/// starting with a letter or digit, at most 255 characters.
fn valid_asset_name(name: &str) -> bool {
    let run_ok = |run: &str| {
        !run.is_empty()
            && run
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'+' | b'-'))
    };
    name.len() <= MAX_NAME_LEN
        && name.starts_with(|first: char| first.is_ascii_alphanumeric())
        && name.split('.').all(run_ok)
}
