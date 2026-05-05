use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum SharingDialogSource {
    PaneHeader,
    CommandPalette,
    DriveIndex,
    StartedSessionShare,
    InviteeRequest,
    InheritedPermission,
    OnboardingBlock,
    ConversationList,
    AIBlockContextMenu,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum TabRenameEvent {
    OpenedEditor,
    CustomNameSet,
    CustomNameCleared,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum NotificationsTurnedOnSource {
    Settings,
    Banner,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum FindOption {
    CaseSensitive,
    FindInBlock,
    Regex,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum LinkOpenMethod {
    CmdClick,
    ToolTip,
    MiddleClick,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CommandXRayTrigger {
    Hover,
    Keystroke,
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug)]
pub enum PaletteSource {
    PrefixChange,
    Keybinding,
    CtrlTab { shift_pressed_initially: bool },
    WarpDrive,
    QuitModal,
    LogOutModal,
    IntegrationTest,
    ConversationManager,
    ContextChip,
    PaneHeader,
    RecentsViewAll,
    AgentTip,
    TitleBarSearchBar,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum FileTreeSource {
    PaneHeader,
    Keybinding,
    LeftPanelToolbelt,
    ForceOpened,
    CLIAgentView,
}

#[cfg(feature = "local_fs")]
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodePanelsFileOpenEntrypoint {
    CodeReview,
    ProjectExplorer,
    GlobalSearch,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum CLIAgentType {
    Claude,
    Gemini,
    Codex,
    Amp,
    Droid,
    OpenCode,
    Copilot,
    Pi,
    Auggie,
    Cursor,
    Goose,
    Vibe,
    Unknown,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentNotificationVariant {
    Oz,
    CLIAgent(CLIAgentType),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum NotificationAgentVariant {
    Oz,
    CLIAgent(CLIAgentType),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum WarpDriveSource {
    Legacy,
    LeftPanelToolbelt,
    ForceOpened,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum CommandCorrectionAcceptedType {
    Autosuggestion,
    Banner,
    Keybinding,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum CommandCorrectionEvent {
    Proposed {
        rule: &'static str,
    },
    Accepted {
        via: CommandCorrectionAcceptedType,
        rule: &'static str,
    },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum CloseTarget {
    App,
    Window,
    Tab,
    Pane,
    EditorTab,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub enum PtySpawnMode {
    TerminalServer,
    FallbackToDirect,
    Direct,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum OpenedWarpAISource {
    GlobalEntryButton,
    HelpWithBlock,
    HelpWithTextSelection,
    FromAICommandSearch,
    WarmWelcome,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum WarpAIRequestResult {
    Succeeded { latency_ms: i64, truncated: bool },
    OutOfRequests,
    Failed,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum WarpAIActionType {
    CopyTranscript,
    Restart,
    CopyAnswer,
    CopyCode,
    InsertIntoInput,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum SaveAsWorkflowModalSource {
    Block,
    Input,
    WarpAIWorkflowCard,
    WarpAIPanel,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum LaunchConfigUiLocation {
    CommandPalette,
    AppMenu,
    TabMenu,
    Uri,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum AICommandSearchEntrypoint {
    ShortHandTrigger,
    Keybinding,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum SecretInteraction {
    RevealSecret,
    HideSecret,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum AnonymousUserSignupEntrypoint {
    HitDriveObjectLimit,
    LoginGatedFeature,
    SignUpButton,
    RenotificationBlock,
    SignUpAIPrompt,
    NextCommandSuggestionsUpgradeBanner,
    Unknown,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum UndoCloseItemType {
    Window,
    Tab,
    Pane,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum ToggleBlockFilterSource {
    Binding,
    ContextMenu,
}

#[derive(Clone, Debug, Copy, Serialize, Deserialize)]
pub enum KnowledgePaneEntrypoint {
    #[serde(rename = "global")]
    Global,
    #[serde(rename = "settings")]
    Settings,
    #[serde(rename = "warp_drive")]
    WarpDrive,
    #[serde(rename = "ai_blocklist")]
    AIBlocklist,
    #[serde(rename = "slash_command")]
    SlashCommand,
}

#[derive(Clone, Debug, Copy, Serialize, Deserialize)]
pub enum MCPServerCollectionPaneEntrypoint {
    #[serde(rename = "global")]
    Global,
    #[serde(rename = "settings")]
    Settings,
    #[serde(rename = "warp_drive")]
    WarpDrive,
    #[serde(rename = "slash_command")]
    SlashCommand,
    #[serde(rename = "mcp_settings_tab")]
    MCPSettingsTab,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AgentModeEntrypointSelectionType {
    Text,
    Block,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AgentModeEntrypoint {
    #[serde(rename = "tab_bar")]
    TabBar,
    #[serde(rename = "new_pane_binding")]
    NewPaneBinding,
    #[serde(rename = "block_toolbelt")]
    BlockToolbelt,
    #[serde(rename = "ai_command_search")]
    AICommandSearch,
    #[serde(rename = "context_menu")]
    ContextMenu {
        selection_type: AgentModeEntrypointSelectionType,
    },
    #[serde(rename = "prompt_chip")]
    PromptChip,
    #[serde(rename = "agent_management_popup")]
    AgentManagementPopup,
    #[serde(rename = "udi_terminal_input_switcher")]
    UDITerminalInputSwitcher,
    #[serde(rename = "agent_management_view")]
    AgentManagementView,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AutonomySettingToggleSource {
    Speedbump,
    SettingsPage,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ToggleCodeSuggestionsSettingSource {
    Speedbump,
    Settings,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum InteractionSource {
    Button,
    Keybinding,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum PromptSuggestionViewType {
    TerminalView,
    AgentView,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AgentModeAttachContextMethod {
    #[serde(rename = "keyboard")]
    Keyboard,
    #[serde(rename = "mouse")]
    Mouse,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum AgentModeRewindEntrypoint {
    Button,
    ContextMenu,
    SlashCommand,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub enum PromptSuggestionFallbackReason {
    #[serde(rename = "file_too_many_lines")]
    FileTooManyLines,
    #[serde(rename = "file_too_many_bytes")]
    FileTooManyBytes,
    #[serde(rename = "missing_file")]
    MissingFile,
    #[serde(rename = "failed_to_retrieve_file")]
    FailedToRetrieveFile,
    #[serde(rename = "ssh_remote_session")]
    SSHRemoteSession,
    #[serde(rename = "no_read_files_permission")]
    NoReadFilesPermission,
    #[serde(rename = "ai_query_timeout")]
    AIQueryTimeout,
    #[serde(rename = "failed_to_send_ai_request")]
    FailedToSendAIRequest,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AgentModeSetupProjectScopedRulesActionType {
    #[serde(rename = "link_from_existing")]
    LinkFromExisting(String),
    #[serde(rename = "generate_warp_md")]
    GenerateWarpMd,
    #[serde(rename = "skip_rules")]
    SkipRules,
    #[serde(rename = "regenerate_warp_md")]
    RegenerateWarpMd,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AgentModeSetupCodebaseContextActionType {
    #[serde(rename = "index_codebase")]
    IndexCodebase,
    #[serde(rename = "skip_indexing")]
    SkipIndexing,
    #[serde(rename = "view_index_status")]
    ViewIndexStatus,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AgentModeSetupCreateEnvironmentActionType {
    #[serde(rename = "create_environment")]
    CreateEnvironment,
    #[serde(rename = "skip_environment")]
    SkipEnvironment,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum AgentModeAutoDetectionSettingOrigin {
    #[serde(rename = "banner")]
    Banner,
    #[serde(rename = "settings_page")]
    SettingsPage,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AgentModeAutoDetectionFalsePositivePayload {
    InternalDogfoodUsers { input_text: String },
    ExternalUsers,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub enum AgentModeCodeFileNavigationSource {
    NavigationCommand,
    SelectedFileTab,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum AddTabWithShellSource {
    CommandPalette,
    ShellSelectorMenu,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeContextDestination {
    Pty,
    AgentInput,
    RichInput,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub enum ImageProtocol {
    Kitty,
    ITerm,
}

#[derive(Clone, Copy, Debug, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InputUXChangeOrigin {
    #[default]
    Settings,
    ADELaunchModal,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub enum SlashMenuSource {
    SlashButton,
    UserTyped,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginEventSource {
    OnboardingSlide,
    AuthModal,
}

#[derive(Clone, Debug, Serialize)]
pub enum SlashCommandAcceptedDetails {
    StaticCommand { command_name: String },
    SavedPrompt,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AutoReloadModalAction {
    #[serde(rename = "dismissed")]
    Dismissed,
    #[serde(rename = "enabled_auto_reload")]
    EnabledAutoReload,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum OutOfCreditsBannerAction {
    #[serde(rename = "dismissed")]
    Dismissed,
    #[serde(rename = "credits_purchased")]
    CreditsPurchased,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CLISubagentControlState {
    AgentInControl,
    UserInControl,
    AgentTaggedIn,
    AgentTaggedOut,
}

#[derive(Clone, Debug, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MCPTemplateCreationSource {
    Json,
    Conversion,
}

#[derive(Clone, Debug, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MCPTemplateInstallationSource {
    Local,
    Shared,
    Gallery,
}

#[derive(Clone, Debug, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MCPServerModel {
    Legacy,
    Templatable,
}
