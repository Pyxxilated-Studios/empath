# Empath Python Module Examples

This directory contains example Python modules that demonstrate how to extend Empath's functionality using Python.

## Examples

### 1. Simple Module (`simple.py`)

A straightforward validation module demonstrating:
- **Connection validation**: Basic connection filtering
- **Sender validation**: Block specific domains
- **Recipient validation**: Limit recipients per message
- **Content validation**: Size limits and keyword filtering
- **Event handling**: Log connection lifecycle events

**Use case**: Quick prototyping, learning the API, simple filtering rules

### 2. Spam Filter Module (`spam_filter.py`)

An advanced spam filtering module featuring:
- **Sender reputation tracking**: Score senders based on behavior
- **Greylisting**: Temporarily reject first-time sender/recipient pairs
- **Content scoring**: Pattern-based spam detection
- **Dynamic reputation**: Adjust scores based on message quality

**Use case**: Production-grade spam filtering, advanced validation logic

## Configuration

Add Python modules to your `empath.config.ron`:

```ron
Empath(
    // ... other config ...

    smtp_controller: (
        modules: [
            // Simple validation module
            (
                type: "Python",
                path: "examples/python_modules/simple.py",
            ),

            // Advanced spam filter
            (
                type: "Python",
                path: "examples/python_modules/spam_filter.py",
            ),
        ],
        // ... other smtp config ...
    ),
)
```

### 3. Message Mutation Module (`message_mutator.py`)

Demonstrates the mutation helpers added to `PyContext`:

* Ensures `Message-ID` exists and records mutation metadata
* Adds mutation-specific headers (`X-Processed-By`, `X-Mutation-Timestamp`, etc.)
* Appends a footer to the message body and tracks the mutation time in `transaction`
* Hooks both `validate_data` for synchronous mutation and `on_smtp_message_received`

Use case: audit or enrich the SMTP payload before spooling/delivery.

