use std::rc::Rc;

use ai::diff_validation::DiffType;
#[cfg(not(target_family = "wasm"))]
use futures::{FutureExt, channel::oneshot, future::Shared};
#[cfg(not(target_family = "wasm"))]
use std::path::PathBuf;
#[cfg(not(target_family = "wasm"))]
use warp_files::FileModel;
use warp_util::file::FileId;
#[cfg(not(target_family = "wasm"))]
use warp_util::file::FileSaveError;
use warp_util::standardized_path::StandardizedPath;
#[cfg(not(target_family = "wasm"))]
use warpui::SingletonEntity;
use warpui::elements::ChildView;
use warpui::{AppContext, Element, Entity, TypedActionView, View, ViewContext, ViewHandle};

use super::DiffResult;
use super::diff_viewer::{DiffViewer, DisplayMode};
use super::editor::NavBarBehavior;
use super::editor::scroll::{ScrollPosition, ScrollTrigger};
use super::editor::view::{CodeEditorEvent, CodeEditorView};
#[cfg(not(target_family = "wasm"))]
use crate::ai::blocklist::inline_action::code_diff_view::DiffSessionType;
use crate::editor::InteractionState;

#[cfg(all(test, not(target_family = "wasm")))]
#[path = "inline_diff_tests.rs"]
mod tests;

pub(crate) type SaveFuture = futures::future::BoxFuture<'static, Result<(), String>>;

#[cfg(not(target_family = "wasm"))]
enum PersistenceState {
    Unpersisted,
    Persisting(Shared<SaveFuture>),
    Persisted,
    Restoring(Shared<SaveFuture>),
    Restored,
}

pub enum InlineDiffViewEvent {
    DiffStatusUpdated,
    #[cfg(not(target_family = "wasm"))]
    FileLoaded,
    #[cfg(not(target_family = "wasm"))]
    FileSaved,
    #[cfg(not(target_family = "wasm"))]
    FailedToSave {
        error: Rc<FileSaveError>,
    },
    DiffAccepted {
        diff: Rc<DiffResult>,
    },
    UserEdited,
}

/// An inline diff viewer with optional file-backed save support.
///
/// When a backing file is registered (via [`Self::register_file`]), this view supports the full
/// accept/save/revert lifecycle through `FileModel`. Without a registered file, it behaves
/// as a read-only diff viewer (e.g. for WASM or restored conversations).
pub struct InlineDiffView {
    editor: ViewHandle<CodeEditorView>,
    diff_type: Option<DiffType>,
    file_path: Option<StandardizedPath>,
    /// Whether the user has edited the diff content.
    was_edited: bool,
    /// `FileModel` file ID for the backing file. Set via [`Self::register_file`].
    ///
    /// When `Some`:
    /// - The editor is editable (interaction state follows the `DisplayMode` rules).
    /// - Accept, save, and revert operations write through `FileModel`.
    ///
    /// When `None` (WASM, restored conversations, or before registration):
    /// - The editor is selection-only (never editable).
    /// - Accept, save, and revert are no-ops.
    backing_file_id: Option<FileId>,
    #[cfg(not(target_family = "wasm"))]
    session_type: Option<DiffSessionType>,
    #[cfg(not(target_family = "wasm"))]
    persistence: PersistenceState,
}

impl InlineDiffView {
    pub fn new(
        editor: ViewHandle<CodeEditorView>,
        diff_type: Option<DiffType>,
        display_mode: Option<DisplayMode>,
        file_path: Option<StandardizedPath>,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        ctx.subscribe_to_view(&editor, |me, _view, event, ctx| match event {
            CodeEditorEvent::DiffUpdated => {
                ctx.emit(InlineDiffViewEvent::DiffStatusUpdated);
            }
            CodeEditorEvent::UnifiedDiffComputed(diff) => {
                ctx.emit(InlineDiffViewEvent::DiffAccepted { diff: diff.clone() });
            }
            CodeEditorEvent::ContentChanged { origin } if origin.from_user() && !me.was_edited => {
                me.was_edited = true;
                ctx.emit(InlineDiffViewEvent::UserEdited);
            }
            _ => {}
        });

        let model = Self {
            editor,
            diff_type,
            file_path,
            was_edited: false,
            backing_file_id: None,
            #[cfg(not(target_family = "wasm"))]
            session_type: None,
            #[cfg(not(target_family = "wasm"))]
            persistence: PersistenceState::Unpersisted,
        };

        model.apply_diffs_if_any(ctx);
        if let Some(display_mode) = display_mode {
            model.set_display_mode(display_mode, ctx);
        }

        model
    }

