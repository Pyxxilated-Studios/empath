# Message Mutation Helpers

Empath now exposes the same message mutation helpers across all module types so you can modify headers and bodies before a message is spooled or delivered.

## Python module example

- Location: `empath-examples/python/message_mutator.py`
- Hooks `validate_data` to manipulate headers (`X-Processed-By`, `X-Mutation-Timestamp`, `X-Mutation-Tag`), ensure a `Message-ID`, and append a footer to the body.
- Stores evidence of the mutation (`transaction["python_message_mutated"]`, `transaction["last_mutation"]`) so downstream modules or delivery stages can detect the change.

### Configuration

```ron
modules: [
    (
        type: "Python",
        path: "empath-examples/python/message_mutator.py",
    ),
]
```

## Shared library example

- Location: `empath-examples/shared/message_mutation.c`
- Mirrors the Python flow using `em_message_headers_*` and `em_message_body_append_text`.
- Includes the helper `log.c` so you can see when the mutation runs.

### Build

```bash
cd empath-examples/shared
gcc message_mutation.c log.c -fpic -shared -o libmessage_mutation.so \
  -I../../target -L../../target/debug -lempath
```

## Why mutate messages?

Use these helpers when you need to:

* Inject audit metadata before spooling (spam scores, mutation timestamps).
* Normalize headers (Message-ID, Content-Type, transfer-encoding).
* Append or replace the body payload after analyzing it (PDF stamping, DLP tags).
* Track mutation fingerprints across Python, C, or WASM modules for observability.

Both the Python and shared library examples log their mutations and decorate the message so you can verify the new API surface end-to-end.

## WASM module example

```rust
use empath_wasm_guest::bindings::empath::validator::message;

fn validate_data(_ctx: empath_wasm_guest::bindings::empath::validator::ValidationContext) -> i32 {
    let _ = message::headers_set("X-Processed-By".into(), "wasm-mutator".into());
    let _ = message::body_set_alternative(
        "Plain text body".into(),
        "<p>HTML body</p>".into(),
        "utf-8".into(),
    );
    0
}
```

## FFI module example (C)

```c
int validate_data(Context *ctx) {
  em_message_headers_set(ctx, "X-Processed-By", "ffi-mutator");
  em_message_body_set_alternative(
      ctx,
      "Plain text body",
      "<p>HTML body</p>",
      "utf-8");
  return 0;
}
```

## Mutation guardrails

- **DKIM/ARC**: mutating signed headers or body bytes invalidates existing signatures. Re-sign downstream if needed.
- **Charset & encoding**: when setting text/HTML, keep `Content-Type` and `Content-Transfer-Encoding` aligned with your payload.
- **Newlines**: body setters normalize to CRLF by default; avoid mixing LF/CRLF in appended chunks.
- **Header folding**: encoded-word folds are collapsed into a single space during parsing; preserve exact encoded-word semantics when rewriting.
- **Multipart**: `body_set_alternative` builds a `multipart/alternative` body with an auto boundary; if you need a specific boundary, build the multipart body manually and set `Content-Type` yourself.
