---
"wry": minor
---

Add experimental macOS native representations for browser-initiated drags behind
the opt-in `macos-private-drag` feature. Requires WebKit's private drag-source
customization API introduced in macOS 27; unsupported runtimes fail at webview
construction. Preserve the browser's original HTML payload for drops back into
the source webview while allowing native destinations to consume file promises.
Extend the unified drag example with a macOS implementation and deferred writes.
