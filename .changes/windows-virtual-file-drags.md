---
"wry": minor
---

Add an experimental Windows composition-hosted virtual-file drag provider. Forward native drops to WebView2, preserve genuine HTML drag payloads and completion, and supply file content through delayed native streams. Requires WebView2 CompositionController5; windowed hosting is unchanged.

Update the Windows bindings for DragStarting support and raise the minimum Rust version to 1.82, as required by those dependencies.
