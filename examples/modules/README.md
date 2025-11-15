# Module Examples

Empath can be extended with custom plugins via the FFI (Foreign Function Interface) module system. Modules can be written in any language that can export C-compatible functions.

## Quick Start

```bash
# 1. Build Empath library
just build

# 2. Build example module
cd empath-ffi/examples
gcc example.c -fpic -shared -o libexample.so -I../../target -L../../target/debug -lempath

# 3. Configure Empath to load it
# Edit empath.config.ron:
# modules: [
#     (type: "SharedLibrary", name: "./empath-ffi/examples/libexample.so"),
# ]

# 4. Run and test
just run
```

---

## Available Examples

The reference module examples are located in `empath-ffi/examples/`:

| Module | Purpose | Language | Lines |
|--------|---------|----------|-------|
| `example.c` | Validation listener demo | C | ~100 |
| `event.c` | Event listener demo | C | ~50 |

### example.c - Validation Listener

**Purpose:** Demonstrates how to validate SMTP transactions and reject unwanted mail.

**Features:**
- Validates `MailFrom` event
- Rejects senders from blacklisted domains
- Accepts all other transactions

**Code snippet:**
```c
int on_mail_from(Context* ctx) {
    String sender = em_context_get_sender(ctx);

    // Reject mail from spam.com
    if (strstr(sender.data, "@spam.com")) {
        em_free_string(sender);
        return 1;  // Reject
    }

    em_free_string(sender);
    return 0;  // Accept
}
```

**Build:**
```bash
cd empath-ffi/examples
gcc example.c -fpic -shared -o libexample.so -I../../target -L../../target/debug -lempath
```

### event.c - Event Listener

**Purpose:** Demonstrates lifecycle event notifications.

**Features:**
- Logs connection opened events
- Logs connection closed events
- No transaction validation (observability only)

**Code snippet:**
```c
void on_connection_opened(Context* ctx) {
    String peer = em_context_get_peer(ctx);
    printf("Connection opened from: %.*s\n", (int)peer.len, peer.data);
    em_free_string(peer);
}
```

**Build:**
```bash
cd empath-ffi/examples
gcc event.c -fpic -shared -o libevent.so -I../../target -L../../target/debug -lempath
```

---

## Writing Your Own Module

### Step 1: Create Module File

Create `my_module.c`:

```c
#include <stdio.h>
#include <string.h>
#include "empath.h"

// Validation function - returns 0 to accept, non-zero to reject
int on_rcpt_to(Context* ctx) {
    StringVector recipients = em_context_get_recipients(ctx);

    // Example: Limit recipients per message
    if (recipients.len > 10) {
        printf("Too many recipients: %zu (max 10)\n", recipients.len);
        em_free_string_vector(recipients);
        return 1;  // Reject
    }

    em_free_string_vector(recipients);
    return 0;  // Accept
}

// Declare module
EM_DECLARE_MODULE("my_module", "1.0",
    NULL,           // on_connect
    NULL,           // on_mail_from
    on_rcpt_to,     // on_rcpt_to
    NULL,           // on_data
    NULL,           // on_start_tls
    NULL,           // on_connection_opened
    NULL            // on_connection_closed
);
```

### Step 2: Build Module

```bash
# Build Empath first
just build

# Build your module
gcc my_module.c -fpic -shared -o libmy_module.so \
    -I../../target \
    -L../../target/debug \
    -lempath
```

### Step 3: Configure Empath

Edit `empath.config.ron`:

```ron
modules: [
    (
        type: "SharedLibrary",
        name: "./path/to/libmy_module.so",
        arguments: [],
    ),
],
```

### Step 4: Test

```bash
# Start Empath
just run

# Send test email with 11 recipients (should be rejected)
./examples/smtp-client/send_bulk.sh
```

---

## Module API Reference

### Context API

```c
// Get session information
String em_context_get_id(Context* ctx);
String em_context_get_peer(Context* ctx);

// Get transaction data
String em_context_get_sender(Context* ctx);
StringVector em_context_get_recipients(Context* ctx);
String em_context_get_data(Context* ctx);

// Get/set metadata (persistent across events)
String em_context_get_metadata(Context* ctx, const char* key);
void em_context_set_metadata(Context* ctx, const char* key, const char* value);

// Memory management (IMPORTANT: Always free!)
void em_free_string(String s);
void em_free_string_vector(StringVector sv);
```

### Validation Events

Return `0` to accept, non-zero to reject:

| Event | When Called | Use Case |
|-------|-------------|----------|
| `on_connect` | Client connects | IP blacklist, rate limiting |
| `on_mail_from` | MAIL FROM received | Sender validation, SPF |
| `on_rcpt_to` | RCPT TO received | Recipient validation, recipient limit |
| `on_data` | After DATA (final `.`) | Content filtering, virus scanning |
| `on_start_tls` | STARTTLS requested | Force TLS policy |

