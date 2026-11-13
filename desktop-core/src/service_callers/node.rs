//! This computer's own spawn node, read from its enrolment on every call
//! (`docs/service-callers.md` section 8, `identity`). Never cached.

use super::host::Host;
use super::Class;
use crate::contract::service_callers::CallerNode;
use agent_relay_node::relay::spawn::node::NODE_FILE;
use agent_relay_node::relay::workspace::{Secret, Workspace};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The fields of `spawn-node.json` this check reads.
#[derive(Deserialize)]
struct Enrolment {
    #[serde(rename = "nodeID")]
    node_id: String,
    #[serde(rename = "nodeName")]
    node_name: String,
    token: Secret,
    #[serde(rename = "baseURL")]
    base_url: String,
    workspace: String,
}

/// The node `spawn.invoke` accepts: enrolled at this workspace's origin,
/// with this workspace's key, under this computer's node name or an
/// alternate of it. Anything else, or nothing, is `node_unavailable`.
pub(crate) fn enrolled(host: &dyn Host, workspace: &Workspace) -> Result<CallerNode, Class> {
    let app_data = host.app_data_dir().ok_or(Class::NodeUnavailable)?;
    let prefix = host.machine_prefix().ok_or(Class::NodeUnavailable)?;
    let base = agent_relay_node::relay::spawn::node_name(&prefix);
    let bytes = std::fs::read(app_data.join(NODE_FILE)).map_err(|_| Class::NodeUnavailable)?;
    let enrolment: Enrolment =
        serde_json::from_slice(&bytes).map_err(|_| Class::NodeUnavailable)?;
    let well_formed = |value: &str| {
        !value.is_empty()
            && value.len() <= 200
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    };
    if enrolment.base_url != workspace.base_url
        || enrolment.workspace != fingerprint(&workspace.key)
        || !named_for(&enrolment.node_name, &base)
        || !well_formed(&enrolment.node_id)
        || !well_formed(&enrolment.node_name)
        || enrolment.token.expose().is_empty()
    {
        return Err(Class::NodeUnavailable);
    }
    Ok(CallerNode {
        id: enrolment.node_id,
        name: enrolment.node_name,
    })
}

/// The workspace key's digest as `spawn-node.json` records it.
pub(crate) fn fingerprint(key: &Secret) -> String {
    let digest = Sha256::digest(key.expose().as_bytes());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// `base`, or `base` and a four-character lowercase hex alternate suffix.
fn named_for(name: &str, base: &str) -> bool {
    name == base
        || name
            .strip_prefix(base)
            .and_then(|rest| rest.strip_prefix('-'))
            .is_some_and(|suffix| {
                suffix.len() == 4
                    && suffix
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
}
