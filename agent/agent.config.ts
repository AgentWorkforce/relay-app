/**
 * layered-relay-app: the layered agent for this repo (ledger, sized review
 * flows, releases). Built by @agentworkforce/layered-agent; lane files are the
 * rest of this folder.
 *
 * A new stack, not an adoption: it creates its own host role and instance
 * profile, log groups, compute stack and session document, and keeps its
 * secrets under its own parameter prefix. Nothing it creates may share a name
 * with the relay-desktop agent's live stack (relaycast-agent-host-<stage>-relay-desktop)
 * or with the relaycast-agent-host-<stage> resources relaycast-cloud owns.
 */
import { defineLayeredAgent } from "@agentworkforce/layered-agent";

const stage = process.env.LAYERED_AGENT_STAGE ?? "dev";

export default defineLayeredAgent({
  name: "relay-app",
  repo: "AgentWorkforce/relay-app",
  stage,
  accountId: process.env.LAYERED_AGENT_ACCOUNT_ID,
  // x64: Relayflows ships runtimes only for x64 Linux and Apple silicon.
  arch: "x64",
  instanceType: "m7i.2xlarge",
  // Pinned (Canonical 24.04, serial 20261023), so Canonical's moving
  // "current" pointer does not make every deploy full.
  // A new image is a deliberate change of this id: a full deploy, dispatched
  // with allow_restart.
  ami: "ami-08cf17e3856f70154",
  rootVolumeGiB: 200,
  // Only the bootstrap rides in the user data (EC2 caps it at 16 KiB raw, and
  // the lane scripts outgrew it): tick.sh, review.sh, release.sh, hotfix.sh
  // and disk-cleanup.sh are installed from main's agent/ in the reference clone by
  // setup-lane.sh (first boot and every hot update) and load-secrets-lane.sh.
  // lane-scripts.sha is their content hash (lane-scripts.ts), so a change
  // to them is a hot change the deploy sees and runs setup-lane.sh for.
  laneFiles: ["lead-prompt.md", "setup-lane.sh", "load-secrets-lane.sh", "share-lead-session.sh", "lane-scripts.sha"],
  // Local CI lanes: cargo + GTK 4 under xvfb, shellcheck.
  aptPackages: ["pkg-config", "libgtk-4-dev", "libssl-dev", "libdbus-1-dev", "xvfb", "shellcheck"],
  secrets: [
    // Jev (sized-review classifier): optional; review.sh falls back to its default depth without it.
    { envVar: "TYPESAFE_KEY_PARAM", param: "typesafe-api-key", exportAs: "TYPESAFE_API_KEY" },
    // Agent Relay Cloud (review.sh submits every review round as a Cloud flow
    // run of flows/cloud-review.flow.ts): without it a round is refused
    // (cloud_auth_missing), never run locally. A flows token with the scopes
    // workflow:invoke:write (submit), workflow:runs:read (status, sync) and
    // workflow:logs:read (logs); it reaches only the flows process.
    { envVar: "FLOWS_CLOUD_TOKEN_PARAM", param: "flows-cloud-token", exportAs: "FLOWS_CLOUD_TOKEN" },
    // Optional: the Cloud API to submit to; absent, the flows CLI's default.
    // A round without the token is refused whatever this holds.
    { envVar: "FLOWS_CLOUD_URL_PARAM", param: "flows-cloud-url", exportAs: "FLOWS_CLOUD_URL" },
  ],
  // The Agent Relay sessions MCP (module 0.1.9): the lead can search and read
  // the team's shared sessions as agent-relay-sessions. The host role grants
  // ssm:GetParameter on exactly this parameter.
  relaySessions: { tokenParam: `/relaycast-agent-host/${stage}/relay-sessions-token-relay-app` },
  repos: ["AgentWorkforce/relay-app"],
  env: { LEDGER_COUNTERPARTS: "macos=app-ui gtk=app-ui" },
  relayDescription: "relay-app layered agent: ledger, sized review flows, releases (EC2, Fable 5.1)",
  // Names. The rule: everything this stack CREATES carries a -relay-app
  // suffix or a /relay-app path segment, so it can never collide with the
  // relay-desktop agent's live stack or relaycast-cloud's shared host
  // resources. Only what it READS stays shared: platformPrefix holds the
  // network ids and the kit secrets (anthropic-api-key, nango-secret-key)
  // that the platform deploy publishes for every agent host. The default
  // tags (layered-agent:name=relay-app) stay, because the deploy role's
  // SendCommand grant is conditioned on that tag and must match this host only.
  names: {
    computeStack: `relaycast-agent-host-${stage}-relay-app`,
    role: `relaycast-agent-host-${stage}-relay-app`,
    sessionDocument: `relaycast-agent-host-${stage}-relay-app-session`,
    sessionLogGroup: `/relaycast-agent-host/${stage}/relay-app/ssm-sessions`,
    telemetryEventsLogGroup: `/relaycast-agent-host/${stage}/relay-app/claude-code-events`,
    telemetryMetricsLogGroup: `/relaycast-agent-host/${stage}/relay-app/claude-code-metrics`,
    telemetryNamespace: "RelaycastAgentHost/ClaudeCode/relay-app",
    paramPrefix: `/relaycast-agent-host/${stage}/relay-app`,
    platformPrefix: `/relaycast-agent-host/${stage}`,
  },
  description: {
    stack: `relay-app layered agent host (${stage}): one x64 EC2 instance, SSM-only access`,
    session: `relay-app layered agent host (${stage}): sessions logged to CloudWatch`,
  },
});