    /// Register a file with `FileModel` for save support.
    ///
    /// The `session_type` determines whether the file is local or remote.
    /// For `Local`, the file is registered by path on the local filesystem.
    /// For `Remote`, the file is registered against the remote backend so
    /// that `save()` / `delete()` dispatch over the wire via
    /// `RemoteServerClient`.
    ///
    /// This must be called after construction for non-WASM environments.
    #[cfg(not(target_family = "wasm"))]
    pub fn register_file(&mut self, session_type: &DiffSessionType, ctx: &mut ViewContext<Self>) {
        let Some(file_path) = &self.file_path else {
            return;
        };

        let file_model = FileModel::handle(ctx);
        let file_id = match session_type {
            DiffSessionType::Local => {
                let Some(local_path) = file_path.to_local_path() else {
                    crate::safe_error!(
                        safe: (
                            "Failed to convert StandardizedPath to local path; diff will be \
                            read-only"
                        ),
                        full: (
                            "Failed to convert StandardizedPath to local path: {file_path}; diff \
                            will be read-only"
                        )
                    );
                    return;
                };
                file_model.update(ctx, |file_model, ctx| {
                    file_model.register_file_path(&local_path, false, ctx)
                })
            }
            DiffSessionType::Remote(host_id) => {
                let host_id = host_id.clone();
                let remote_path = file_path.clone();
                file_model.update(ctx, |file_model, _ctx| {
                    file_model.register_remote_file(host_id, remote_path)
                })
            }
        };

        self.session_type = Some(session_type.clone());
        self.finish_file_registration(file_id, ctx);
    }

    /// Common registration logic: subscribes to events and sets the
    /// backing file ID after a file has been registered with `FileModel`.
    #[cfg(not(target_family = "wasm"))]
    fn finish_file_registration(&mut self, file_id: FileId, ctx: &mut ViewContext<Self>) {
        let file_model = FileModel::handle(ctx);

        let version = self.editor.as_ref(ctx).version(ctx);
        file_model.update(ctx, |file_model, _ctx| {
            file_model.set_version(file_id, version);
        });

        self.backing_file_id = Some(file_id);

        ctx.emit(InlineDiffViewEvent::FileLoaded);
    }

    fn apply_diffs_if_any(&self, ctx: &mut ViewContext<Self>) {
        let Some(diff) = self.diff_type.clone() else {
            return;
        };

        let deltas = match diff {
            DiffType::Create { delta } => vec![delta],
            DiffType::Update { mut deltas, .. } => {
                deltas.sort_by_key(|delta| delta.replacement_line_range.start);
                deltas
            }
            DiffType::Delete { delta } => vec![delta],
        };

        if deltas.is_empty() {
            return;
        }

        self.editor.update(ctx, |editor, ctx| {
            editor.apply_diffs(deltas, ctx);
            editor.toggle_diff_nav(None, ctx);
            editor.set_pending_scroll(ScrollTrigger::new(
                ScrollPosition::FocusedDiffHunk,
                editor.buffer_version(ctx),
            ));
        });
    }

    #[cfg(not(target_family = "wasm"))]
    fn save_content(&mut self, ctx: &mut ViewContext<Self>) -> Option<SaveFuture> {
        let file_id = self.backing_file_id?;
        match &self.persistence {
            PersistenceState::Persisting(future) => return Some(future.clone().boxed()),
            PersistenceState::Persisted => return Some(futures::future::ready(Ok(())).boxed()),
            PersistenceState::Restoring(_) => {
                return Some(
                    futures::future::ready(
                        Err("Cannot accept while restoring the edit".to_owned()),
                    )
                    .boxed(),
                );
            }
            PersistenceState::Unpersisted | PersistenceState::Restored => {}
        }
        let content = self.editor.as_ref(ctx).text(ctx).into_string();
        let version = self.editor.as_ref(ctx).version(ctx);
        let operation = FileModel::handle(ctx).update(ctx, |model, ctx| match &self.diff_type {
            Some(DiffType::Delete { .. }) => model.delete_with_completion(file_id, version, ctx),
            Some(DiffType::Update {
                rename: Some(to), ..
            }) if self.is_move() => {
                model.rename_and_save_with_completion(file_id, to.clone(), content, version, ctx)
            }
            _ => model.save_with_completion(file_id, content, version, ctx),
        });
        Some(match operation {
            Ok(future) => self.track_operation(future, false, ctx),
            Err(error) => {
                let message = error.to_string();
                ctx.emit(InlineDiffViewEvent::FailedToSave {
                    error: Rc::new(error),
                });
                futures::future::ready(Err(message)).boxed()
            }
        })
    }

