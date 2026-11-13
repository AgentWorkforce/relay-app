/**
 * layered-relay-desktop: the layered agent for this repo (ledger, sized review
 * flows, releases). Built by @agentworkforce/layered-agent; lane files are the
 * rest of this folder.
 *
 * Adopted from relaycast-cloud/agent-host, so it keeps that host's names, tags,
 * descriptions and secret parameters: its compute template stays identical
 * apart from UserData and the pinned AmiId (test/agent.test.ts), and the move
 * is a stop/start of the instance, never a replacement. Adopting the AMI pin
 * does not touch the instance.
 */
import { defineLayeredAgent } from "@agentworkforce/layered-agent";

const stage = process.env.LAYERED_AGENT_STAGE ?? "dev";

export default defineLayeredAgent({
  name: "relay-desktop",
  repo: "AgentWorkforce/relay-desktop",
  stage,
  accountId: process.env.LAYERED_AGENT_ACCOUNT_ID,
  // x64: Relayflows ships runtimes only for x64 Linux and Apple silicon.
  arch: "x64",
  instanceType: "m7i.2xlarge",
  // Pinned to the AMI the adopted host runs (Canonical 24.04, serial 20261023),
  // so Canonical's moving "current" pointer no longer makes every deploy full.
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
  // the team's shared sessions as agent-relay-sessions. The host runs on the
  // shared relaycast-agent-host-dev role, which relaycast-cloud owns, so the
  // module grants nothing: relaycast-cloud's agent-host role must allow
  // ssm:GetParameter on exactly this parameter.
  relaySessions: { tokenParam: `/relaycast-agent-host/${stage}/relay-sessions-token-relay-desktop` },
  repos: ["AgentWorkforce/relay-desktop"],
  env: { LEDGER_COUNTERPARTS: "macos=app-ui gtk=app-ui" },
  relayDescription: "relay-desktop layered agent: ledger, sized review flows, releases (EC2, Fable 5.1)",
  names: {
    computeStack: `relaycast-agent-host-${stage}-relay-desktop`,
    role: `relaycast-agent-host-${stage}`,
    sessionDocument: `relaycast-agent-host-${stage}-relay-desktop-session`,
    sessionLogGroup: `/relaycast-agent-host/${stage}/ssm-sessions`,
    telemetryEventsLogGroup: `/relaycast-agent-host/${stage}/claude-code-events`,
    telemetryMetricsLogGroup: `/relaycast-agent-host/${stage}/claude-code-metrics`,
    telemetryNamespace: "RelaycastAgentHost/ClaudeCode",
    paramPrefix: `/relaycast-agent-host/${stage}`,
    platformPrefix: `/relaycast-agent-host/${stage}`,
  },
  tags: { "relaycast:component": "agent-host", "relaycast:stage": stage },
  description: {
    stack: `relay-desktop layered agent host (${stage}): one x64 EC2 instance, SSM-only access`,
    session: `relay-desktop layered agent host (${stage}): sessions logged to CloudWatch`,
  },
});
