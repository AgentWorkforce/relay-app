//! Analytics preferences and durable telemetry file writes.

use super::*;

impl Telemetry {
    /// Persists the choice. Turning analytics off discards queued events and
    /// forgets the installation id, so turning it back on starts a new one;
    /// while an install is attached, that one is merged into its user again.
    pub fn set_enabled(&self, enabled: bool) -> std::io::Result<()> {
        self.set_enabled_inner(
            enabled,
            #[cfg(target_os = "macos")]
            macos_identity::sync_directory,
        )
    }

    pub(super) fn set_enabled_inner(
        &self,
        enabled: bool,
        #[cfg(target_os = "macos")] sync_preferences: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        let mut state = self.shared.lock();
        #[cfg(target_os = "macos")]
        let _disk_lock = macos_identity::lock(&self.config.app_data_dir)?;
        #[cfg(target_os = "macos")]
        let (mut preferences, disk_consent) =
            macos_identity::preferences_for_update(&self.preferences)?;
        #[cfg(not(target_os = "macos"))]
        let mut preferences = read_preferences(&self.preferences);
        #[cfg(target_os = "macos")]
        if enabled && (!state.enabled || !disk_consent) {
            state.enabled = false;
            self.rotate_locked(&mut state)?;
        }
        preferences.insert(SETTING.into(), Value::Bool(enabled));
        let mut bytes = serde_json::to_vec(&Value::Object(preferences))?;
        bytes.push(b'\n');
        #[cfg(target_os = "macos")]
        let durability = {
            persist_file(&self.preferences, &bytes)?;
            let result = sync_preferences(&self.config.app_data_dir);
            if enabled && result.is_err() {
                return result;
            }
            // The opt-out is already visible after rename, even if fsync failed.
            result
        };
        #[cfg(not(target_os = "macos"))]
        write_file(&self.preferences, &bytes)?;
        state.enabled = enabled;
        let result = if enabled {
            Ok(())
        } else {
            state.queue.clear();
            #[cfg(target_os = "macos")]
            {
                self.rotate_locked(&mut state)
            }
            #[cfg(not(target_os = "macos"))]
            {
                self.rotate(&mut state);
                Ok(())
            }
        };
        self.shared.changed.notify_all();
        #[cfg(target_os = "macos")]
        let result = durability.and(result);
        result
    }

    /// A new installation id, with nothing merged and nothing queued for the
    /// old one.
    pub(super) fn rotate(&self, state: &mut State) {
        let result = (|| {
            #[cfg(target_os = "macos")]
            let _disk_lock = macos_identity::lock(&self.config.app_data_dir)?;
            self.rotate_locked(state)
        })();
        if let Err(error) = result {
            #[cfg(target_os = "macos")]
            {
                state.identity_generation = state.identity_generation.wrapping_add(1);
                state.identity = None;
                state.queue.clear();
                state.enabled = false;
            }
            agent_relay_node::relay::diagnostics::write(format_args!(
                "relay: could not remove a telemetry id: {error}"
            ));
        }
    }

    fn rotate_locked(&self, state: &mut State) -> std::io::Result<()> {
        state.identity = None;
        state.identified = None;
        state.identifying = false;
        state.recent_errors.clear();
        state.queue.retain(|queued| queued.merges.is_none());
        #[cfg(target_os = "macos")]
        {
            state.identity_generation = state.identity_generation.wrapping_add(1);
            state.queue.clear();
        }
        let mut result = Ok(());
        for path in [&self.identity, &self.shared.identified] {
            if let Err(error) = std::fs::remove_file(path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    result = Err(error);
                }
            }
        }
        #[cfg(target_os = "macos")]
        result.and_then(|()| macos_identity::sync_directory(&self.config.app_data_dir))?;
        #[cfg(not(target_os = "macos"))]
        result?;
        Ok(())
    }
}

pub(super) fn read_preferences(path: &Path) -> Map<String, Value> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| match value {
            Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default()
}

pub(super) fn write_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    persist_file(path, bytes)?;
    #[cfg(target_os = "macos")]
    macos_identity::sync_directory(path.parent().unwrap_or(Path::new(".")))?;
    Ok(())
}

fn persist_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    std::io::Write::write_all(&mut temporary, bytes)?;
    #[cfg(target_os = "macos")]
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}
