# Code review diff selector redesign — tech spec (APP-4199)
Companion to `specs/APP-4199/PRODUCT.md`. Behavior invariants are referenced by number (e.g. "PRODUCT.md §10") rather than restated.
## Context
The diff-against control in the code review header is currently a `FilterableDropdown<CodeReviewAction>`:
- Constructed in `app/src/code_review/code_review_view.rs` and styled in `apply_diff_mode_dropdown_style`.
- Wrapped in the header via `render_diff_mode_dropdown` in `app/src/code_review/code_review_header/header_revamp.rs`.
- Populated by `CodeReviewView::setup_dropdown` with `DropdownItem`s (display text + action only — no custom icon slot).
- Selection dispatches `CodeReviewAction::SetDiffMode(DiffMode)`, handled in `code_review_view.rs` which calls `DiffStateModel::set_diff_mode` in `app/src/code_review/diff_state.rs`.
### Reuse evaluation
Two existing widgets were considered and rejected before landing on a purpose-built component.
`FilterableDropdown` (`app/src/view_components/filterable_dropdown.rs`) is a generic two-state widget. Its closed state renders a `TextAndIcon` button with a trailing `ChevronDown`; its open state *replaces* that button in-place with a search input (`render_closed_top_bar` vs `render_filter_input`). Rows come from flat `DropdownItem { display_text, action }` values rendered by a shared `Menu<DropdownAction<A>>`, which surfaces selection as a row highlight, not a fixed-width checkmark slot. PRODUCT.md requires (a) a `ButtonVariant::Text` trigger with a **leading** decorative icon and no chevron (§1–§2, §18), (b) the trigger to remain visible in an "open" state while the picker opens **below** it with the search input inside the picker (§4, §7–§8), and (c) a reserved check slot on each row with labels vertically aligned across selected/unselected rows (§10, §25). Reaching those from `FilterableDropdown` means rewriting the trigger, the open/closed structure, and row rendering — at which point we're wrapping a generic shell around a near-total override.
`DisplayChipMenu` (`app/src/context_chips/display_menu.rs`, used by the agent-bar `ContextChip(ShellGitBranch)`) has the right *shape* — trigger above, pinned search input inside a floating menu, up/down/enter/escape bindings, fuzzy filtering, a `GenericMenuItem` trait with icon/name/right-side slots — but it's semantically coupled to context chips: the chip menu replaces the agent prompt at the call site, and its row styling, padding, selection highlight, and variant switches are tuned around that family of surfaces. Dropping a `ChipMenuType::CodeReview` variant into the middle of that component to dress it for a code-review picker conflates two unrelated UIs and expands surface area on a component we don't want code review to reach into.
## Proposed changes
Build a small, code-review-scoped picker: `DiffSelector` owns the trigger button and an overlay menu view, `CodeReviewDiffMenu`, that lives next to it in `app/src/code_review/`. Neither `FilterableDropdown` nor `DisplayChipMenu` is touched by this change.
### New module: `app/src/code_review/diff_selector.rs`
Split out of `code_review_header/` to keep the header file from growing. Contains:
1. `DiffTarget` — a plain value type representing one selectable row.
   - Fields: `label: String`, `mode: DiffMode`, `is_selected: bool`.
   - No trait impls beyond `Debug` / `Clone`; the selector menu reads fields directly.
2. `DiffSelector` — one view that owns both the trigger button and the overlay menu.
   - Holds a `ViewHandle<CodeReviewDiffMenu>`, a `menu_open: bool`, a `MouseStateHandle` for the trigger, and a cached `trigger_label`.
   - Registers a view-scoped `DiffSelectorAction::Toggle` keybinding (`enter` / `space`) so the focused trigger opens/closes the picker with the keyboard (PRODUCT.md §5).
   - `render` paints a single `ButtonVariant::Text` button containing the decorative `Icon::SwitchHorizontal01` and the label, and mounts `ChildView<CodeReviewDiffMenu>` as an `OffsetPositioning` overlay anchored `BottomLeft/TopLeft` with a small y offset when `menu_open` is true (PRODUCT.md §1–§7, §19).
   - `on_focus` forwards focus to the menu while it's open so the menu's search input receives keystrokes.
   - Exposes `toggle`, `close`, and `set_targets(Vec<DiffTarget>, &mut ViewContext<Self>)`.
