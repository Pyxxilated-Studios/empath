"""
Simple Empath Python Module Example

This module demonstrates basic validation logic:
- Blocks connections from specific domains
- Validates sender addresses
- Logs all events
"""


def init(*args):
    """Initialize the module (optional)"""
    print(f"Simple Python module initialized {args}")
    return True


def validate_connect(ctx):
    """
    Validate incoming SMTP connections

    Args:
        ctx: Context object with connection information

    Returns:
        bool: True to accept, False to reject
    """
    # Block connections if we had session context indicating blocked IPs
    # (in real usage, session context would be set during connection)
    if ctx.session_exists("blocked"):
        ctx.reject(550, "Connection blocked")
        return False

    print(f"Connection from client: {ctx.client_id}")
    return True


def validate_mail_from(ctx):
    """
    Validate MAIL FROM (sender) addresses

    Args:
        ctx: Context object with envelope information

    Returns:
        bool: True to accept, False to reject
    """
    sender = ctx.sender

    if not sender:
        # No sender (null sender for bounces) - allow
        return True

    # Block specific domains
    blocked_domains = ["spam.com", "scam.net", "blocked.example.com"]

    for domain in blocked_domains:
        if sender.endswith(f"@{domain}"):
            ctx.reject(550, f"Sender domain {domain} is blocked")
            return False

    # Set transaction context for use in later validation
    ctx.transaction_set("sender_domain", sender.split("@")[1] if "@" in sender else "")

    print(f"Accepted sender: {sender}")
    return True


def validate_rcpt_to(ctx):
    """
    Validate RCPT TO (recipient) addresses

    Args:
        ctx: Context object with envelope information

    Returns:
        bool: True to accept, False to reject
    """
    recipients = ctx.recipients

    if not recipients:
        return True

    # Example: limit recipients per message
    max_recipients = 100
    if len(recipients) > max_recipients:
        ctx.reject(552, f"Too many recipients (max {max_recipients})")
        return False

    print(f"Accepted {len(recipients)} recipient(s)")
    return True


def validate_data(ctx):
    """
    Validate message data

    Args:
        ctx: Context object with full message

    Returns:
        bool: True to accept, False to reject
    """
    # Get message data as bytes
    data = ctx.data

    if not data:
        ctx.reject(554, "No message data")
        return False

    # Simple size check
    max_size = 10 * 1024 * 1024  # 10 MB
    if len(data) > max_size:
        ctx.reject(552, "Message too large")
        return False

    # Try to get as string for content inspection
    data_str = ctx.data_str
    if data_str:
        # Simple spam keyword check
        spam_keywords = ["viagra", "cialis", "lottery winner"]
        for keyword in spam_keywords:
            if keyword.lower() in data_str.lower():
                ctx.reject(550, "Message contains spam content")
                ctx.transaction_set("spam_score", "10.0")
                return False

    print(f"Accepted message ({len(data)} bytes)")
    ctx.transaction_set("message_size", str(len(data)))
    return True


def on_connection_closed(ctx):
    """
    Event handler for connection close

    Args:
        ctx: Context object

    Returns:
        bool: Always return True for event handlers
    """
    print(f"Connection closed for client: {ctx.client_id}")
    return True


def on_smtp_message_received(ctx):
    """
    Event handler for when a complete message is received

    Args:
        ctx: Context object with full message

    Returns:
        bool: Always return True for event handlers
    """
    sender = ctx.sender or "<>"
    recipient_count = len(ctx.recipients) if ctx.recipients else 0
    message_size = ctx.transaction_get("message_size") or "unknown"

    print(
        f"Message received: from={sender}, recipients={recipient_count}, size={message_size}"
    )
    return True