### Lifecycle Events

No return value (notification only):

| Event | When Called | Use Case |
|-------|-------------|----------|
| `on_connection_opened` | Session starts | Logging, metrics |
| `on_connection_closed` | Session ends | Cleanup, metrics |

---

## Best Practices

### 1. Always Free Memory

```c
String sender = em_context_get_sender(ctx);
// ... use sender ...
em_free_string(sender);  // ALWAYS FREE!
```

Leaked memory will accumulate with each transaction.

### 2. Check for NULL

```c
String data = em_context_get_data(ctx);
if (data.data == NULL || data.len == 0) {
    // No data yet (not in DATA state)
    em_free_string(data);
    return 0;
}
```

### 3. Use Metadata for State

```c
// In on_mail_from
em_context_set_metadata(ctx, "sender_domain", "example.com");

// In on_data
String domain = em_context_get_metadata(ctx, "sender_domain");
// ... use domain ...
em_free_string(domain);
```

Metadata persists across the entire transaction.

### 4. Log Everything

```c
printf("[my_module] Rejecting sender: %.*s\n", (int)sender.len, sender.data);
```

Helps with debugging and auditing.

### 5. Fast Validation

Modules run synchronously - keep validation fast:
- ✅ Local blacklist lookups
- ✅ Simple pattern matching
- ⚠️ External API calls (add timeouts!)
- ❌ Long database queries

---

## Advanced Examples

### Spam Filter with Keyword Blocking

```c
int on_data(Context* ctx) {
    String data = em_context_get_data(ctx);

    const char* spam_keywords[] = {
        "viagra", "cialis", "lottery", "prince"
    };

    for (int i = 0; i < 4; i++) {
        if (strstr(data.data, spam_keywords[i])) {
            printf("Spam detected: keyword '%s'\n", spam_keywords[i]);
            em_free_string(data);
            return 1;  // Reject
        }
    }

    em_free_string(data);
    return 0;  // Accept
}
```

### Rate Limiter (per IP)

```c
#include <time.h>
#include <stdlib.h>

// Global state (use proper thread-safe data structure in production)
typedef struct {
    char ip[64];
    int count;
    time_t reset_time;
} RateLimit;

RateLimit limits[1000];
int limit_count = 0;

int on_connect(Context* ctx) {
    String peer = em_context_get_peer(ctx);
    time_t now = time(NULL);

    // Find or create rate limit entry
    for (int i = 0; i < limit_count; i++) {
        if (strcmp(limits[i].ip, peer.data) == 0) {
            // Reset counter every minute
            if (now > limits[i].reset_time) {
                limits[i].count = 0;
                limits[i].reset_time = now + 60;
            }

            // Check limit (max 10 connections per minute)
            if (limits[i].count >= 10) {
                printf("Rate limit exceeded for %s\n", peer.data);
                em_free_string(peer);
                return 1;  // Reject
            }

            limits[i].count++;
            em_free_string(peer);
            return 0;  // Accept
        }
    }

    // New IP
    if (limit_count < 1000) {
        strncpy(limits[limit_count].ip, peer.data, 63);
        limits[limit_count].count = 1;
        limits[limit_count].reset_time = now + 60;
        limit_count++;
    }

    em_free_string(peer);
    return 0;  // Accept
}
```

---

## Debugging Modules

### Enable Verbose Logging

```bash
RUST_LOG=debug just run
```

### Check Module Loading

Look for log output:
```
INFO  empath::modules] Loading module: ./libmy_module.so
INFO  empath::modules] Module loaded: my_module v1.0
```

### Common Issues

**Problem:** `error while loading shared libraries: libempath.so`

**Solution:**
```bash
export LD_LIBRARY_PATH=./target/debug:$LD_LIBRARY_PATH
```

**Problem:** Module loaded but callbacks not called

**Solution:** Check that you're using `EM_DECLARE_MODULE` macro correctly.

**Problem:** Segmentation fault

**Solution:** You're probably not freeing strings. Check all `em_free_string()` calls.

---

## Further Reading

- [CLAUDE.md - Module System](../../CLAUDE.md#moduleplugin-system) - Detailed architecture
- [empath.h](../../target/empath.h) - Complete C API reference (generated during build)
- [docs/ARCHITECTURE.md](../../docs/ARCHITECTURE.md#moduleplugin-system) - Module system diagram
- [empath-ffi/src/lib.rs](../../empath-ffi/src/lib.rs) - FFI implementation

---

## Contributing Modules

Have a useful module? Share it with the community!

1. Clean up the code
2. Add documentation
3. Add tests
4. Submit a pull request

See [CONTRIBUTING.md](../../CONTRIBUTING.md) for guidelines.
