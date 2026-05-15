use std::fmt::Display;

use regex::Regex;
use warpui::{AppContext, Entity, ModelContext, SingletonEntity, UpdateModel};

use crate::terminal::safe_mode_settings::SafeModeSettings;

use settings::{
    macros::{maybe_define_setting, register_settings_events},
    ChangeEventReason, RespectUserSyncSetting, Setting, SupportedPlatforms, SyncToCloud,
};

use serde::{Deserialize, Serialize};

use crate::workspaces::workspace::EnterpriseSecretRegex;

pub trait RegexDisplayInfo {
    fn pattern(&self) -> &str;
    fn name(&self) -> Option<&str>;
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(description = "A custom regex pattern for detecting and redacting secrets.")]
pub struct CustomSecretRegex {
    #[serde(with = "serde_regex")]
    #[schemars(with = "String", description = "The regex pattern to match secrets.")]
    pub pattern: Regex,
    #[serde(default)]
    #[schemars(description = "Optional display name for this secret pattern.")]
    pub name: Option<String>,
}

impl CustomSecretRegex {
    pub fn pattern(&self) -> &Regex {
        &self.pattern
    }
}

impl RegexDisplayInfo for CustomSecretRegex {
    fn pattern(&self) -> &str {
        self.pattern.as_str()
    }

    fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

impl RegexDisplayInfo for EnterpriseSecretRegex {
    fn pattern(&self) -> &str {
        &self.pattern
    }

    fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

impl Display for CustomSecretRegex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.pattern.as_str())
    }
}

impl PartialEq for CustomSecretRegex {
    fn eq(&self, other: &Self) -> bool {
        self.pattern.as_str() == other.pattern.as_str()
    }
}

impl settings_value::SettingsValue for CustomSecretRegex {}

maybe_define_setting!(CustomSecretRegexList, group: PrivacySettings, {
    type: Vec<CustomSecretRegex>,
    default: Vec::new(),
    supported_platforms: SupportedPlatforms::ALL,
    sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::No),
    private: false,
    toml_path: "privacy.custom_secret_regex_list",
    description: "Custom regex patterns for detecting and redacting secrets.",
});

maybe_define_setting!(HasInitializedDefaultSecretRegexes, group: PrivacySettings, {
    type: bool,
    default: false,
    supported_platforms: SupportedPlatforms::ALL,
    sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::No),
    private: true,
});

pub struct PrivacySettings {
    pub has_initialized_default_secret_regexes: HasInitializedDefaultSecretRegexes,
    pub user_secret_regex_list: CustomSecretRegexList,
    pub enterprise_secret_regex_list: Vec<CustomSecretRegex>,
    pub is_enterprise_secret_redaction_enabled: bool,
}

impl PrivacySettings {
    pub fn register_singleton(ctx: &mut AppContext) {
        let handle = ctx.add_singleton_model(PrivacySettings::new);

        register_settings_events!(
            PrivacySettings,
            user_secret_regex_list,
            CustomSecretRegexList,
            handle,
            ctx
        );
    }

    fn new(ctx: &mut ModelContext<Self>) -> Self {
        let user_secret_regex_list: CustomSecretRegexList =
            CustomSecretRegexList::new_from_storage(ctx);
        let has_initialized_default_secret_regexes: HasInitializedDefaultSecretRegexes =
            HasInitializedDefaultSecretRegexes::new_from_storage(ctx);

        Self {
            user_secret_regex_list,
            has_initialized_default_secret_regexes,
            is_enterprise_secret_redaction_enabled: false,
            enterprise_secret_regex_list: Vec::new(),
        }
    }

    pub fn is_enterprise_secret_redaction_enabled(&self) -> bool {
        self.is_enterprise_secret_redaction_enabled
    }

