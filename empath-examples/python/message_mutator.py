"""Python module that demonstrates message mutation helpers."""

from datetime import datetime


def _mutate_message(ctx):
    """Apply header and body mutations via the new Context helpers."""
    ctx.headers_set_if_absent("X-Processed-By", "python-message-mutator")
    ctx.headers_set("X-Mutation-Timestamp", datetime.utcnow().isoformat())
    ctx.headers_append("X-Mutation-Tag", "mutated")

    if not ctx.headers_contains("Message-ID"):
        ctx.headers_ensure_message_id()

    ctx.body_append_text("\n\n-- Processed by python message mutator --", "utf-8")
    ctx.transaction_set("python_message_mutated", "true")


def validate_data(ctx):
    """Mutate the message payload before it is spooled."""
    _mutate_message(ctx)
    return True


def on_smtp_message_received(ctx):
    """Log the mutation for observability."""
    ctx.transaction_set("last_mutation", datetime.utcnow().isoformat())
    return True
