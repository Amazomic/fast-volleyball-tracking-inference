# volley-web — actual Rust/WebAssembly client state

This is the first browser-executed **Rust/WASM** module in the migration. It owns the ordered, deduplicated, bounded (80 ID) manual selection of immutable 24-character provisional rally candidate IDs. It exposes a fixed 24-byte memory buffer and integer-only functions; no wasm-bindgen or JavaScript framework dependency is required. All security-sensitive project ownership, Touch evidence validation and original-video trims remain on the Axum server. The remaining DOM and HTTP bridge in `volley-app/static/studio.js` is transitional and still needs migration.

Build with a locally installed `wasm32-unknown-unknown` rustup target:

```sh
cd rust
cargo build -p volley-web --release --target wasm32-unknown-unknown --offline
```

The pinned deployed binary is `rust/volley-app/static/volley_web.wasm`; rebuilding it requires an explicit update and Node WebAssembly test before committing. A native test suite covers selection order, duplicate handling, removal, strict ID format and the 80-ID limit. No model inference or Python service is required by the browser WASM.
