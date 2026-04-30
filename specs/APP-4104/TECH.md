# APP-4104: Tech Spec

## Problem


## Relevant code

- `crates/warpui/src/platform/mac/mod.rs (29-34)` — `make_nsstring`, the existing helper that returns an autoreleased `NSString`
- `app/src/app_services/mac.rs (20-29)` — another app-side example of returning an autoreleased `NSString`

## Current state

### Rust bridge



- `unsafe fn to_nsstring(val: &str) -> id`
- implemented with `NSString::alloc(nil).init_str(val)`

That helper returns a retained Objective-C object. The crash-reporting bridge does not currently release or autorelease those values before returning to the caller.

### Objective-C bridge



### Existing repo pattern

Warp already has established patterns for temporary Cocoa strings:

- `warpui::platform::mac::make_nsstring` returns an autoreleased `NSString`
- app-owned macOS bridges already use direct `.autorelease()` in places like `app_services/mac.rs`

The crash-reporting bridge is the outlier.

## Proposed changes

### 1. Replace retained bridge strings with autoreleased strings


Concretely:

- import `warpui::platform::mac::make_nsstring` at the top of the file
- use that helper for every bridge string created in this module

Using one helper for the whole file keeps the ownership model uniform at the Rust→ObjC boundary and fixes the hot breadcrumb path without leaving the same bug in the lower-volume tag and user paths.

### 2. Add a local autorelease boundary in `forward_breadcrumb`


This bounds the lifetime of the Rust-created bridge strings because they are now autoreleased while the per-breadcrumb pool is current, rather than relying on an unknown outer pool on whatever thread emitted the log record.

### 3. Keep native breadcrumb ownership explicit


### 4. Preserve breadcrumb semantics

This fix is ownership-only. It does not change:

- which Rust log records become breadcrumbs
- how `before_breadcrumb` is registered

## End-to-end flow

2. `before_breadcrumb` forwards that breadcrumb to `mac::forward_breadcrumb`.
3. `forward_breadcrumb` creates a short-lived `NSAutoreleasePool`, then creates autoreleased `NSString` values for `message`, `category`, and `level`, and calls `recordBreadcrumb`.

## Risks and mitigations

1. **Autoreleased arguments must be retained by the callee**

2. **Per-breadcrumb autorelease-pool overhead**
   Adding a small pool around each breadcrumb has a fixed cost. That cost is negligible compared with the existing cross-language allocation work, and bounding memory is the higher-priority outcome for this path.

3. **Other macOS FFI bridges may still have separate ownership issues**
   APP-4104 is scoped to crash reporting because that is the hot path implicated by the memory profile. A wider audit can be handled separately once this fix lands.

## Testing and validation

2. Run a breadcrumb-heavy macOS session and verify that RSS no longer grows linearly while breadcrumb forwarding remains active.

## Follow-ups

- If APP-4104 resolves the memory growth, do a focused audit of other app-owned macOS FFI helpers that still call `NSString::alloc(nil).init_str(...)` directly so we can standardize on one safe bridge helper.
