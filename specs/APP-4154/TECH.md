# APP-4154: Audit macOS ObjC allocations for leaks

## Problem


This spec covers the follow-up audit APP-4104 explicitly flagged: walk every NSString production site (Rust) and every `alloc]`/`new]`/`copy]`/`mutableCopy]` in every non-ARC `.m` file, classify each call site, and fix the leaked ones. The audit ships as multiple small, semantically-related PRs so each slice can be reviewed independently.

## Scope

**Rust files** that produce or pass ObjC objects (via `make_nsstring`, `NSString::alloc`, or `msg_send![class!(X), alloc]`) across `app/src/` and `crates/warpui*`. See `nsstring_checklist.md` for the full row set.

**Non-ARC ObjC translation units** (compiled via `cc::Build` without `-fobjc-arc`):
- `crates/warpui/src/platform/mac/objc/`: `alert.m`, `app.m`, `fullscreen_queue.m`, `host_view.m`, `hotkey.m`, `keycode.m`, `menus.m`, `notifications/notifications.m`, `reachability.m`, `window.m`, `window_blur.m`

**Out of scope:**
- `app/DockTilePlugin/WarpDockTilePlugin.m` (ARC, per `app/DockTilePlugin/Makefile:5`).
- Switching non-ARC files to ARC wholesale.
- Non-macOS targets.

## Ambient autorelease pools

`make_nsstring` (`crates/warpui/src/platform/mac/mod.rs:34`) autoreleases its NSString, so it only works if an `NSAutoreleasePool` is active for the calling scope. Several contexts create one for us:

- **AppKit main event loop** drains a pool around each event dispatch (delegate callbacks, menu selectors, key/mouse events, timer callbacks).
- **GCD blocks** drain a pool around each block invocation (see comment at `crates/warpui/src/platform/mac/objc/reachability.m:75-79`).
- **`NSThread` detaches** and ObjC methods wrapped in `@autoreleasepool { ... }` inherit the same guarantee.

Contexts that do **not** provide an ambient pool:

- **Rust-spawned threads** (`std::thread::spawn`, Tokio workers, `async_channel` recv loops).
- **`lazy_static` / `OnceCell` initializers** triggered from a non-AppKit thread.
- **Early `main`** before the AppKit event loop starts.

## Decision rule for each row

The audit does not unconditionally wrap every call site in a pool; redundant pools add a per-call push/pop on hot AppKit-event paths. For each row, pick one of:

1. **`ambient`** — no change. Use when the call is provably reached only from an AppKit event handler or GCD block, and the scope creates a small bounded number of autoreleased temporaries.
3. **`autorelease-helper`** — replace retained `NSString::alloc(nil).init_str(...)` with `make_nsstring`, or swap `[[Class alloc] init]` for a convenience constructor that returns an autoreleased instance (`[NSMutableArray array]`, `[NSString stringWith...]`). Typically combined with (1) or (2).

Default when uncertain about thread origin: (2). Nested pools are correct and cheap.

`ambient` vs `local-pool` is a perf/peak-memory call, not correctness: an ambient AppKit-event pool is sufficient to prevent leaks on a path that runs on that thread, but a nested `local-pool` drains earlier and bounds peak memory at the cost of a per-call push/pop. Prefer `ambient` only when adding a nested pool has measurable per-call overhead on a hot path; otherwise default to `local-pool`.

### `hot/cold` classification

Each row must be marked `hot` or `cold` so the `ambient`/`local-pool` trade-off above can be applied consistently. Treat any call that runs on a per-frame, per-keystroke, per-mouse-move, per-log-event, or otherwise recurring path as `hot`. Treat one-shot init code, user-initiated rare actions (menu click, file picker, settings change), and error/alert paths as `cold`. If in doubt, mark `hot` — the consequence is at worst an unnecessary pool push/pop, which is safer than unbounded peak memory.

## How to use the checklists

`nsstring_checklist.md` and `objc_checklist.md` list every call site we care about, grouped by the PR batch that will ship its fix. Each row has the form:

```
- [ ] path/file.ext:line — function/symbol — disposition — thread-origin — hot/cold — strategy — action
```

The trailing columns start as TODOs. When you pick up a batch:

1. Walk the call graph one level up for each row to determine thread origin and hot/cold classification.
2. Fill in the disposition, thread-origin, hot/cold, and strategy columns with your finding.
3. Apply the fix per the chosen strategy.
4. Tick the row.
6. Open a PR against `lucie/app-4154-prep` (stacked). Keep the diff under ~200 lines; split by file if needed.

The checklist files are edited in-place by each batch PR (each PR only touches its own rows, avoiding conflicts). The orchestrator runs `./script/presubmit` once all batches have merged, and re-runs the greps at the top of each checklist to prove completeness.

## Validation

- Xcode Instruments Leaks template: rerun the breadcrumb-hammer repro from #560 plus a short session exercising touched UI paths (window open/close, menu open, clipboard, appearance change, file picker). No new `Warp`-owned frames should appear.
- `./script/presubmit` before the final merge.

## References

- PR #560 / APP-4104: `specs/APP-4104/TECH.md`.
- `make_nsstring` helper: `crates/warpui/src/platform/mac/mod.rs:34`.
