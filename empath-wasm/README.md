# empath-wasm

WebAssembly (WASM) module support for Empath MTA, enabling validation modules written in any language that compiles to WASM.

## Features

- **Multi-language support**: Write modules in Rust, C, Go, AssemblyScript, or any WASM-compatible language
- **Near-native performance**: ~1-2ms overhead compared to native modules
- **True sandboxing**: WASM modules cannot access host memory or system resources
- **Resource limits**: Configurable memory caps (default: 10 MB) and execution timeouts (default: 5 seconds)
- **Epoch-based timeouts**: Precise timeout enforcement without fuel overhead
- **Event-driven**: Receive events for SMTP validation and lifecycle hooks

## Quick Start

### 1. Write a WASM Module

```rust
#![no_std]

#[no_mangle]
pub extern "C" fn init() -> i32 {
    0 // Success
}

#[no_mangle]
pub extern "C" fn validate_connect() -> i32 {
    // Return 0 to accept, non-zero to reject
    0
}
```

### 2. Compile to WASM

```bash
rustc --target wasm32-unknown-unknown --crate-type=cdylib -O \
  your_module.rs -o your_module.wasm
```

### 3. Configure in Empath

Add to your `empath.config.ron`:

```ron
Empath(
    modules: [
        {
            type: "Wasm",
            path: "/path/to/your_module.wasm",
            timeout_secs: 5,           // Optional, default: 5
            max_memory_bytes: 10485760, // Optional, default: 10 MB
            enable_wasi: false,         // Optional, default: false
        },
    ],
    // ... rest of config
)
```

## Available Callbacks

### Validation Callbacks (return 0 to accept, non-zero to reject)

- `validate_connect()` - Validate SMTP connection
- `validate_ehlo()` - Validate EHLO/HELO command
- `validate_mail_from()` - Validate MAIL FROM command
- `validate_rcpt_to()` - Validate RCPT TO command
- `validate_data()` - Validate message content
- `validate_starttls()` - Validate STARTTLS command

### Event Callbacks (return value ignored)

- `on_connection_opened()` - Connection established
- `on_connection_closed()` - Connection closed
- `on_delivery_attempt()` - Delivery attempt started
- `on_delivery_success()` - Delivery succeeded
- `on_delivery_failure()` - Delivery failed
- `on_smtp_error()` - SMTP error occurred
- `on_smtp_message_received()` - Message received
- `on_dns_lookup()` - DNS lookup completed

All callbacks are optional. Missing callbacks are treated as "allow".

## Host Functions

WASM modules can call these host functions:

```c
// Memory management
u32 em_alloc(u32 size);           // Allocate memory
void em_free(u32 ptr);            // Free memory (no-op)

// Context access
u32 em_get_context_size();        // Get context size
u32 em_get_context(u32 ptr, u32 max_len); // Read context

// Logging
void em_log(u32 ptr, u32 len);    // Log message
```

## Context Serialization

The `Context` is serialized using bincode and passed to WASM modules. Fields include:

- `id`: Session ID
- `sender`: Envelope sender (MAIL FROM)
- `recipients`: Envelope recipients (RCPT TO)
- `data`: Message content (DATA command)
- `extended`: ESMTP mode flag
- `max_message_size`: Size limit in bytes
- `transaction`: Key-value metadata
- `tracking_id`: Spool tracking ID

## Resource Limits

### Memory Limits

Default: 10 MB. Configurable via `max_memory_bytes`:

```ron
{
    type: "Wasm",
    path: "module.wasm",
    max_memory_bytes: 52428800, // 50 MB
}
```

Memory growth beyond the limit returns an error, stopping execution.

### Execution Timeouts

Default: 5 seconds. Configurable via `timeout_secs`:

```ron
{
    type: "Wasm",
    path: "module.wasm",
    timeout_secs: 10,
}
```

Uses Wasmtime's epoch-based interruption for precise timeouts without fuel overhead.

## Error Handling

- **Module not found**: Logged as error, initialization fails
- **Callback timeout**: Logged as error, returns 0 (allow)
- **Callback panic**: Logged as error, returns 0 (allow)
- **Memory limit exceeded**: Callback fails, returns 0 (allow)

All errors use a **fail-open** policy to prevent service disruption.

## Performance

- **Module loading**: ~10-50ms (one-time at startup)
- **Callback execution**: ~1-2ms overhead vs native
- **Memory overhead**: Minimal (~100 KB base + module size)
- **Isolation**: Full sandboxing with zero host memory access

## Comparison with Python Modules

| Feature | WASM | Python |
|---------|------|--------|
| Performance | ~1-2ms | ~5-10ms |
| Sandboxing | Full (no host access) | Partial (restricted imports) |
| Memory limits | Enforced | Best effort |
| Languages | Any WASM-compilable | Python only |
| Startup time | Faster | Slower |
| Hot reload | Not supported | Not supported |

## Examples

See `examples/modules/wasm/simple_validator.rs` for a complete example.

## Architecture

- **`loader.rs`**: WASM module loading, Wasmtime engine management
- **`host.rs`**: Host import functions, Context serialization
- **`error.rs`**: Error types for WASM operations

Integration: `empath-ffi/src/modules/wasm.rs` dispatches events to WASM callbacks.

## Limitations

- WASI support is experimental (disabled by default)
- No hot reload (restart required to update modules)
- Single-threaded execution (callbacks run sequentially)
- No async/await support in callbacks

## Future Enhancements

- Component Model support (interface types)
- WASI filesystem/network access
- Hot reload via module versioning
- Async callback execution
- Shared memory for bulk data transfer
