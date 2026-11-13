# Dedicated release App setup

The GitHub organization owner and hosted Cloud workspace owner perform this
one-time setup. The runtime never reads a host App key or general GitHub token.
Do not send credentials through Relay or chat, commit them, or put them in flow
inputs. App and installation IDs can be shared as setup metadata.

## GitHub owner

Create a private App under [AgentWorkforce's GitHub App settings](https://github.com/organizations/AgentWorkforce/settings/apps/new):

| Setting | Value |
| --- | --- |
| Name | Relay Desktop Cloud Release (or an available equivalent) |
| Homepage | `https://github.com/AgentWorkforce/relay-app` |
| Webhook | Disabled |
| User authorization / OAuth | Not used |
| Repository permissions | Actions: read/write; Contents: read/write; Metadata: read |
| Organization/account permissions and events | None |
| Installation | Only `AgentWorkforce/relay-app` |

Record the App ID and installation ID. Generate a private key for this new
App and download its PEM directly to the owner-controlled machine. Keep this
separate from the organization pusher and host credentials. GitHub App creation,
installation approval and private-key handling belong to the account owner.

## Hosted Cloud owner

Use the workspace bound to the release lead's submit login. Confirm that
workspace ID with the lead; do not choose another workspace with a similar name.
Provision three environment secrets:

| Name | Value |
| --- | --- |
| `DESKTOP_RELEASE_GH_APP_ID` | Numeric App ID |
| `DESKTOP_RELEASE_GH_INSTALLATION_ID` | Numeric installation ID |
| `DESKTOP_RELEASE_GH_APP_PRIVATE_KEY` | New App's PEM, preserving newlines |

The supported Cloud API is `POST /api/v1/workspaces/{workspaceId}/secrets`,
relative to the same Cloud application base as the lead's flows CLI (normally
`https://agentrelay.com/cloud`). Authenticate with the owner's signed-in session
or a Cloud CLI token with `cli:auth`; the host's workflow-only submit token
does not authorize these writes. Each request has this shape:

```json
{"name":"DESKTOP_RELEASE_GH_APP_ID","value":"<App ID>","kind":"environment"}
```

Send the other two values under their respective names with the same kind.
For the PEM, serialize the file contents as JSON rather than pasting a literal
backslash-n string. Read the Cloud token and PEM from secure local input/files;
never put values in command arguments, shell history or logs. Expect HTTP 201
and metadata with `deliveredToFlowRuns: true`. The API does not read secret
values back. Report only the three configured names and the App/installation
IDs to the rollout owner.

## Rollout owner

After the reviewed App-auth code is on main and secrets are provisioned, ask
`layered-relay-desktop` to execute the prepare-only smoke from its host. Require
Cloud acceptance, native worker dispatch, exact-tree CI reuse, candidate output,
and rejection of deliberately invalid evidence. Provisioning alone is not a
successful smoke. After those checks pass, land the managed environment change
`DESKTOP_RELEASE_ORCHESTRATOR=cloud` through the normal review and Agent Deploy.
Verify a host tick submits a Cloud run. Stable publication still requires
the owner's direct confirmation and is not part of this setup.
