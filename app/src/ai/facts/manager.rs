use std::collections::HashMap;
use std::path::Path;

use warpui::{Entity, EntityId, ModelContext, SingletonEntity, ViewHandle, WindowId};

use crate::PaneViewLocator;
use crate::ai::facts::AIFactView;
use crate::ai::facts::local_memory::{
    LocalMemoryError, LocalMemoryRecord, LocalMemoryRepository, LocalMemoryScope,
};
use crate::pane_group::{AIFactPane, PaneContent};

/// Singleton model to manage state of AI fact panes across multiple windows
/// (where only one AI fact pane can exist per window). Specifically:
/// - Maintains AI fact view handles to preserve state when panes are hidden
/// - Tracks currently open AI fact panes and their location
pub struct AIFactManager {
    panes: HashMap<WindowId, AIFactPaneData>,
    memory_repository: LocalMemoryRepository,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AIFactManagerEvent {
    MemoriesChanged,
}

struct AIFactPaneData {
    locator: Option<PaneViewLocator>,
    view: ViewHandle<AIFactView>,
}

impl AIFactManager {
    pub fn new() -> Self {
        let memory_repository =
            LocalMemoryRepository::open_current_scope().unwrap_or_else(|error| {
                log::error!("Failed to initialize local memory: {error}");
                LocalMemoryRepository::unavailable(error.to_string())
            });
        Self {
            panes: HashMap::new(),
            memory_repository,
        }
    }

    pub fn memory_startup_error(&self) -> Option<String> {
        self.memory_repository.startup_error()
    }

    pub fn list_memories(&self) -> Result<Vec<LocalMemoryRecord>, LocalMemoryError> {
        self.memory_repository.list()
    }

    pub fn search_memories(
        &self,
        query: &str,
        current_directory: Option<&Path>,
    ) -> Result<Vec<LocalMemoryRecord>, LocalMemoryError> {
        self.memory_repository.search(query, current_directory)
    }

    pub fn create_memory(
        &mut self,
        scope: LocalMemoryScope,
        title: &str,
        content: &str,
        ctx: &mut ModelContext<Self>,
    ) -> Result<LocalMemoryRecord, LocalMemoryError> {
        let record = self.memory_repository.create(scope, title, content)?;
        ctx.emit(AIFactManagerEvent::MemoriesChanged);
        Ok(record)
    }

    pub fn update_memory(
        &mut self,
        id: uuid::Uuid,
        expected_revision: i64,
        scope: LocalMemoryScope,
        title: &str,
        content: &str,
        ctx: &mut ModelContext<Self>,
    ) -> Result<LocalMemoryRecord, LocalMemoryError> {
        let record = self
            .memory_repository
            .update(id, expected_revision, scope, title, content)?;
        ctx.emit(AIFactManagerEvent::MemoriesChanged);
        Ok(record)
    }

    pub fn delete_memory(
        &mut self,
        id: uuid::Uuid,
        expected_revision: i64,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), LocalMemoryError> {
        self.memory_repository.delete(id, expected_revision)?;
        ctx.emit(AIFactManagerEvent::MemoriesChanged);
        Ok(())
    }

    pub fn ai_fact_view(&self, window_id: WindowId) -> ViewHandle<AIFactView> {
        self.panes
            .get(&window_id)
            .expect("Window should have corresponding AI fact view")
            .view
            .clone()
    }

    pub fn register_view(&mut self, window_id: WindowId, view: ViewHandle<AIFactView>) {
        if let Some(data) = self.panes.get_mut(&window_id) {
            data.view = view;
        } else {
            self.panes.insert(
                window_id,
                AIFactPaneData {
                    locator: None,
                    view,
                },
            );
        }
    }

    pub fn find_pane(&self, window_id: WindowId) -> Option<PaneViewLocator> {
        self.panes.get(&window_id).and_then(|data| data.locator)
    }

    pub fn register_pane(
        &mut self,
        pane: &AIFactPane,
        pane_group_id: EntityId,
        window_id: WindowId,
        _ctx: &mut ModelContext<Self>,
    ) {
        if let Some(data) = self.panes.get_mut(&window_id) {
            data.locator = Some(PaneViewLocator {
                pane_group_id,
                pane_id: pane.id(),
            });
        } else {
            log::warn!("AI fact view should already exist for AI fact pane");
        }
    }

    pub fn deregister_pane(&mut self, window_id: &WindowId, _ctx: &mut ModelContext<Self>) {
        if let Some(data) = self.panes.get_mut(window_id) {
            data.locator = None;
        }
    }
}

impl Entity for AIFactManager {
    type Event = AIFactManagerEvent;
}

impl SingletonEntity for AIFactManager {}
