// This crate exists solely to trigger build.rs during cargo build.
// The build.rs script invokes xtask to compile example modules (C and WASM).
// This enables `cargo build` to automatically build all example modules
// without requiring separate manual commands.
