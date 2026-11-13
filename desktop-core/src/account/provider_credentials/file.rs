use super::*;
use agent_relay_node::platform::fs as private_fs;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Read, Write},
    path::Path,
};

#[derive(Serialize, Deserialize)]
pub(super) struct Credentials {
    site: String,
    account: String,
    workspace: String,
    pub installation_id: String,
    pub grants: BTreeMap<String, Grant>,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Grant {
    pub token: Secret,
    pub id: Option<String>,
    pub lease_expires: i64,
    #[serde(default)]
    pub retry_after: i64,
    #[serde(default)]
    pub needs_app_login: bool,
}
impl Grant {
    pub fn new() -> io::Result<Self> {
        let mut bytes = [0; 32];
        getrandom::getrandom(&mut bytes)
            .map_err(|_| io::Error::other("Credential creation failed"))?;
        Ok(Self {
            token: Secret::new(format!(
                "cld_at_{}",
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
            )),
            id: None,
            lease_expires: 0,
            retry_after: 0,
            needs_app_login: false,
        })
    }
}
impl Credentials {
    pub fn publish(
        &self,
        installer: &Installer,
        binding: &Binding,
        selected: &BTreeSet<String>,
        now: i64,
    ) {
        installer.set_provider_credentials(binding, self.active(selected, now));
        installer.set_provider_login_required(
            binding,
            self.grants
                .iter()
                .filter(|(id, g)| selected.contains(*id) && g.needs_app_login)
                .map(|(id, _)| id.clone())
                .collect(),
        );
    }
    pub fn needs_network(&self, selected: &BTreeSet<String>, now: i64) -> bool {
        selected.iter().any(|id| !self.grants.contains_key(id))
            || self.grants.iter().any(|(id, grant)| {
                grant.retry_after <= now
                    && (!selected.contains(id)
                        || grant.id.is_none()
                        || grant.lease_expires <= now + 600_000)
            })
    }
    pub fn apply(&mut self, answers: Vec<(String, remote::Change)>, now: i64) {
        for (provider, result) in answers {
            match result {
                remote::Change::Updated(grant) => {
                    self.grants.insert(provider, grant);
                }
                remote::Change::Remove => {
                    self.grants.remove(&provider);
                }
                remote::Change::Rejected => {
                    // Do not serve or renew a credential the server refused:
                    // its parent login may be gone, so renewing the same id
                    // would fail forever. Fresh material lets the next pass
                    // (or a repair) issue a grant bound to the current login.
                    if self.grants.contains_key(&provider) {
                        match Grant::new() {
                            Ok(mut fresh) => {
                                fresh.retry_after = now + 60_000;
                                self.grants.insert(provider, fresh);
                            }
                            Err(_) => {
                                self.grants.remove(&provider);
                            }
                        }
                    }
                }
                remote::Change::SignInRequired => {
                    // Kept until a new app login renews it; not served meanwhile.
                    if let Some(grant) = self.grants.get_mut(&provider) {
                        grant.needs_app_login = true;
                        grant.lease_expires = 0;
                        grant.retry_after = now + 60_000;
                    }
                }
                remote::Change::Retry => {
                    if let Some(grant) = self.grants.get_mut(&provider) {
                        grant.retry_after = now + 60_000;
                    }
                }
            }
        }
    }
    fn new(binding: &Binding) -> Self {
        Self {
            site: binding.site_url.clone(),
            account: binding.account_id.clone(),
            workspace: binding.workspace_id.clone(),
            installation_id: agent_relay_node::relay::store::random_uuid(),
            grants: BTreeMap::new(),
        }
    }
    pub fn load(path: &Path, binding: &Binding) -> io::Result<Self> {
        let mut options = fs::OpenOptions::new();
        options.read(true);
        let mut file = match private_fs::no_follow(&mut options).open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::new(binding)),
            Err(error) => return Err(error),
        };
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || !private_fs::owned_by_current_user(&metadata)
            || !private_fs::private_to_owner(&metadata, 0o077)
            || metadata.len() > 64 * 1024
        {
            return Err(io::Error::other("Unsafe provider credential file"));
        }
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;
        let saved: Self = serde_json::from_slice(&data)
            .map_err(|_| io::Error::other("Invalid provider credential file"))?;
        if saved.site != binding.site_url
            || saved.account != binding.account_id
            || saved.workspace != binding.workspace_id
        {
            // Old account grants cannot authorize this one and expire without
            // renewal. Never copy them across origin/account/workspace changes.
            return Ok(Self::new(binding));
        }
        if !agent_relay_node::relay::store::is_uuid(&saved.installation_id) {
            return Err(io::Error::other("Invalid provider installation"));
        }
        Ok(saved)
    }
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("Missing credential directory"))?;
        private_fs::create_private_dir_all(parent)?;
        let metadata = fs::symlink_metadata(parent)?;
        if !metadata.is_dir()
            || !private_fs::owned_by_current_user(&metadata)
            || !private_fs::private_to_owner(&metadata, 0o022)
        {
            return Err(io::Error::other("Unsafe credential directory"));
        }
        if let Ok(meta) = fs::symlink_metadata(path) {
            if !meta.is_file()
                || !private_fs::owned_by_current_user(&meta)
                || !private_fs::private_to_owner(&meta, 0o077)
            {
                return Err(io::Error::other("Unsafe provider credential file"));
            }
        }
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        private_fs::set_mode(temporary.path(), 0o600)?;
        let bytes = serde_json::to_vec(self)
            .map_err(|_| io::Error::other("Credential serialization failed"))?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary.persist(path).map_err(|e| e.error)?;
        private_fs::sync_dir(parent)
    }
    pub fn active(&self, selected: &BTreeSet<String>, now: i64) -> BTreeMap<String, String> {
        self.grants
            .iter()
            .filter(|(id, g)| {
                selected.contains(*id) && g.id.is_some() && g.lease_expires > now + 15_000
            })
            .map(|(id, g)| (id.clone(), g.token.expose().to_string()))
            .collect()
    }
}
