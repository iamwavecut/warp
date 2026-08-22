cfg_if::cfg_if! {
    if #[cfg(not(feature = "local_fs"))] {
        mod dummy_skill_manager;
        pub use dummy_skill_manager::SkillManager;
    }
}

pub use ai::skills::SkillReference;
use std::path::{Path, PathBuf};
use warp_util::local_or_remote_path::LocalOrRemotePath;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SkillOpenOrigin {
    ReadSkill,
    ReadFiles,
    EditFiles,
    OpenSkillCommand,
}

/// Emitted after the local skill catalog changes.
///
/// Consumers use this event to refresh views backed by the filesystem watcher. The event is
/// shared with the no-filesystem implementation so the surrounding UI keeps the same lifecycle
/// contract on every supported target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillManagerEvent {
    SkillsChanged,
}

#[cfg(not(target_family = "wasm"))]
mod global_skills;
#[cfg(not(target_family = "wasm"))]
pub use global_skills::{filter_skills_by_spec, resolve_skill_repos};

mod listed_skill;
pub use listed_skill::SkillDescriptor;

mod skill_utils;
pub use skill_utils::{
    icon_override_for_skill_name, list_skills_if_changed, render_skill_button,
    skill_path_from_file_path,
};

pub trait SkillPathQuery {
    fn to_skill_location(&self) -> LocalOrRemotePath;
}

impl SkillPathQuery for LocalOrRemotePath {
    fn to_skill_location(&self) -> LocalOrRemotePath {
        self.clone()
    }
}

impl SkillPathQuery for Path {
    fn to_skill_location(&self) -> LocalOrRemotePath {
        LocalOrRemotePath::Local(self.to_path_buf())
    }
}

impl SkillPathQuery for PathBuf {
    fn to_skill_location(&self) -> LocalOrRemotePath {
        LocalOrRemotePath::Local(self.clone())
    }
}

#[cfg(not(target_family = "wasm"))]
mod resolve_skill_spec;
#[cfg(not(target_family = "wasm"))]
pub use resolve_skill_spec::{
    ResolveSkillError, ResolvedSkill, clone_repo_for_skill, resolve_skill_spec,
};

cfg_if::cfg_if! {
    if #[cfg(feature = "local_fs")] {
        mod skill_manager;
        pub use skill_manager::{read_skills_from_directories, SkillManager, SkillWatcher};

    }
}
