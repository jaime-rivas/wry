# Unified HTML and native virtual-file drag experiment

This example tests one repository-item drag that can finish on an HTML target
or in the native filesystem. It also exercises incoming files and directories.
It is an experimental API demonstration, not a cross-platform compatibility
guarantee. Upload persistence remains application work.

## Run

Windows, with a WebView2 runtime supporting CompositionController5:

```sh
cargo run --example unified_drag
```

macOS, with WebKit's private source-customization hook (introduced in macOS 27):

```sh
cargo run --example unified_drag --features macos-private-drag
```

Add `-- --remote` to either command to offer the same 62,577,581-byte release
asset rather than the small in-memory fixture. On macOS, `/usr/bin/curl` streams
the response directly into the destination file only when AppKit requests it.
The example disables curl configuration-file loading and restricts redirects
to HTTPS. Nothing is downloaded at drag start. Use a fresh destination folder.

The default fixture is `wry-virtual-file.txt`, containing exactly
`Deferred content for repository:item-1\n` (39 bytes, including the newline).
The remote asset's published GitHub digest, inspected on 2026-10-27, is
`c84a33966f42ef1881040fb9bac1d627bd76429289ee9fdef760508251684878` (SHA-256).

## Manual checks

1. Purple repository item to green HTML target: custom identity survives,
   genuine (`trusted: true`) DOM events, no native content requests.
2. Same source to Finder/Explorer: actual file bytes match the fixture, content
   requested only by the accepting destination, correct source completion.
3. Native file and directory to green: matching HTML entries and native paths.
   The example reads up to 32 bytes from files and inspects directory metadata;
   it does not recursively upload directories or persist access grants.
4. Internal copy/move/link modifiers, HTML-only baseline, rejected drop, Escape,
   and native hover followed by returning to HTML. These must not read content
   or make network requests for the repository item.

Windows modifiers are Ctrl/Shift/Ctrl+Shift; macOS uses Option/Command/
Option+Command. Keep logs for each session; do not infer behavior from the cursor
alone. An accepted HTML drop does not create or upload a file in this fixture.

## Platform boundaries

### Recorded macOS run (2026-10-27)

Tested with Finder on macOS 27.0 (26A428), WebKit 22625.1.29.11.27.
The final remote-mode run recorded these five consecutive native gestures:

| Session | Gesture | DOM completion | Content/network requests |
| --- | --- | --- | --- |
| 1 | Option+Command to HTML | link, trusted drop with repository identity | 0 / 0 |
| 2 | Rejected HTML target | none | 0 / 0 |
| 3 | Escape | none | 0 / 0 |
| 4 | Hover Finder, return to HTML | move, trusted drop with repository identity | 0 / 0 |
| 5 | Finder export | copy | 1 / 1 |

The exported file was 62,577,581 bytes; `shasum -a 256` matched the published
digest above. Earlier small-fixture runs confirmed internal copy, the HTML-only
baseline, a 39-byte Finder export, and incoming file/directory drops with native
read/metadata access. Copy and the baseline were tested before the final native
modifier-mask correction; the final run above covers link and unmodified move.

Scoped verification passed: `cargo build --example unified_drag --features
macos-private-drag`, `cargo check --example unified_drag` (default features),
and `cargo test --lib --features macos-private-drag
wkwebview::drag_source::tests` (2 passed). The default-feature check preceded
the final feature-gated modifier correction. Windows was not rerun on this Mac;
the parent branch contains the separately tested Windows implementation.

### Compatibility and lifetime

Windows preserves the browser's original IDataObject and native source loop.
The private `chromium/x-ignore-file-contents` marker prevents browser readback
of the added Shell stream. This is not a public WebView2 guarantee. Composition
hosting is required; stream reads are synchronous and can block the UI thread.

macOS supplies NSDraggingItems through WebKit's private UI-delegate callback.
WebKit owns the real source gesture and completion. Wry snapshots the browser's
original pasteboard verbatim and presents it only when the native dragging
source is the exact originating webview. It does not invent browser provenance,
synthetically dispatch DOM events, or rewrite AppKit's live drag pasteboard.
Other webviews and native applications see the supplied native representations.

For those internal drops, Wry applies native Option/Command state to the declared
local operation mask without widening it. This avoids AppKit's promised-file
copy preference overriding an internal link and does not depend on WebKit
populating the DOM drag event's modifier booleans. The example respects a
single-operation `effectAllowed` before choosing its default HTML operation.

The callback returns `MacosDragSource`, containing native items and the allowed
local operations. Match those operations to the HTML source's `effectAllowed`.
Return `None` for normal HTML/image/file-input drags. Only select application
custom-data sources; snapshotting file-backed pasteboard data may request bytes.
Native observers must return false to allow WebKit's actual drop decision.

The macOS example retains its weak promise delegate through the provider's
`userInfo`, without a cycle or a reference to the webview. Writes run on a
background NSOperationQueue, and errors reach AppKit's completion callback.
Each export creates its destination exclusively and removes only its own partial
file on failure. The OS owns native item lifetime; keeping the process alive
for outstanding exports remains application work.

Neither private dependency establishes support for all OS/runtime versions.
Do not enable the macOS feature by default or silently fall back on unsupported
runtimes. Cross-webview identity sharing, navigation during a drag, multi-file
export, concurrent transfers and failure recovery need separate qualification.

## Source attribution

The Windows composition implementation retains the attribution for
[Wry PR #1762](https://github.com/tauri-apps/wry/pull/1762). This branch builds on
Legend-Master's `windows-file-path-getter` prototype. The modern NSURL-based
macOS path collector is backported from upstream Wry's
[PR #1844](https://github.com/tauri-apps/wry/pull/1844); it avoids the legacy
pasteboard materialization panic on incoming files.

The macOS private callback is declared in WebKit's
[WKUIDelegatePrivate.h](https://github.com/WebKit/WebKit/blob/main/Source/WebKit/UIProcess/API/Cocoa/WKUIDelegatePrivate.h).
The standalone Swift probe demonstrated the architecture before this Rust port.