    pub fn is_move(&self) -> bool {
        matches!(&self.diff_type, Some(DiffType::Update { rename: Some(to), .. })
            if StandardizedPath::try_new(&to.to_string_lossy()).ok().as_ref() != self.file_path.as_ref())
    }

    pub fn persisted_file_path(&self) -> Option<String> {
        if self.is_move()
            && let Some(DiffType::Update {
                rename: Some(to), ..
            }) = &self.diff_type
        {
            Some(to.to_string_lossy().into_owned())
        } else {
            self.file_path.as_ref().map(ToString::to_string)
        }
    }

    /// Starts persistence and retains the fork's unified-diff result event.
    pub fn accept_and_save_diff(&mut self, ctx: &mut ViewContext<Self>) -> Option<SaveFuture> {
        if self.backing_file_id.is_none() {
            return None;
        }
        if let Some(file_path) = &self.file_path {
            let file_name = file_path.to_string();
            self.editor.update(ctx, |editor, ctx| {
                editor.retrieve_unified_diff(file_name, ctx)
            });
        }
        #[cfg(not(target_family = "wasm"))]
        {
            self.save_content(ctx)
        }
        #[cfg(target_family = "wasm")]
        {
            None
        }
    }

    #[cfg(not(target_family = "wasm"))]
    fn track_operation(
        &mut self,
        operation: SaveFuture,
        restoring: bool,
        ctx: &mut ViewContext<Self>,
    ) -> SaveFuture {
        let (tx, rx) = oneshot::channel();
        let completion = async move {
            rx.await
                .unwrap_or_else(|_| Err("File operation completion was cancelled".to_owned()))
        }
        .boxed()
        .shared();
        self.persistence = if restoring {
            PersistenceState::Restoring(completion.clone())
        } else {
            PersistenceState::Persisting(completion.clone())
        };
        ctx.spawn(operation, move |me, result, ctx| {
            me.persistence = match (&result, restoring) {
                (Ok(()), true) => PersistenceState::Restored,
                (Ok(()), false) | (Err(_), true) => PersistenceState::Persisted,
                (Err(_), false) => PersistenceState::Unpersisted,
            };
            if !restoring {
                match &result {
                    Ok(()) => ctx.emit(InlineDiffViewEvent::FileSaved),
                    Err(error) => ctx.emit(InlineDiffViewEvent::FailedToSave {
                        error: Rc::new(FileSaveError::Other(error.clone())),
                    }),
                }
            }
            let _ = tx.send(result);
        });
        completion.boxed()
    }

