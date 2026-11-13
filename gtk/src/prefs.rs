//! The app's own preferences: what only the UI uses, like the Mac's
//! `UserDefaults`. Settings the core owns stay in the core.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SessionOpenPreference {
    #[default]
    Automatic,
    Terminal,
}

impl SessionOpenPreference {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::Terminal => "terminal",
        }
    }
}

/// How the app's windows are drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Appearance {
    /// Follows the desktop's light or dark style.
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Appearance; 3] = [Appearance::System, Appearance::Light, Appearance::Dark];

    /// The stored name.
    pub fn as_str(self) -> &'static str {
        match self {
            Appearance::System => "system",
            Appearance::Light => "light",
            Appearance::Dark => "dark",
        }
    }

    /// The appearance a stored name means; anything unknown follows the system.
    pub fn from_name(name: &str) -> Appearance {
        Appearance::ALL
            .into_iter()
            .find(|appearance| appearance.as_str() == name)
            .unwrap_or_default()
    }
}

/// How big the whole window is drawn, from a fixed ladder of percentages:
/// the Mac's View > Zoom In, Zoom Out and Actual Size, and Settings >
/// Appearance > Zoom. Text, spacing, corners and minimum sizes all scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiZoom(usize);

impl Default for UiZoom {
    fn default() -> UiZoom {
        UiZoom::ACTUAL_SIZE
    }
}

impl UiZoom {
    /// The levels, in percent. The Mac's `UIZoom.levels` is the same ladder.
    pub const LEVELS: [u32; 6] = [100, 110, 125, 150, 175, 200];
    /// 100 percent.
    pub const ACTUAL_SIZE: UiZoom = UiZoom(0);

    /// The level stored as `percent`; anything off the ladder is 100.
    pub fn from_percent(percent: i64) -> UiZoom {
        UiZoom::LEVELS
            .iter()
            .position(|level| i64::from(*level) == percent)
            .map_or(UiZoom::ACTUAL_SIZE, UiZoom)
    }

    pub fn percent(self) -> u32 {
        UiZoom::LEVELS[self.0]
    }

    /// The factor sizes are multiplied by: 1.5 at 150 percent.
    pub fn factor(self) -> f64 {
        f64::from(self.percent()) / 100.0
    }

    pub fn is_actual_size(self) -> bool {
        self == UiZoom::ACTUAL_SIZE
    }

    /// One step up the ladder; the top stays the top.
    pub fn zoomed_in(self) -> UiZoom {
        UiZoom((self.0 + 1).min(UiZoom::LEVELS.len() - 1))
    }

    /// One step down the ladder; 100 percent stays 100.
    pub fn zoomed_out(self) -> UiZoom {
        UiZoom(self.0.saturating_sub(1))
    }

    pub fn can_zoom_in(self) -> bool {
        self.zoomed_in() != self
    }

    pub fn can_zoom_out(self) -> bool {
        self.zoomed_out() != self
    }

    /// `pixels` at this level, rounded to a whole pixel. Negative values
    /// (GTK's "unset" -1) are left alone.
    pub fn scale(self, pixels: i32) -> i32 {
        if pixels <= 0 || self.is_actual_size() {
            return pixels;
        }
        (f64::from(pixels) * self.factor()).round() as i32
    }

    /// "125%".
    pub fn label(self) -> String {
        format!("{}%", self.percent())
    }
}

/// The Mac's `UserDefaults` key for the zoom level, in percent.
pub const UI_ZOOM: &str = "uiZoom";
/// The Mac's `UserDefaults` key for the same choice.
pub const CAPY_SETUP_DISMISSED: &str = "capySetupDismissed";
/// The Mac's `UserDefaults` key for Not now on the Devin card.
pub const DEVIN_SETUP_DISMISSED: &str = "devinSetupDismissed";
/// The Mac's `UserDefaults` key for Keep agents connected after quitting.
pub const KEEP_AGENTS_CONNECTED: &str = "keepAgentsConnectedAfterQuit";

/// `$XDG_CONFIG_HOME/agent-relay/desktop.json`, else under `~/.config`.
#[cfg(not(windows))]
pub fn default_path() -> io::Result<PathBuf> {
    let home = agent_relay_node::relay::paths::home().map_err(io::Error::other)?;
    Ok(path_in(std::env::var_os("XDG_CONFIG_HOME"), &home))
}

