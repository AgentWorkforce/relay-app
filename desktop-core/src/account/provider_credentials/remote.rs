use super::*;
use agent_relay_node::relay::cloud_http;

pub(super) enum Change {
    Updated(Grant),
    Remove,
    Rejected,
    SignInRequired,
    Retry,
}

fn route(binding: &Binding) -> String {
    format!(
        "{}/workspaces/{}/mcp/shared-sessions-tokens",
        cloud_session::api_base(&binding.site_url),
        binding.workspace_id
    )
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() < 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
}

pub(super) async fn revoke(binding: &Binding, token: &Secret, id: &str) -> bool {
    if valid_id(id) {
        return cloud_http::send(
            "DELETE",
            format!("{}/{}", route(binding), id),
            Some(token.expose().into()),
            None,
            Duration::from_secs(5),
        )
        .await
        .is_ok_and(|answer| answer.ok() || matches!(answer.status, 404 | 410));
    }
    false
}

pub(super) async fn reconcile(
    binding: &Binding,
    token: &Secret,
    installation: &str,
    provider: &str,
    grant: &Grant,
    selected: bool,
    now: i64,
) -> (String, Change) {
    let change = if grant.retry_after > now {
        Change::Updated(grant.clone())
    } else if !selected {
        retire(binding, token, installation, provider, grant, now).await
    } else if grant.id.is_some() && grant.lease_expires > now + 600_000 {
        Change::Updated(grant.clone())
    } else {
        renew(binding, token, installation, provider, grant, now).await
    };
    (provider.into(), change)
}

async fn renew(
    binding: &Binding,
    token: &Secret,
    installation: &str,
    provider: &str,
    grant: &Grant,
    now: i64,
) -> Change {
    let (method, url, body) = match &grant.id {
        Some(id) if valid_id(id) => ("PUT", format!("{}/{}", route(binding), id), json!({})),
        Some(_) => return Change::Rejected,
        None => (
            "POST",
            route(binding),
            json!({"provider":provider,"installationId":installation,"accessToken":grant.token.expose()}),
        ),
    };
    let result = cloud_http::send(
        method,
        url,
        Some(token.expose().into()),
        Some(body),
        Duration::from_secs(10),
    )
    .await;
    let Ok(answer) = result else {
        return Change::Retry;
    };
    // Bodies may echo secret input on an older backend; never expose them.
    if answer.status == 403
        && answer.body["error"].as_str() == Some("An Agent Relay app login is required.")
    {
        return Change::SignInRequired;
    }
    if grant.id.is_some() && matches!(answer.status, 404 | 410) {
        return Change::Remove;
    }
    if matches!(answer.status, 401 | 403 | 404 | 409 | 410) {
        return Change::Rejected;
    }
    if !answer.ok() {
        return Change::Retry;
    }
    accept(answer.inner(), binding, provider, grant, now)
        .map(Change::Updated)
        .unwrap_or(Change::Retry)
}

pub(super) fn accept(
    value: &Value,
    binding: &Binding,
    provider: &str,
    grant: &Grant,
    now: i64,
) -> Option<Grant> {
    let id = value["id"].as_str().filter(|id| valid_id(id))?;
    let expiry = agent_relay_node::parse_iso_ms(value["leaseExpiresAt"].as_str()?)?;
    if value["workspaceId"].as_str() != Some(&binding.workspace_id)
        || value["provider"].as_str() != Some(provider)
        || expiry <= now
        || expiry > now + 930_000
        || grant.id.as_deref().is_some_and(|old| old != id)
        || value
            .get("accessToken")
            .is_some_and(|v| v.as_str() != Some(grant.token.expose()))
    {
        return None;
    }
    Some(Grant {
        token: grant.token.clone(),
        id: Some(id.into()),
        lease_expires: expiry,
        retry_after: 0,
        needs_app_login: false,
    })
}

// Keep the durable id/secret until revocation is acknowledged. Otherwise an
// offline uninstall would lose the only secret that can retry an idempotent POST.
async fn retire(
    binding: &Binding,
    token: &Secret,
    installation: &str,
    provider: &str,
    grant: &Grant,
    now: i64,
) -> Change {
    let mut recovered = match &grant.id {
        Some(_) => grant.clone(),
        None => match renew(binding, token, installation, provider, grant, now).await {
            Change::Updated(grant) => grant,
            other => return other,
        },
    };
    if let Some(id) = &recovered.id {
        if revoke(binding, token, id).await {
            return Change::Remove;
        }
    }
    recovered.retry_after = now + 60_000;
    Change::Updated(recovered)
}

// Old token routes predate provider binding and accept unrelated request fields.
// Never POST to them until the hosted server advertises this exact protocol.
pub(super) async fn refresh(
    binding: &Binding,
    token: &Secret,
    installation: &str,
    grants: &BTreeMap<String, Grant>,
    selected: &BTreeSet<String>,
    now: i64,
) -> Vec<(String, Change)> {
    if !selected.is_empty() && !supported(binding, token).await {
        return grants
            .keys()
            .map(|id| (id.clone(), Change::Retry))
            .collect();
    }
    futures_util::future::join_all(grants.iter().map(|(id, grant)| {
        reconcile(
            binding,
            token,
            installation,
            id,
            grant,
            selected.contains(id),
            now,
        )
    }))
    .await
}
async fn supported(binding: &Binding, token: &Secret) -> bool {
    super::super::mcp_auth::hosted_read(&binding.site_url,token,json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"agent-relay-desktop-providers","version":agent_relay_node::VERSION}
    }})).await.is_ok_and(|reply|reply["result"]["capabilities"]["experimental"]["com.agentrelay/desktop-provider-credentials"]["version"].as_u64()==Some(1))
}
