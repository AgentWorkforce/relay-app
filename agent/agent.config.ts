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
  // The A2A card the lead publishes on the relay (set_relay_agent_card,
  // kit 0.1.17), so a peer can see what to send it before delegating.
  // Delivered by hot update, never in the user data.
  card: {
    description:
      "layered-relay-app runs the maintainer workflow for AgentWorkforce/relay-app, the Agent Relay desktop app for macOS, Linux and Windows. It keeps a ledger of who is changing what and warns both agents when work overlaps, runs one sized review round per pushed branch as Agent Relay Cloud flow runs and lands green changes on main, and publishes beta releases and hotfixes. Tell it what you are starting in relay-app, and send it your pushed review/ branch for review.",
    skills: [
      {
        id: "track-work",
        name: "Record your work and flag overlaps",
        description:
          "Records what an agent is working on in relay-app (task, branch, session, paths) in its ledger, checks it against every other open entry and main, and messages both agents when their work overlaps. macOS and GTK count as one area. Send a one-line summary with your branch and paths, or a shared session id.",
        tags: ["ledger", "coordination"],
        examples: [
          "Starting the GTK settings page rewrite on review/alice-gtk-settings, paths gtk/src/settings,macos/Settings",
          "Here is my session id; record what I'm working on",
          "review/bob-history-search landed; mark me done",
        ],
      },
      {
        id: "review-change",
        name: "Review a pushed change",
        description:
          "Runs one review round for a pushed branch at the size the change needs (small, standard or deep, picked by the Jev classifier), as Agent Relay Cloud flow runs with local CI, and sends back every finding at once. A fix push gets CI and the GitHub gate only; only a blocker gets a re-review of its fix.",
        tags: ["code-review", "ci"],
        examples: [
          "Please review review/bob-history-search: it adds search to the session history view",
          "Review review/carol-signin-retry at deep depth",
          "I pushed the fixes for the findings on review/carol-signin-retry; run CI and the gate",
        ],
      },
      {
        id: "land-change",
        name: "Land a reviewed change",
        description:
          "Squashes a change onto main once its review round has reported, no blocker is open, and local CI and the GitHub gate are green on that commit; rebases and re-runs CI when main has moved. Say \"hold this\" or \"I land this myself\" when you ask for the review to keep it from landing.",
        tags: ["merge", "main"],
        examples: [
          "Hold review/dave-tray-menu, I'll land it myself",
          "main moved under review/alice-gtk-settings; is it still landing?",
        ],
      },
      {
        id: "publish-release",
        name: "Publish a release",
        description:
          "Keeps a signed release candidate of main built and promotes it on request: a beta when asked, a stable only after confirming with the owner directly. Reports the version, source commit, how far behind main it is and the release URL.",
        tags: ["release", "desktop"],
        examples: [
          "Release beta",
          "Which candidate is ready, and how many commits behind main is it?",
          "What commit did 2026.11.5 ship from?",
        ],
      },
      {
        id: "start-hotfix",
        name: "Start and ship a hotfix",
        description:
          "Gives you a verified shipped commit to branch a hotfix from (any release, older ones included), reviews the hotfix branch, publishes from it, and makes sure the same fix lands on main.",
        tags: ["hotfix", "release"],
        examples: [
          "I need to hotfix 2026.11.5 for a crash on sign-in: give me the base commit",
          "hotfix/2026.11.5-signin-crash is pushed; review it and prepare a beta from it",
        ],
      },
    ],
    tags: ["relay-app", "desktop", "code-review", "releases", "coordination"],
    documentationUrl: "https://github.com/AgentWorkforce/relay-app",
  },
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
