import os
import time
import smtplib
from email.message import EmailMessage

from locust import User, task, between, events


def env_int(name: str, default: int) -> int:
    value = os.getenv(name)
    if value is None or value == "":
        return default
    return int(value, 10)


def env_float(name: str, default: float) -> float:
    value = os.getenv(name)
    if value is None or value == "":
        return default
    return float(value)


SMTP_HOST = os.getenv("SMTP_HOST", "127.0.0.1")
SMTP_PORT = env_int("SMTP_PORT", 2525)
SMTP_FROM = os.getenv("SMTP_FROM", "loadtest@example.com")
SMTP_TO = os.getenv("SMTP_TO", "sink@example.com")
SMTP_HELO = os.getenv("SMTP_HELO", "")
SMTP_STARTTLS = os.getenv("SMTP_STARTTLS", "0") == "1"
SMTP_USERNAME = os.getenv("SMTP_USERNAME", "")
SMTP_PASSWORD = os.getenv("SMTP_PASSWORD", "")
SMTP_TIMEOUT_SEC = env_int("SMTP_TIMEOUT_SEC", 15)

MESSAGE_BODY_BYTES = env_int("MESSAGE_BODY_BYTES", 1024)

WAIT_MIN_SEC = env_float("WAIT_MIN_SEC", 0.5)
WAIT_MAX_SEC = env_float("WAIT_MAX_SEC", 1.5)


def build_message() -> EmailMessage:
    msg = EmailMessage()
    msg["From"] = SMTP_FROM
    msg["To"] = SMTP_TO
    msg["Subject"] = f"empath load test {time.time_ns()}"
    msg["X-Load-Test"] = "empath"
    body = "X" * max(0, MESSAGE_BODY_BYTES)
    msg.set_content(body)
    return msg


class SmtpUser(User):
    wait_time = between(WAIT_MIN_SEC, WAIT_MAX_SEC)

    @task
    def send_message(self) -> None:
        start = time.perf_counter()
        message = build_message()

        try:
            with smtplib.SMTP(SMTP_HOST, SMTP_PORT, timeout=SMTP_TIMEOUT_SEC) as client:
                if SMTP_HELO:
                    client.ehlo(SMTP_HELO)
                else:
                    client.ehlo()
                if SMTP_STARTTLS:
                    client.starttls()
                    client.ehlo()
                if SMTP_USERNAME:
                    client.login(SMTP_USERNAME, SMTP_PASSWORD)
                client.send_message(message)
            elapsed_ms = (time.perf_counter() - start) * 1000.0
            events.request.fire(
                request_type="smtp",
                name="send_message",
                response_time=elapsed_ms,
                response_length=len(message.as_bytes()),
                exception=None,
            )
        except Exception as exc:
            elapsed_ms = (time.perf_counter() - start) * 1000.0
            events.request.fire(
                request_type="smtp",
                name="send_message",
                response_time=elapsed_ms,
                response_length=0,
                exception=exc,
            )
            raise
