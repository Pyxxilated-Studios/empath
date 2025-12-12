# empath-wasm-guest

Helper crate for building Empath WASM validation modules. It packages the canonical WIT interface, a `bindings!` proc macro, and ergonomic helpers for interacting with the host.

## Quick start

```rust
#![no_main]

empath_wasm_guest::bindings!();

use empath_wasm_guest::{accept, host, ContextExt, ValidationGuest, ValidationResult};

struct Validator;
export!(Validator);

impl ValidationGuest for Validator {
    fn validate_mail_from(ctx: Context) -> ValidationResult {
        host::log(format!("MAIL FROM from {:?}", ctx.sender()));

        if ctx.sender() == Some("bad@example.com") {
            return empath_wasm_guest::reject(550, "sender blocked");
        }

        accept()
    }
}
```

Add the dependencies:

```toml
[dependencies]
empath-wasm-guest = { version = "0", path = "../empath-wasm-guest" }
wit-bindgen = "0.49.0"
```

### What `bindings!` gives you

- Generates the correct component bindings for the `empath:validator` world (no local WIT copy required).
- Implements `ContextExt` so you can call `ctx.sender()`, `ctx.data_utf8()`, `ctx.transaction_value(..)`, etc.
- Adds `accept()`, `reject()`, `reject_temporary()`, and `reject_permanent()` helpers so you never have to build `ValidationResult` manually.
- Exposes host helpers under `host::*` (log, session/transaction context) and message helpers under `message::*` with typed error handling.

## Migration notes (from raw `wit-bindgen`)

1. Add the dependency: `empath-wasm-guest = { path = "../../empath-wasm-guest" }` (or use the published crate once available).
2. Replace `wit_bindgen::generate!` with `empath_wasm_guest::bindings!();` and drop the local WIT path.
3. Swap `empath::validator::host::*` calls for the shorter `host::*` wrappers.
4. Replace manual `ValidationResult::Reject(Rejection { code, message })` with `reject(code, message)` (or `reject_temporary` / `reject_permanent`), and `ValidationResult::Accept` with `accept()`.
5. Prefer the `ContextExt` helpers instead of reaching into struct fields directly.

Run `cargo test -p empath-wasm-guest` to ensure the embedded WIT stays in sync with the host copy.