    pub fn set_enterprise_secret_redaction_settings(
        &mut self,
        enabled: bool,
        enterprise_regexes: Vec<EnterpriseSecretRegex>,
        change_event_reason: ChangeEventReason,
        ctx: &mut ModelContext<Self>,
    ) {
        if enabled {
            if !self.is_enterprise_secret_redaction_enabled {
                let safe_mode_settings = SafeModeSettings::handle(ctx);
                ctx.update_model(&safe_mode_settings, |safe_mode_settings, ctx| {
                    let _ = safe_mode_settings.safe_mode_enabled.set_value(true, ctx);
                });
            }

            let mut enterprise_secrets = Vec::new();
            for enterprise_regex in enterprise_regexes {
                if let Ok(regex) = Regex::new(&enterprise_regex.pattern) {
                    enterprise_secrets.push(CustomSecretRegex {
                        pattern: regex,
                        name: enterprise_regex.name,
                    });
                } else {
                    log::error!(
                        "Invalid enterprise secret regex pattern: {}",
                        enterprise_regex.pattern
                    );
                }
            }
            self.enterprise_secret_regex_list = enterprise_secrets;
        } else {
            self.enterprise_secret_regex_list.clear();
        }

        self.is_enterprise_secret_redaction_enabled = enabled;

        ctx.emit(PrivacySettingsChangedEvent::CustomSecretRegexList {
            change_event_reason,
        });
        ctx.notify();
    }

    pub fn reset_state(&mut self) {
        self.is_enterprise_secret_redaction_enabled = false;
        self.enterprise_secret_regex_list.clear();
    }

    pub fn fetch_or_update_settings(&self, ctx: &mut ModelContext<Self>) {
        let _ = (self, ctx);
    }

    #[cfg(test)]
    pub fn mock(_ctx: &mut ModelContext<Self>) -> Self {
        Self {
            user_secret_regex_list: CustomSecretRegexList::new(None),
            has_initialized_default_secret_regexes: HasInitializedDefaultSecretRegexes::new(None),
            is_enterprise_secret_redaction_enabled: false,
            enterprise_secret_regex_list: Vec::new(),
        }
    }

    pub fn remove_user_secret_regex(&mut self, idx: &usize, ctx: &mut ModelContext<Self>) {
        let mut new_user_secret_regex_list = self.user_secret_regex_list.to_vec();
        new_user_secret_regex_list.remove(*idx);
        if self
            .user_secret_regex_list
            .set_value(new_user_secret_regex_list, ctx)
            .is_err()
        {
            log::error!("Custom Secret Regex List failed to serialize")
        }
    }

    pub fn add_all_recommended_regex(&mut self, ctx: &mut ModelContext<Self>) {
        let mut new_user_secret_regex_list = self.user_secret_regex_list.to_vec();
        let num_existing_regexes = new_user_secret_regex_list.len();

        for default_regex in crate::terminal::model::secrets::regexes::DEFAULT_REGEXES_WITH_NAMES {
            if let Ok(regex) = Regex::new(default_regex.pattern) {
                let custom_regex = CustomSecretRegex {
                    pattern: regex,
                    name: Some(default_regex.name.to_string()),
                };
                if !new_user_secret_regex_list.contains(&custom_regex) {
                    new_user_secret_regex_list.push(custom_regex);
                }
            } else {
                log::error!("Failed to compile default regex: {}", default_regex.pattern);
            }
        }

        if num_existing_regexes == new_user_secret_regex_list.len() {
            return;
        }

        if self
            .user_secret_regex_list
            .set_value(new_user_secret_regex_list, ctx)
            .is_err()
        {
            log::error!("Failed to serialize default regexes to custom secret regex list")
        }

        ctx.notify();
    }

    pub fn disable_default_regex_trigger(&mut self, ctx: &mut ModelContext<Self>) {
        if self
            .has_initialized_default_secret_regexes
            .set_value(true, ctx)
            .is_err()
        {
            log::error!("Failed to disable default regex trigger");
        }
    }

    pub fn initialize_default_regexes_once(&mut self, ctx: &mut ModelContext<Self>) {
        if !*self.has_initialized_default_secret_regexes.value() {
            self.add_all_recommended_regex(ctx);

            if self
                .has_initialized_default_secret_regexes
                .set_value(true, ctx)
                .is_err()
            {
                log::error!("Failed to set has_initialized_default_secret_regexes flag");
            }
        }
    }
}

#[derive(Clone, Copy)]
pub enum PrivacySettingsChangedEvent {
    CustomSecretRegexList {
        change_event_reason: ChangeEventReason,
    },
    HasInitializedDefaultSecretRegexes {
        change_event_reason: ChangeEventReason,
    },
}

impl Entity for PrivacySettings {
    type Event = PrivacySettingsChangedEvent;
}

impl SingletonEntity for PrivacySettings {}
