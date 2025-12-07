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
