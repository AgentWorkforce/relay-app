//! The bridge methods this build adds to the core's contract, with their
//! parameter and result types. `agent-relay-desktop-contract` writes them, with
//! the core's own registry, into the macOS app's Swift.

pub mod account;
pub mod capy;
pub mod chat;
pub mod contractors;
pub mod cursor_cloud;
pub mod platform;
pub mod replicas;
pub mod service_callers;
pub mod tool_integrations;

use account::*;
use agent_relay_node::relay::contract::agents::LimitParams;
use agent_relay_node::relay::contract::directory::SessionOverviewParams;
use agent_relay_node::relay::contract::transcript::{LocalTranscriptPage, LocalTranscriptParams};
use agent_relay_node::relay::contract::{Availability, Done, Method, MethodSpec, NoParams};
use agent_relay_node::relay::state::WorkspaceStatus;
use capy::*;
use chat::*;
use contractors::*;
use cursor_cloud::*;
use platform::*;
use replicas::*;
use service_callers::*;
use tool_integrations::*;

macro_rules! methods {
    ($($marker:ident = $name:literal, $params:ty => $result:ty;)*) => {
        /// One marker per method; `<Marker>::NAME` is its wire name.
        pub mod method {
            use super::*;
            $(
                pub struct $marker;
                impl $marker {
                    pub const NAME: &'static str = $name;
                }
                impl Method for $marker {
                    const NAME: &'static str = $name;
                    type Params = $params;
                    type Result = $result;
                }
            )*
        }

        /// Every method this build's account answers.
        pub fn account_specs() -> Vec<MethodSpec> {
            vec![$(MethodSpec::of::<method::$marker>(Availability::Account),)*]
        }
    };
}