### New module: `app/src/code_review/diff_menu.rs`
A self-contained overlay menu sized for the code-review diff picker. Responsibilities:
- Owns a single-line `EditorView` for search with placeholder `Search diff sets or branches to compare…` (PRODUCT.md §8).
- Stores `targets: Vec<DiffTarget>` plus a `filtered: Vec<(usize, Option<FuzzyMatchResult>)>` and a `selected_index` into `filtered`. Filter reuses `fuzzy_match::match_indices_case_insensitive` (the same primitive `DisplayChipMenu`, command palettes, and pickers across the app use) for membership **only** — the score is discarded so rows keep their original order (§9, §12). The returned `matched_indices` are kept per row so we can bold matched characters in the label (§11).
- Renders two stacked regions inside a `Dismiss`: the search input pinned at the top, and a vertically-scrollable `UniformList` of rows below. Each row renders a fixed-width check slot (`Icon::Check` when `target.is_selected`, else an empty spacer of the same width) followed by the label — giving the left-aligned checkmark with aligned labels required by §10 / §25. Selection (keyboard focus) is shown via a distinct row background; mouse hover is a separate, lighter state (§24).
- Renders a `No matches` empty state when the query is non-empty and nothing matches (§13). Empty query with zero targets renders nothing.
- Registers view-scoped fixed bindings (`up`, `down`, `enter`, `escape`) on `CodeReviewDiffMenu::ui_name()` for keyboard nav (§21–§22). Up from the first row returns focus to the search input. `Escape` emits `Close`.
- Emits `CodeReviewDiffMenuEvent::Select(DiffMode)` and `CodeReviewDiffMenuEvent::Close`.
- `on_focus` forwards focus to the search input so keystrokes flow into the filter (§11).
- Exposes `set_targets(Vec<DiffTarget>, &mut ViewContext<Self>)`, which replaces the row set, resets scroll / selected index to the top, and re-applies the active filter (§14).
- Menu width, padding, corner radius, and drop shadow are defined locally as module constants. Max list height caps at ~200px with vertical scrolling (§15).
### Replace the trigger in the header
- `app/src/code_review/code_review_header/header_revamp.rs` (`render_diff_mode_dropdown`) renders `DiffSelector` instead of the `FilterableDropdown`'s `ChildView`.
- Remove `FilterableDropdown` from `CodeReviewView` state: delete `diff_mode_dropdown` from `CodeReviewHeaderFields` and from `CodeReviewView`, along with the builder block, `apply_diff_mode_dropdown_style` / `refresh_diff_mode_dropdown_style`, and the appearance subscription that drove them.
- Add a `diff_selector: ViewHandle<DiffSelector>` field on `CodeReviewView` and plumb it through `CodeReviewHeaderFields` in its place.
### Data flow
- `CodeReviewView::build_diff_targets(&self, ctx) -> Vec<DiffTarget>` produces the rows in the same order as today's `setup_dropdown` (`Uncommitted changes` first, then the current `OtherBranch` if not already in the list, then main, then other branches). Pure — does not touch the selector view. Exposed at `pub(crate)` so unit tests can assert ordering without reading back through `CodeReviewDiffMenu` internals.
- `update_diff_selector_selection(&mut self, ctx)` calls `build_diff_targets` and pushes the result into the selector with `DiffSelector::set_targets`. Called from `fetch_branches_and_setup_dropdown`, `DiffStateModelEvent::CurrentBranchChanged`, and `DiffStateModelEvent::DiffModeChanged`.
- On selection the `DiffSelector` emits `DiffSelectorEvent::SelectMode(DiffMode)`. `CodeReviewView` subscribes and calls its shared `apply_diff_mode` helper (which also backs `CodeReviewAction::SetDiffMode` from the legacy action path).
- The header label comes from the currently selected `DiffTarget`; if none are selected (e.g. during first load) it falls back to "Uncommitted changes".
### Per-target stats
Out of scope (PRODUCT.md Non-goals). No new stats computation, caching, or `right_side_element` plumbing is introduced by this change.
### Swap icon
`Icon::SwitchHorizontal01` (pointing at `app/assets/bundled/svg/switch-horizontal-01.svg`) is rendered inside the `DiffSelector`'s trigger as a decorative element with no click handler of its own (PRODUCT.md §2, §28). The whole button is a single hit target.
### Action additions
- `DiffSelectorAction::Toggle` is a view-scoped typed action used by the trigger's `on_click` handler and by the `Enter`/`Space` fixed bindings.
- Dismissal (outside click / `Escape`) flows through `CodeReviewDiffMenuEvent::Close`; `DiffSelector` subscribes and sets `menu_open = false`. Outside-click dismissal is driven by a `Dismiss` wrapping the menu card.
- No new variants are added to `CodeReviewAction`. `CodeReviewAction::SetDiffMode` is retained unchanged for backward compatibility with non-selector callers.
## Risks and mitigations
- **Duplicated searchable-picker machinery.** `CodeReviewDiffMenu` reimplements pieces of `DisplayChipMenu` (search input wiring, up/down/enter bindings, filtered list with highlight). Mitigation: scope is small (label-only rows, no footer/sidecar), the filter primitive itself is shared via `fuzzy_match`, and the Follow-ups section flags consolidation into a shared picker once the second real consumer lands.
- **Focus handling on open/close.** `DiffSelector` calls `ctx.focus(&self.menu)` from `toggle` when opening, and `CodeReviewDiffMenu::on_focus` forwards focus to its search input so keystrokes reach the filter (PRODUCT.md §11). `DiffSelector::on_focus` also forwards to the menu while it's open so tabbing back to the trigger does not strand the picker.
- **Removing `FilterableDropdown` usage.** The diff selector was the only caller of `FilterableDropdown` in the code-review area. Other callers across the app are unaffected; the component stays.
- **Ordering under filter.** `CodeReviewDiffMenu` uses `fuzzy_match::match_indices_case_insensitive` for membership only and discards the score; rows must never be reordered by match score or §12 is violated. Enforced by unit test (`diff_menu_filter_preserves_order`).
## Testing and validation
Reference PRODUCT.md invariants by number.
### Unit tests (Rust, alongside `code_review_view_tests.rs`)
- `diff_selector_items_preserve_legacy_order` — asserts `CodeReviewView::build_diff_targets` produces the same `DiffMode` sequence as today's selector for the same `available_branches` + `current_mode`. Covers §9, §26.
- `diff_selector_marks_selected` — after `update_diff_selector_selection`, exactly one `DiffTarget` has `is_selected = true`, matching `DiffStateModel::diff_mode()`. Covers §25.
### Integration tests (`crates/integration`, follow `warp-integration-test` skill)
- `code_review_diff_selector_opens_below` — open code review with prepared fixture, click the trigger, assert the menu is rendered below the trigger and the search input has focus. Covers §7, §11.
- `code_review_diff_selector_filters_and_selects` — type a substring, assert only matching rows remain in order; click a row, assert the button label updates and `DiffMode` changes via `DiffStateModel`. Covers §11, §12, §16.
- `code_review_diff_selector_keyboard_nav` — open, press `ArrowDown` into list, `Enter` to select, verify selection changed; re-open, press `Escape`, verify menu closes with no change. Covers §21, §22.
- `code_review_diff_selector_no_matches` — type a query with no matches, assert the `No matches` empty state and that the search input remains focused; clear query, assert full list returns. Covers §13, §14.
### Manual verification
Boot `cargo run --features with_local_server`, open a repo with several branches, and walk PRODUCT.md §1–§28 by hand. Capture a before/after screenshot and attach to the PR.
### Presubmit
`./script/presubmit` (fmt + clippy + tests) before opening the PR. The skill `fix-errors` covers common WASM gotchas.
## Follow-ups
- Consider unifying `FilterableDropdown`, `DisplayChipMenu`, and `CodeReviewDiffMenu` into one shared "searchable picker" primitive once there's a second concrete consumer with the same shape as the code-review menu. Not in scope for this PR.
- If the swap icon ever becomes interactive (reverse base/target), spec that as a separate feature — the button hit region and action plumbing assume a single action today.
- If we decide to surface per-target diff stats in the picker in the future, add a lightweight right-side slot to `CodeReviewDiffMenu` row rendering and drive it from `DiffStateModel`. Explicitly out of scope here.