/// `%APPDATA%\agent-relay\desktop.json`, Windows' place for a person's
/// settings (`XDG_CONFIG_HOME`'s counterpart); without `APPDATA`, under the
/// home's `AppData\Roaming`.
#[cfg(windows)]
pub fn default_path() -> io::Result<PathBuf> {
    let app_data = std::env::var_os("APPDATA").filter(|value| Path::new(value).is_absolute());
    let config = match app_data {
        Some(app_data) => app_data,
        None => agent_relay_node::relay::paths::home()
            .map_err(io::Error::other)?
            .join(r"AppData\Roaming")
            .into_os_string(),
    };
    Ok(path_in(Some(config), Path::new("")))
}

/// [`default_path`] for a given `XDG_CONFIG_HOME` and home directory.
pub fn path_in(config_home: Option<OsString>, home: &Path) -> PathBuf {
    let base = config_home
        .and_then(|value| {
            let trimmed = value.to_string_lossy().trim().to_owned();
            (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
        })
        .unwrap_or_else(|| home.join(".config"));
    base.join("agent-relay").join("desktop.json")
}

/// The preferences file and its values. A missing or unreadable file reads
/// as empty, so every preference has its default.
#[derive(Debug)]
pub struct Preferences {
    path: PathBuf,
    values: Map<String, Value>,
}

impl Preferences {
    pub fn session_opening(&self, source: &str) -> SessionOpenPreference {
        match self
            .get(&format!("sessionOpening.{source}"))
            .and_then(Value::as_str)
        {
            Some("terminal") => SessionOpenPreference::Terminal,
            _ => SessionOpenPreference::Automatic,
        }
    }

    pub fn set_session_opening(
        &mut self,
        source: &str,
        preference: SessionOpenPreference,
    ) -> io::Result<()> {
        self.set(
            &format!("sessionOpening.{source}"),
            Value::from(preference.as_str()),
        )
    }

    pub fn load(path: PathBuf) -> Preferences {
        let values = fs::read(&path)
            .ok()
            .and_then(|data| serde_json::from_slice::<Value>(&data).ok())
            .and_then(|value| match value {
                Value::Object(values) => Some(values),
                _ => None,
            })
            .unwrap_or_default();
        Preferences { path, values }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    /// Stores `value` and writes the file atomically. The value holds for
    /// this run even when the file cannot be written.
    pub fn set(&mut self, key: &str, value: Value) -> io::Result<()> {
        self.values.insert(key.to_owned(), value);
        self.save()
    }

    fn save(&self) -> io::Result<()> {
        if let Some(directory) = self.path.parent() {
            fs::create_dir_all(directory)?;
        }
        let sorted: std::collections::BTreeMap<&String, &Value> = self.values.iter().collect();
        let text = serde_json::to_string_pretty(&sorted).map_err(io::Error::other)?;
        let temporary = self.path.with_extension("tmp");
        fs::write(&temporary, text)?;
        fs::rename(&temporary, &self.path)
    }

    pub fn appearance(&self) -> Appearance {
        match self.get("appearance") {
            Some(Value::String(name)) => Appearance::from_name(name),
            _ => Appearance::System,
        }
    }

    pub fn set_appearance(&mut self, appearance: Appearance) -> io::Result<()> {
        self.set("appearance", Value::from(appearance.as_str()))
    }

    /// The zoom level; a missing, non-integer or off-ladder value is 100
    /// percent.
    pub fn ui_zoom(&self) -> UiZoom {
        self.get(UI_ZOOM)
            .and_then(Value::as_i64)
            .map_or(UiZoom::ACTUAL_SIZE, UiZoom::from_percent)
    }

    pub fn set_ui_zoom(&mut self, zoom: UiZoom) -> io::Result<()> {
        self.set(UI_ZOOM, Value::from(zoom.percent()))
    }

    /// On unless turned off.
    pub fn automatically_checks_for_updates(&self) -> bool {
        self.get("automatically_checks_for_updates")
            .is_none_or(super::truthy)
    }

    pub fn set_automatically_checks_for_updates(&mut self, value: bool) -> io::Result<()> {
        self.set("automatically_checks_for_updates", Value::Bool(value))
    }

    /// Beta updates require an explicit opt-in, including on a beta build.
    pub fn include_beta_releases(&self) -> bool {
        self.get("include_beta_releases").and_then(Value::as_bool) == Some(true)
    }

    pub fn set_include_beta_releases(&mut self, value: bool) -> io::Result<()> {
        self.set("include_beta_releases", Value::Bool(value))
    }

    /// Quitting leaves the core running unless turned off.
    pub fn keeps_agents_connected_after_quit(&self) -> bool {
        self.get(KEEP_AGENTS_CONNECTED).is_none_or(super::truthy)
    }

    pub fn set_keeps_agents_connected_after_quit(&mut self, value: bool) -> io::Result<()> {
        self.set(KEEP_AGENTS_CONNECTED, Value::Bool(value))
    }

    /// The person chose Not now on the Capy setup card.
    pub fn capy_setup_dismissed(&self) -> bool {
        self.get(CAPY_SETUP_DISMISSED).is_some_and(super::truthy)
    }

    pub fn dismiss_capy_setup(&mut self) -> io::Result<()> {
        self.set(CAPY_SETUP_DISMISSED, Value::Bool(true))
    }

    /// The person chose Not now on the Devin card.
    pub fn devin_setup_dismissed(&self) -> bool {
        self.get(DEVIN_SETUP_DISMISSED).is_some_and(super::truthy)
    }

    pub fn dismiss_devin_setup(&mut self) -> io::Result<()> {
        self.set(DEVIN_SETUP_DISMISSED, Value::Bool(true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn capy_setup_dismissal_persists() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("desktop.json");
        let mut prefs = Preferences::load(path.clone());
        assert!(!prefs.capy_setup_dismissed());
        prefs.dismiss_capy_setup().unwrap();
        assert!(Preferences::load(path).capy_setup_dismissed());
    }

    #[test]
    fn devin_setup_dismissal_persists_apart_from_capy() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("desktop.json");
        let mut prefs = Preferences::load(path.clone());
        assert!(!prefs.devin_setup_dismissed());
        prefs.dismiss_devin_setup().unwrap();
        let again = Preferences::load(path);
        assert!(again.devin_setup_dismissed());
        assert!(!again.capy_setup_dismissed());
    }

    #[test]
    fn beta_updates_require_opt_in_and_persist() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("desktop.json");
        let mut prefs = Preferences::load(path.clone());
        assert!(!prefs.include_beta_releases());
        prefs.set_include_beta_releases(true).unwrap();
        assert!(Preferences::load(path.clone()).include_beta_releases());
        prefs.set_include_beta_releases(false).unwrap();
        assert!(!Preferences::load(path.clone()).include_beta_releases());
        prefs
            .set("include_beta_releases", Value::from("true"))
            .unwrap();
        assert!(!Preferences::load(path).include_beta_releases());
    }

    #[test]
    fn session_opening_choices_persist_per_provider() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("desktop.json");
        let mut prefs = Preferences::load(path.clone());
        assert_eq!(
            prefs.session_opening("claude"),
            SessionOpenPreference::Automatic
        );
        prefs
            .set_session_opening("claude", SessionOpenPreference::Terminal)
            .unwrap();
        let again = Preferences::load(path);
        assert_eq!(
            again.session_opening("claude"),
            SessionOpenPreference::Terminal
        );
        assert_eq!(
            again.session_opening("codex"),
            SessionOpenPreference::Automatic
        );
    }

    #[test]
    fn choices_persist_and_unknown_values_fall_back() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("desktop.json");
        let mut prefs = Preferences::load(path.clone());
        assert_eq!(prefs.appearance(), Appearance::System);
        assert!(prefs.automatically_checks_for_updates());
        prefs.set_appearance(Appearance::Dark).unwrap();
        prefs.set_automatically_checks_for_updates(false).unwrap();
        let again = Preferences::load(path.clone());
        assert_eq!(
            (again.appearance(), again.automatically_checks_for_updates()),
            (Appearance::Dark, false)
        );
        prefs.set("appearance", Value::from("purple")).unwrap();
        assert_eq!(
            Preferences::load(path.clone()).appearance(),
            Appearance::System
        );
        fs::write(&path, "not json").unwrap();
        assert_eq!(
            Preferences::load(path.clone()).appearance(),
            Appearance::System
        );
        assert!(!directory.path().join("desktop.tmp").exists());
    }

    #[test]
    fn the_zoom_ladder_steps_clamps_and_resets() {
        let percents = |mut zoom: UiZoom, step: fn(UiZoom) -> UiZoom| {
            let mut seen = vec![zoom.percent()];
            for _ in 0..7 {
                zoom = step(zoom);
                seen.push(zoom.percent());
            }
            seen
        };
        assert_eq!(
            percents(UiZoom::ACTUAL_SIZE, UiZoom::zoomed_in),
            [100, 110, 125, 150, 175, 200, 200, 200]
        );
        let top = UiZoom::from_percent(200);
        assert_eq!(
            percents(top, UiZoom::zoomed_out),
            [200, 175, 150, 125, 110, 100, 100, 100]
        );
        assert!(!top.can_zoom_in() && top.can_zoom_out());
        assert!(UiZoom::ACTUAL_SIZE.can_zoom_in() && !UiZoom::ACTUAL_SIZE.can_zoom_out());
        assert_eq!(UiZoom::default(), UiZoom::ACTUAL_SIZE);
        assert!(UiZoom::default().is_actual_size() && !top.is_actual_size());
        assert_eq!(UiZoom::from_percent(125).label(), "125%");
    }

    #[test]
    fn stored_zoom_levels_map_to_the_ladder_or_fall_back_to_100() {
        for percent in UiZoom::LEVELS {
            assert_eq!(UiZoom::from_percent(i64::from(percent)).percent(), percent);
        }
        for unknown in [0, 99, 120, 300, -100] {
            assert_eq!(UiZoom::from_percent(unknown), UiZoom::ACTUAL_SIZE);
        }
    }

    #[test]
    fn zoom_scales_whole_pixels_and_leaves_unset_sizes() {
        let at = UiZoom::from_percent;
        assert_eq!(at(150).factor(), 1.5);
        assert_eq!(
            (at(100).scale(13), at(150).scale(12), at(200).scale(13)),
            (13, 18, 26)
        );
        assert_eq!(
            (at(110).scale(13), at(125).scale(6), at(175).scale(7)),
            (14, 8, 12)
        );
        assert_eq!((at(200).scale(-1), at(200).scale(0)), (-1, 0));
    }

    #[test]
    fn the_zoom_level_persists_and_bad_values_read_as_100() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("desktop.json");
        let mut prefs = Preferences::load(path.clone());
        assert_eq!(prefs.ui_zoom(), UiZoom::ACTUAL_SIZE);
        prefs.set_ui_zoom(UiZoom::from_percent(150)).unwrap();
        assert_eq!(Preferences::load(path.clone()).ui_zoom().percent(), 150);
        assert!(fs::read_to_string(&path)
            .unwrap()
            .contains("\"uiZoom\": 150"));
        for stored in [json!(130), json!("150"), json!(150.5), json!(null)] {
            prefs.set(UI_ZOOM, stored).unwrap();
            assert_eq!(
                Preferences::load(path.clone()).ui_zoom(),
                UiZoom::ACTUAL_SIZE
            );
        }
    }

    #[test]
    fn the_file_is_sorted_two_space_json() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/desktop.json");
        let mut prefs = Preferences::load(path.clone());
        prefs.set_automatically_checks_for_updates(true).unwrap();
        prefs.set_appearance(Appearance::Light).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "{\n  \"appearance\": \"light\",\n  \"automatically_checks_for_updates\": true\n}"
        );
    }

    #[test]
    fn the_path_honors_xdg_config_home() {
        let home = Path::new("/home/a");
        assert_eq!(
            path_in(None, home),
            PathBuf::from("/home/a/.config/agent-relay/desktop.json")
        );
        assert_eq!(
            path_in(Some(" ".into()), home),
            PathBuf::from("/home/a/.config/agent-relay/desktop.json")
        );
        assert_eq!(
            path_in(Some("/cfg".into()), home),
            PathBuf::from("/cfg/agent-relay/desktop.json")
        );
    }
}
