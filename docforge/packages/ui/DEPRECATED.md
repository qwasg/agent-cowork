# ⚠️ DEPRECATED — Old Tiptap-based DocForge UI

This Tiptap/React UI package has been **superseded by the native Rust (GPUI) DocForge app**.

- New app: `rust/apps/docforge` (crate `moonlit-docforge`)
- Run: `cargo run -p moonlit-docforge` from `rust/`
- Packaged: `rust/packaging/build-windows.ps1` → `rust/dist/moonlit-*-windows-x64.zip`

The native app provides the Word rich-text editor and the PPT native canvas on a
real `yrs` CRDT core (`moonlit-doccore`), with live L1 preview, real-time
collaboration via the embedded sync server, MS Office-compatible `.docx`/`.pptx`
export (`moonlit-compile`), and true PNG L2 rasterization (`moonlit-preview`).

This package is retained for reference only and is no longer maintained. Do not
add new features here. It will be removed in a future cleanup.