methods! {
    // Workspace and account.
    AccountStatus = "account.status", NoParams => agent_relay_node::relay::contract::account::SignInReport;
    AccountMcpHeaders = "account.mcp_headers", McpHeadersParams => McpHeaders;
    AccountFeedHeaders = "account.feed_headers", FeedHeadersParams => FeedHeaders;
    AccountSignIn = "account.sign_in", account::SignInRequest => agent_relay_node::relay::contract::account::SignInReport;
    AccountLoginConfig = "account.login_config", NoParams => account::LoginConfig;
    AccountSelectWorkspace = "account.select_workspace", ConnectWorkspaceParams => Done;
    AccountSignInCancel = "account.sign_in.cancel", NoParams => Done;
    AccountSignOut = "account.sign_out", NoParams => Done;
    AccountConnectWorkspace = "account.connect_workspace", ConnectWorkspaceParams => WorkspaceStatus;

    // The uploader.
    UploaderStatus = "uploader.status", NoParams => account::UploaderStatus;
    UploaderStart = "uploader.start", NoParams => UploaderStarted;
    UploaderPause = "uploader.pause", NoParams => UploaderPaused;
    UploaderResume = "uploader.resume", NoParams => UploaderPaused;
    UploaderRetry = "uploader.retry", NoParams => UploadsRetried;
    UploaderCompact = "uploader.compact", NoParams => JournalCompacted;
    UploaderDisconnect = "uploader.disconnect", NoParams => UploaderDisconnected;
    UploaderSharingSet = "uploader.sharing.set", SharingParams => SharingChanged;
    UploaderSessionsList = "uploader.sessions.list", LimitParams => UploadSessions;
    UploaderSessionsFacets = "uploader.sessions.facets", SessionFacetParams => SessionFacetsPage;
    UploaderSessionsQuery = "uploader.sessions.query", SessionQueryParams => SessionQueryPage;
    UploaderSessionsPreview = "uploader.sessions.preview", PreviewParams => UploadPreview;
    UploaderSessionsInclude = "uploader.sessions.include", SessionKeysParams => SessionsUpdated;
    UploaderSessionsReset = "uploader.sessions.reset", ResetSessionsParams => SessionsReset;
    UploaderSessionsDelete = "uploader.sessions.delete", SessionKeysParams => SessionsUpdated;
    UploaderSessionsExclude = "uploader.sessions.exclude", SessionKeysParams => SessionsUpdated;

    // Session transcripts, provider setup and usage.
    SessionsTranscript = "sessions.transcript", LocalTranscriptParams => LocalTranscriptPage;
    ToolsStatus = "tools.status", NoParams => ToolsReport;
    ToolsCheck = "tools.check", NoParams => ToolsReport;
    ToolsSelect = "tools.select", ToolsSelectParams => ToolsReport;
    ToolsManage = "tools.manage", ToolsManageParams => ToolsReport;
    TelemetryRecord = "telemetry.record", crate::telemetry::TelemetryEvent => Done;

    // Cloud's provider routes and session summaries.
    CursorCloudModelsList = "cursor_cloud.models.list", NoParams => CursorCloudModels;
    CursorCloudRepositoriesList = "cursor_cloud.repositories.list", NoParams => CursorCloudRepositories;
    CursorCloudEnvironmentsList = "cursor_cloud.environments.list", CursorCloudEnvironmentsListParams => CursorCloudEnvironments;
    CursorCloudAgentsList = "cursor_cloud.agents.list", CursorCloudAgentsListParams => CursorCloudAgents;
    CursorCloudAgentsSpawn = "cursor_cloud.agents.spawn", CursorCloudAgentSpawnParams => CursorCloudAgent;
    CursorCloudAgentsGet = "cursor_cloud.agents.get", CursorCloudAgentParams => CursorCloudAgent;
    CursorCloudAgentsAttach = "cursor_cloud.agents.attach", CursorCloudAgentParams => CursorCloudAgent;
    CursorCloudRunsCreate = "cursor_cloud.runs.create", CursorCloudRunCreateParams => CursorCloudRun;
    CursorCloudRunsList = "cursor_cloud.runs.list", CursorCloudRunsListParams => CursorCloudRuns;
    CursorCloudRunsGet = "cursor_cloud.runs.get", CursorCloudRunParams => CursorCloudRun;
    CursorCloudRunsCancel = "cursor_cloud.runs.cancel", CursorCloudRunParams => CursorCloudRun;
    CursorCloudAgentsArchive = "cursor_cloud.agents.archive", CursorCloudAgentParams => CursorCloudLifecycle;
    CursorCloudAgentsUnarchive = "cursor_cloud.agents.unarchive", CursorCloudAgentParams => CursorCloudLifecycle;
    ReplicasEnvironmentsList = "replicas.environments.list", NoParams => ReplicasEnvironments;
    ReplicasSpawn = "replicas.spawn", ReplicasSpawnParams => replicas::ReplicasSpawn;
    ReplicasSpawnGet = "replicas.spawn.get", ReplicasSpawnGetParams => replicas::ReplicasSpawn;
    ReplicasRelease = "replicas.release", ReplicasSpawnGetParams => ReplicasReleased;
    CapyProjectsList = "capy.projects.list", CapyProjectsParams => CapyProjects;
    CapyThreadsSpawn = "capy.threads.spawn", CapyThreadSpawnParams => CapyThreadSpawn;
    CapyThreadsGet = "capy.threads.get", CapyThreadSpawnGetParams => CapyThreadSpawn;
    SessionsOverviewGet = "sessions.overview.get", SessionOverviewParams => serde_json::Value;
    SessionsOverviewRequest = "sessions.overview.request", SessionOverviewParams => serde_json::Value;
    SessionsOverviewEvents = "sessions.overview.events", SessionOverviewParams => serde_json::Value;

    // Launch at login.
    LoginItemSet = "login_item.set", LoginItemParams => Done;
    LoginItemStatus = "login_item.status", NoParams => LoginItemState;

    // Skip chat.
    ChatConversation = "chat.conversation", NoParams => chat::ChatConversation;
    ChatSend = "chat.send", ChatSendParams => ChatAnswer;
    ChatClear = "chat.clear", NoParams => chat::ChatConversation;
    ChatAct = "chat.act", ActParams => ActResult;
    ChatHistory = "chat.history", ChatHistoryParams => HistoryPage;
    ChatProjects = "chat.projects", NoParams => ProjectsState;
    ChatProjectsRefresh = "chat.projects.refresh", NoParams => ProjectsState;
    ChatPush = "chat.push", PushParams => PushResult;

    // Contractor access.
    ContractorsStatus = "contractors.status", NoParams => ContractorStatus;
    ContractorsRefresh = "contractors.refresh", NoParams => ContractorStatus;
    ContractorsPropose = "contractors.propose", ProposeParams => crate::contractor_access::Proposal;
    ContractorsApprove = "contractors.approve", ApproveParams => GrantResult;
    ContractorsRevoke = "contractors.revoke", RevokeParams => Done;
    ContractorsDismiss = "contractors.dismiss", DismissParams => ContractorStatus;
    // Service callers: the person's management methods, then the caller's
    // own identity and result inbox (docs/service-callers.md).
    SpawnCallersList = "spawn_callers.list", CallerListParams => CallerList;
    SpawnCallersProvision = "spawn_callers.provision", CallerProvisionParams => CallerProvisioned;
    SpawnCallersRotate = "spawn_callers.rotate", CallerRotateParams => CallerRotated;
    SpawnCallersSetEnabled = "spawn_callers.set_enabled", CallerEnabledParams => CallerChanged;
    SpawnCallersRevoke = "spawn_callers.revoke", CallerIdParams => Done;
    SpawnCallersDecide = "spawn_callers.decide", CallerDecideParams => CallerDecided;
    SpawnCallersIdentity = "spawn_callers.identity", CallerAuthParams => CallerIdentity;
    SpawnCallersInboxOpen = "spawn_callers.inbox.open", InboxOpenParams => InboxOpened;
    SpawnCallersInboxNext = "spawn_callers.inbox.next", InboxNextParams => InboxNext;
    SpawnCallersInboxAck = "spawn_callers.inbox.ack", InboxSettleParams => InboxSettled;
    SpawnCallersInboxFail = "spawn_callers.inbox.fail", InboxSettleParams => InboxSettled;
    SpawnCallersInboxClose = "spawn_callers.inbox.close", InboxIteratorParams => Done;
    SpawnCallersDisconnect = "spawn_callers.disconnect", CallerAuthParams => Done;
    SpawnCallersChallenge = "spawn_callers.challenge", CallerChallengeParams => CallerChallenge;
    SpawnCallersAuthenticate = "spawn_callers.authenticate", CallerAuthenticateParams => CallerBound;
}

/// Every method this build adds, as `hello` and the Swift generator read them.
pub fn all_specs() -> Vec<MethodSpec> {
    let mut specs = account_specs();
    for spec in &mut specs {
        if spec.name == <method::SpawnCallersDecide as Method>::NAME {
            spec.result = decided_and_error_classes;
        }
    }
    specs.push(MethodSpec::of::<crate::session_locations::SessionsLocation>(Availability::Always));
    specs.push(MethodSpec::of::<crate::session_locations::SessionsOpenPlan>(Availability::Always));
    specs
}

/// `spawn_callers.decide`'s result, naming beside it the class every
/// `spawn_callers.*` error carries as its message, so the generated
/// contract decodes the class typed although no result holds one.
fn decided_and_error_classes(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    generator.subschema_for::<CallerErrorClass>();
    generator.subschema_for::<CallerDecided>()
}