**Note**: The `timeout_secs` parameter is currently not enforced due to threading constraints (Python's signal module only works in the main thread, but callbacks run in worker threads). Keep your callbacks fast (< 1 second recommended) to avoid blocking SMTP sessions.

## Python Module API

### Validation Callbacks

Return `True` to accept, `False` to reject:

```python
def validate_connect(ctx) -> bool:
    """Validate incoming connection"""
    return True

def validate_ehlo(ctx) -> bool:
    """Validate EHLO/HELO command"""
    return True

def validate_mail_from(ctx) -> bool:
    """Validate MAIL FROM (sender)"""
    return True

def validate_rcpt_to(ctx) -> bool:
    """Validate RCPT TO (recipient)"""
    return True

def validate_data(ctx) -> bool:
    """Validate message content"""
    return True

def validate_starttls(ctx) -> bool:
    """Validate STARTTLS command"""
    return True
```

### Event Callbacks

Event handlers should always return `True`:

```python
def on_connection_opened(ctx) -> bool:
    """Connection established"""
    return True

def on_connection_closed(ctx) -> bool:
    """Connection closed"""
    return True

def on_delivery_attempt(ctx) -> bool:
    """Before delivery attempt"""
    return True

def on_delivery_success(ctx) -> bool:
    """Successful delivery"""
    return True

def on_delivery_failure(ctx) -> bool:
    """Failed delivery"""
    return True

def on_smtp_error(ctx) -> bool:
    """SMTP error occurred"""
    return True

def on_smtp_message_received(ctx) -> bool:
    """Complete message received"""
    return True

def on_dns_lookup(ctx) -> bool:
    """DNS lookup completed"""
    return True
```

### Context API

The `ctx` object provides access to message and session data:

#### Envelope Properties
```python
ctx.sender            # Sender address (str or None)
ctx.recipients        # List of recipient addresses
ctx.client_id         # Client identifier from HELO/EHLO
ctx.extended          # True if ESMTP extensions enabled
```

#### Message Data
```python
ctx.data              # Message data as bytes
ctx.data_str          # Message data as UTF-8 string (or None if invalid UTF-8)
```

#### Session Context (persists across RSET, NOT spooled)
```python
ctx.session_get("key")               # Get session value
ctx.session_set("key", "value")      # Set session value
ctx.session_exists("key")            # Check if key exists
```

**Use for**: TLS state, authentication info, connection properties

#### Transaction Context (cleared on RSET, IS spooled)
```python
ctx.transaction_get("key")           # Get transaction value
ctx.transaction_set("key", "value")  # Set transaction value
ctx.transaction_exists("key")        # Check if key exists
```

**Use for**: Spam scores, validation results, per-message tracking

#### TLS Information
```python
ctx.is_tls           # True if using TLS
ctx.tls_protocol     # TLS protocol version (e.g., "TLSv1.3")
ctx.tls_cipher       # TLS cipher suite
```

#### Rejection
```python
ctx.reject(550, "Spam detected")     # Reject with SMTP code and message
ctx.is_rejected                       # Check if rejected
ctx.rejection                         # Get (code, message) tuple if rejected
```

## Best Practices

### 1. Initialize Carefully

```python
def init(args):
    """
    Optional initialization function
    Called once when the module is loaded
    """
    # args is a list of strings from config "args"
    # Load configuration
    # Initialize connections
    # Set up state

    return True  # Return False to prevent module loading
```

### 2. Handle Missing Data

```python
def validate_mail_from(ctx):
    sender = ctx.sender

    if not sender:
        # Null sender (for bounces) - allow
        return True

    # Validate sender...
    return True
```

### 3. Use Context for State

```python
def validate_mail_from(ctx):
    # Set data for later validation stages
    ctx.transaction_set("sender_domain", extract_domain(ctx.sender))
    return True

def validate_data(ctx):
    # Retrieve data from earlier stage
    sender_domain = ctx.transaction_get("sender_domain")
    # ...
    return True
```

### 4. Fail Open on Errors

Wrap risky operations in try/except to prevent blocking legitimate mail:

```python
def validate_data(ctx):
    try:
        # Complex validation logic
        return check_spam(ctx.data)
    except Exception as e:
        print(f"Validation error: {e}")
        return True  # Allow on error
```

### 5. Log Appropriately

```python
def validate_mail_from(ctx):
    sender = ctx.sender
    print(f"Validating sender: {sender}")  # Logged via Empath's tracing

    if is_blocked(sender):
        print(f"Blocked sender: {sender}")
        ctx.reject(550, "Sender blocked")
        return False

    return True
```

### 6. Respect Timeouts

Keep validation logic fast. Default timeout is 5 seconds:
- Avoid blocking I/O
- Use async operations where possible
- Cache expensive lookups

## Testing Your Module

Test modules using the Python REPL before deploying:

```python
# Test your module's logic
from spam_filter import validate_mail_from, validate_data

class MockContext:
    def __init__(self):
        self.sender = "test@example.com"
        self.data_str = "Test message"
        self._transaction = {}

    def transaction_set(self, key, value):
        self._transaction[key] = value

    def transaction_get(self, key):
        return self._transaction.get(key)

    def reject(self, code, msg):
        print(f"REJECT {code}: {msg}")

ctx = MockContext()
result = validate_mail_from(ctx)
print(f"Result: {result}")
```

## Security Considerations

1. **Timeout Enforcement**: Callbacks are terminated after `timeout_secs`
2. **Exception Handling**: Python exceptions are caught and logged
3. **Fail Open**: Errors result in allowing the message (customize in your code)
4. **Resource Limits**: Python interpreter runs in-process (monitor memory usage)

## Performance Tips

1. **Use Sets for Lookups**: `if domain in blocked_set:` is O(1)
2. **Compile Regex Once**: Compile patterns at module level, not per-message
3. **Cache External Calls**: Store DNS/API results in module-level dict
4. **Batch Operations**: If calling external APIs, batch multiple messages

## Troubleshooting

### Module Not Loading

Check logs for initialization errors:
```bash
tail -f logs/empath.log | grep -i python
```

### Callback Not Called

Ensure function name matches exactly:
- `validate_connect` (not `validate_connection`)
- `on_connection_closed` (not `connection_closed`)

### Timeout Errors

Increase `timeout_secs` in configuration or optimize your code.

### Import Errors

Ensure required Python packages are installed:
```bash
pip install required-package
```

## Contributing

Have a useful Python module? Consider contributing it:
1. Add to `examples/modules/python/`
2. Document usage in this README
3. Add tests in `tests/modules/python/`
4. Submit a pull request

## License

These examples are licensed under the same terms as Empath (Apache-2.0).
