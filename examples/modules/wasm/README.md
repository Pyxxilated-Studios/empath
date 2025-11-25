# Simple WASM Validator Example

This is an example SMTP validator module implemented as a WebAssembly Component using the WASIp2 Component Model.

## Features

This validator demonstrates:

- **Connection tracking**: Uses session context to track validated connections
- **Sender validation**: Blocks emails from `spam.example.com`
- **Recipient limiting**: Rejects messages with more than 10 recipients
- **Message size validation**: Rejects messages larger than 1MB
- **Simple spam filtering**: Checks for spam keywords in message content
- **Transaction context**: Stores metadata that persists to spool for delivery modules
- **Host function usage**: Calls back to Empath for logging and context management

## Building

### Prerequisites

- Rust nightly toolchain
- `wasm32-wasip2` target installed:
  ```bash
  rustup target add wasm32-wasip2
  ```

### Compile

```bash
cargo build --target wasm32-wasip2 --release
```

The compiled component will be at:
```
target/wasm32-wasip2/release/simple_validator.wasm
```

A copy is also placed in the project root for convenience:
```
simple_validator.wasm (109KB)
```

## Using in Empath

Add to your `empath.config.ron`:

```ron
Empath (
    smtp_controller: (
        listeners: [{ /* ... */ }],
        modules: [
            (
                type: "Wasm",
                path: "./examples/modules/wasm/simple_validator.wasm",
                timeout_secs: 5,
                max_memory_bytes: 10485760,  // 10MB
            ),
        ],
    ),
    // ...
)
```

## Architecture

### WIT Interface

The module implements the `empath:validator` world defined in `wit/empath.wit`:

- **Exports** (validation interface):
  - `validate-connect`: Called when client connects
  - `validate-ehlo`: Called on EHLO/HELO command
  - `validate-mail-from`: Called on MAIL FROM command
  - `validate-rcpt-to`: Called on RCPT TO command
  - `validate-data`: Called on DATA command
  - `validate-starttls`: Called on STARTTLS command

- **Imports** (host functions):
  - `log`: Log messages from WASM to host
  - `session-set/get/exists`: Session context (not persisted)
  - `transaction-set/get/exists`: Transaction context (persisted to spool)

### Context Types

**Session Context** (ephemeral):
- Connection tracking
- Authentication state
- Per-connection state that's reset on RSET

**Transaction Context** (persisted):
- Spam scores
- Validation results
- Sender/recipient metadata
- Available to delivery modules

## Example Validations

### Blocking Spam Domains

```rust
if sender.ends_with("@spam.example.com") {
    empath::validator::host::transaction_set(
        "rejection_reason",
        "sender_blocked_spam_domain",
    );

    return ValidationResult::Reject(Rejection {
        code: 550,
        message: "Sender from spam.example.com is not allowed".to_string(),
    });
}
```

### Content Filtering

```rust
let spam_keywords = ["viagra", "pills", "enlarge"];
for keyword in &spam_keywords {
    if msg_str.to_lowercase().contains(keyword) {
        return ValidationResult::Reject(Rejection {
            code: 550,
            message: "Message contains spam content".to_string(),
        });
    }
}
```

### Recipient Limiting

```rust
if ctx.recipients.len() > 10 {
    return ValidationResult::Reject(Rejection {
        code: 452,
        message: "Too many recipients (max 10)".to_string(),
    });
}
```

## Development

The module is built using `wit-bindgen` to generate Rust bindings from the WIT interface definition. This provides:

- Type-safe API
- Automatic serialization/deserialization
- Component Model support
- WASIp2 capabilities

## Performance

- Module size: ~109KB (release build)
- Memory limit: 10MB (configurable)
- Execution timeout: 5 seconds (configurable)
- Near-native performance (~1-2ms overhead)

## Security

- **Sandboxed execution**: No direct host memory access
- **Resource limits**: Memory and CPU time capped
- **WASI isolation**: Filesystem access requires explicit preopens
- **Type safety**: All FFI calls are type-checked at compile time

## Further Reading

- [WIT specification](https://component-model.bytecodealliance.org/design/wit.html)
- [Component Model](https://component-model.bytecodealliance.org/)
- [wasmtime documentation](https://docs.wasmtime.dev/)
- [wit-bindgen](https://github.com/bytecodealliance/wit-bindgen)