    /// Restores persisted operations and shares concurrent or repeated undo completion.
    pub fn restore_and_save_diff(
        &mut self,
        ctx: &mut ViewContext<Self>,
    ) -> Result<SaveFuture, String> {
        #[cfg(target_family = "wasm")]
        {
            let _ = ctx;
            Ok(Box::pin(futures::future::ready(Ok(()))))
        }
        #[cfg(not(target_family = "wasm"))]
        {
            match &self.persistence {
                PersistenceState::Restoring(future) => return Ok(future.clone().boxed()),
                PersistenceState::Unpersisted | PersistenceState::Restored => {
                    return Ok(futures::future::ready(Ok(())).boxed());
                }
                PersistenceState::Persisting(_) => {
                    return Err("File operation is still in flight".to_owned());
                }
                PersistenceState::Persisted => {}
            }
            let file_id = self
                .backing_file_id
                .ok_or_else(|| "Missing backing file".to_owned())?;
            let version = self.editor.as_ref(ctx).version(ctx);
            let model = FileModel::handle(ctx);
            let operation = match &self.diff_type {
                Some(DiffType::Create { .. }) => model.update(ctx, |model, ctx| {
                    model.delete_with_completion(file_id, version, ctx)
                }),
                diff => {
                    let base = self
                        .editor
                        .as_ref(ctx)
                        .model
                        .as_ref(ctx)
                        .diff()
                        .as_ref(ctx)
                        .base()
                        .ok_or_else(|| "Missing base content".to_owned())?
                        .to_string();
                    match diff {
                        Some(DiffType::Delete { .. })
                            if matches!(self.session_type, Some(DiffSessionType::Local)) =>
                        {
                            let path = PathBuf::from(
                                self.file_path
                                    .as_ref()
                                    .ok_or_else(|| "Missing original path".to_owned())?
                                    .as_str(),
                            );
                            model.update(ctx, |model, ctx| {
                                let restored = model.register_file_path(&path, false, ctx);
                                let result =
                                    model.save_with_completion(restored, base, version, ctx);
                                model.unsubscribe(restored, ctx);
                                result
                            })
                        }
                        Some(DiffType::Update {
                            rename: Some(to), ..
                        }) if self.is_move() => {
                            let target = model.update(ctx, |model, ctx| {
                                match self.session_type.as_ref() {
                                    Some(DiffSessionType::Local) => {
                                        Ok(model.register_file_path(to, false, ctx))
                                    }
                                    Some(DiffSessionType::Remote(host)) => {
                                        StandardizedPath::try_new(&to.to_string_lossy())
                                            .map(|path| {
                                                model.register_remote_file(host.clone(), path)
                                            })
                                            .map_err(|error| error.to_string())
                                    }
                                    None => Err("Missing registered session".to_owned()),
                                }
                            })?;
                            let original = PathBuf::from(
                                self.file_path
                                    .as_ref()
                                    .ok_or_else(|| "Missing original path".to_owned())?
                                    .as_str(),
                            );
                            model.update(ctx, |model, ctx| {
                                let result = model.rename_and_save_with_completion(
                                    target, original, base, version, ctx,
                                );
                                model.unsubscribe(target, ctx);
                                result
                            })
                        }
                        _ => model.update(ctx, |model, ctx| {
                            model.save_with_completion(file_id, base, version, ctx)
                        }),
                    }
                }
            }
            .map_err(|error| error.to_string())?;
            Ok(self.track_operation(operation, true, ctx))
        }
    }
}

impl InlineDiffView {
    pub fn file_path(&self) -> Option<&StandardizedPath> {
        self.file_path.as_ref()
    }

    pub fn file_name(&self) -> Option<String> {
        self.file_path()
            .map(|p| p.file_name().unwrap_or_default().to_owned())
    }
}

impl DiffViewer for InlineDiffView {
    fn editor(&self) -> &ViewHandle<CodeEditorView> {
        &self.editor
    }

    fn diff(&self) -> Option<&DiffType> {
        self.diff_type.as_ref()
    }

    fn was_edited(&self) -> bool {
        self.was_edited
    }

    fn set_display_mode(&self, mode: DisplayMode, ctx: &mut ViewContext<Self>) {
        let is_delete = matches!(self.diff(), Some(DiffType::Delete { .. }));
        let interaction_state = if self.backing_file_id.is_some() {
            mode.interaction_state(is_delete)
        } else {
            // No file registered (e.g. WASM or restored conversations): always read-only.
            InteractionState::Selectable
        };
        self.editor().update(ctx, |editor, ctx| {
            editor.set_scroll_wheel_behavior(mode.scroll_wheel_behavior());
            editor.set_vertical_expansion_behavior(mode.vertical_expansion_behavior(), ctx);
            editor.set_vertical_scrollbar_appearance(mode.scrollbar_appearance());
            editor.set_horizontal_scrollbar_appearance(mode.scrollbar_appearance());
            editor.set_interaction_state(interaction_state, ctx);
            editor.set_show_nav_bar(mode.show_nav_bar());
            editor.set_nav_bar_behavior(NavBarBehavior::NotClosable, ctx);
        });
    }
}

impl Entity for InlineDiffView {
    type Event = InlineDiffViewEvent;
}

impl View for InlineDiffView {
    fn ui_name() -> &'static str {
        "InlineDiffView"
    }

    fn render(&self, _app: &AppContext) -> Box<dyn Element> {
        ChildView::new(&self.editor).finish()
    }
}

impl TypedActionView for InlineDiffView {
    type Action = ();
}
