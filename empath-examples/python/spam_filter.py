"""
Advanced Spam Filter Module for Empath

This module demonstrates more sophisticated validation:
- Sender reputation tracking
- Content-based scoring
- Rate limiting awareness
- Greylisting simulation
"""

import hashlib
import time

# In-memory storage (in production, use Redis or database)
sender_reputation = {}
greylist = {}

def init():
    """Initialize the spam filter module"""
    print("Spam filter module initialized")
    return True

def calculate_sender_reputation(sender):
    """
    Calculate sender reputation score (0-100)

    Args:
        sender: Email address

    Returns:
        int: Reputation score
    """
    if not sender or sender == "<>":
        return 50  # Neutral for null sender

    # Check our in-memory reputation database
    if sender in sender_reputation:
        return sender_reputation[sender]

    # New sender - start with neutral reputation
    sender_reputation[sender] = 50
    return 50

def update_reputation(sender, change):
    """Update sender reputation"""
    if not sender or sender == "<>":
        return

    current = sender_reputation.get(sender, 50)
    new_score = max(0, min(100, current + change))
    sender_reputation[sender] = new_score

def check_greylist(sender, recipient):
    """
    Simulate greylisting - temporarily reject first-time sender/recipient pairs

    Args:
        sender: Sender email
        recipient: Recipient email

    Returns:
        tuple: (should_greylist, is_first_attempt)
    """
    if not sender or not recipient:
        return (False, False)

    # Create unique key for sender/recipient pair
    key = hashlib.md5(f"{sender}:{recipient}".encode()).hexdigest()

    if key not in greylist:
        # First attempt - add to greylist
        greylist[key] = time.time()
        return (True, True)

    # Check if enough time has passed (5 minutes for this example)
    first_attempt_time = greylist[key]
    elapsed = time.time() - first_attempt_time

    if elapsed < 300:  # 5 minutes
        return (True, False)

    # Passed greylisting period
    return (False, False)

def validate_mail_from(ctx):
    """
    Validate sender with reputation checking

    Args:
        ctx: Context object

    Returns:
        bool: True to accept, False to reject
    """
    sender = ctx.sender

    # Calculate reputation
    reputation = calculate_sender_reputation(sender)

    # Block low reputation senders
    if reputation < 20:
        ctx.reject(550, f"Sender reputation too low ({reputation}/100)")
        ctx.transaction_set("spam_score", "10.0")
        return False

    # Warn on medium reputation
    if reputation < 50:
        ctx.transaction_set("spam_score", "5.0")
        ctx.transaction_set("reputation_warning", "true")

    ctx.transaction_set("sender_reputation", str(reputation))
    print(f"Sender {sender} has reputation: {reputation}/100")

    return True

def validate_rcpt_to(ctx):
    """
    Validate recipient with greylisting

    Args:
        ctx: Context object

    Returns:
        bool: True to accept, False to reject
    """
    sender = ctx.sender or "<>"
    recipients = ctx.recipients

    if not recipients:
        return True

    # Check greylisting for first recipient
    first_recipient = recipients[0]
    should_greylist, is_first = check_greylist(sender, first_recipient)

    if should_greylist:
        if is_first:
            ctx.reject(451, "Greylisting: Please try again in a few minutes")
            print(f"Greylisting first attempt: {sender} -> {first_recipient}")
        else:
            ctx.reject(451, "Greylisting: Retry too soon")
            print(f"Greylisting early retry: {sender} -> {first_recipient}")
        return False

    print(f"Passed greylisting: {sender} -> {first_recipient}")
    return True

def validate_data(ctx):
    """
    Content-based spam scoring

    Args:
        ctx: Context object

    Returns:
        bool: True to accept, False to reject
    """
    data_str = ctx.data_str

    if not data_str:
        # Binary data or encoding issues - allow but flag
        ctx.transaction_set("content_warning", "non_text")
        return True

    # Calculate spam score based on content
    spam_score = 0.0

    # Check for spam indicators
    spam_patterns = {
        "urgent": 1.0,
        "click here": 2.0,
        "act now": 2.0,
        "limited time": 1.5,
        "free money": 3.0,
        "nigerian prince": 5.0,
        "pharmacy": 2.0,
        "enlarge": 2.5,
        "$$$": 1.5,
    }

    data_lower = data_str.lower()
    for pattern, score in spam_patterns.items():
        if pattern in data_lower:
            spam_score += score
            print(f"Found spam pattern '{pattern}': +{score}")

    # Check for suspicious patterns
    if data_lower.count("http://") + data_lower.count("https://") > 10:
        spam_score += 2.0
        print("Too many URLs: +2.0")

    if data_str.count("!") > 20:
        spam_score += 1.0
        print("Excessive exclamation marks: +1.0")

    # Get existing score from sender validation
    existing_score = float(ctx.transaction_get("spam_score") or "0.0")
    total_score = existing_score + spam_score

    ctx.transaction_set("spam_score", f"{total_score:.1f}")

    # Reject high spam scores
    if total_score > 10.0:
        ctx.reject(550, f"Message rejected: spam score {total_score:.1f}/10")
        sender = ctx.sender or "<>"
        update_reputation(sender, -10)  # Penalize reputation
        print(f"Rejected spam (score {total_score:.1f})")
        return False

    # Flag medium scores
    if total_score > 5.0:
        ctx.transaction_set("spam_flag", "medium")
        print(f"Flagged as potential spam (score {total_score:.1f})")

    print(f"Message accepted (spam score {total_score:.1f})")
    return True

def on_smtp_message_received(ctx):
    """
    Update reputation on successful delivery

    Args:
        ctx: Context object

    Returns:
        bool: Always True
    """
    sender = ctx.sender
    spam_score = float(ctx.transaction_get("spam_score") or "0.0")

    # Improve reputation for clean messages
    if spam_score < 2.0:
        update_reputation(sender, +1)
    elif spam_score < 5.0:
        update_reputation(sender, +0.5)

    return True
