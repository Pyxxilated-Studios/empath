# Empath MTA Architecture

This document provides a visual overview of Empath's architecture using diagrams. For detailed implementation details, see [CLAUDE.md](../CLAUDE.md).

## Table of Contents

- [High-Level Overview](#high-level-overview)
- [Component Architecture](#component-architecture)
- [Data Flow](#data-flow)
- [Workspace Structure](#workspace-structure)
- [Protocol System](#protocol-system)
- [Module/Plugin System](#moduleplugin-system)
- [SMTP State Machine](#smtp-state-machine)
- [Delivery Pipeline](#delivery-pipeline)
- [Control System](#control-system)

---

## High-Level Overview

Empath is a modular Mail Transfer Agent (MTA) built with Rust, designed for:
- **Easy debugging and testing**
- **Extensibility through plugins** (FFI-based module system)
- **Production-ready observability** (OpenTelemetry, Prometheus, Grafana)
- **Embeddability** (produces cdylib for each crate)

```mermaid
graph TB
    subgraph "External"
        Client[SMTP Client]
        RemoteMTA[Remote MTA]
        Admin[Administrator]
    end

    subgraph "Empath MTA"
        SMTP[SMTP Server<br/>Port 1025]
        Spool[Message Spool<br/>Filesystem]
        Queue[Delivery Queue<br/>In-Memory]
        Delivery[Delivery Processor]
        Control[Control Socket<br/>IPC]
        Modules[Plugin Modules<br/>FFI]
    end

    subgraph "Observability"
        OTEL[OTEL Collector]
        Prom[Prometheus]
        Graf[Grafana]
    end

    Client -->|Connect| SMTP
    SMTP -->|Validate| Modules
    SMTP -->|Store| Spool
    Spool -->|Scan| Queue
    Queue -->|Process| Delivery
    Delivery -->|Send| RemoteMTA
    Admin -->|Manage| Control
    Control -->|Query| Queue

    SMTP -->|Metrics| OTEL
    Delivery -->|Metrics| OTEL
    OTEL -->|Export| Prom
    Prom -->|Visualize| Graf
```

---

## Component Architecture

### 7-Crate Workspace

```mermaid
graph TD
    subgraph "Binary Crate"
        Main[empath<br/>Main orchestrator]
    end

    subgraph "Core Libraries"
        Common[empath-common<br/>Core traits & types]
        Tracing[empath-tracing<br/>Instrumentation macros]
    end

    subgraph "Protocol Layer"
        SMTP[empath-smtp<br/>SMTP implementation]
    end

    subgraph "Storage Layer"
        Spool[empath-spool<br/>Message persistence]
    end

    subgraph "Delivery Layer"
        Delivery[empath-delivery<br/>Outbound delivery]
    end

    subgraph "Extension Layer"
        FFI[empath-ffi<br/>C API & modules]
    end

    Main --> Common
    Main --> SMTP
    Main --> Spool
    Main --> Delivery
    Main --> FFI

    SMTP --> Common
    SMTP --> Tracing
    Spool --> Common
    Delivery --> Common
    Delivery --> Spool
    FFI --> Common
```

**Crate Responsibilities:**

- **empath**: Main binary, configuration, controller orchestration
- **empath-common**: Core abstractions (Protocol, FSM, Controller, Listener traits)
- **empath-smtp**: SMTP protocol implementation with finite state machine
- **empath-delivery**: Outbound mail delivery queue and processor
- **empath-spool**: Message persistence to filesystem with watching
- **empath-ffi**: C-compatible API for embedding and dynamic modules
- **empath-tracing**: `#[traced]` procedural macro for instrumentation

---

## Data Flow

### Message Reception and Delivery

```mermaid
sequenceDiagram
    participant Client
    participant Listener
    participant Session
    participant Modules
    participant Spool
    participant Queue
    participant Delivery
    participant RemoteMTA

    Client->>Listener: TCP Connect
    Listener->>Session: Create Session
    Session->>Modules: ConnectionOpened Event

    Client->>Session: EHLO example.com
    Session->>Client: 250 Features

    Client->>Session: MAIL FROM:<sender@example.com>
    Session->>Modules: MailFrom Validation
    Modules->>Session: Allow (0)
    Session->>Client: 250 OK

    Client->>Session: RCPT TO:<recipient@remote.com>
    Session->>Modules: RcptTo Validation
    Modules->>Session: Allow (0)
    Session->>Client: 250 OK

    Client->>Session: DATA
    Session->>Client: 354 Start input
    Client->>Session: Message content<br/>.<cr><lf>
    Session->>Modules: Data Validation
    Modules->>Session: Allow (0)
    Session->>Spool: Write Message
    Spool->>Session: Message ID
    Session->>Client: 250 Queued

    Queue->>Spool: Scan for pending
    Spool->>Queue: Message list
    Queue->>Delivery: Process message
    Delivery->>RemoteMTA: SMTP handshake
    RemoteMTA->>Delivery: 250 OK
    Delivery->>Spool: Delete message

    Session->>Modules: ConnectionClosed Event
```

---

## Workspace Structure

### File Organization

```
empath/
├── empath/                    # Main binary crate
│   ├── src/
│   │   ├── main.rs           # Entry point
│   │   ├── config.rs         # Configuration parsing
│   │   └── control_handler.rs # Control socket IPC
│   └── Cargo.toml
│
├── empath-common/            # Core abstractions
│   ├── src/
│   │   ├── traits/
│   │   │   ├── protocol.rs   # Protocol trait
│   │   │   ├── fsm.rs        # FiniteStateMachine trait
│   │   │   └── ...
│   │   ├── context.rs        # Message context
│   │   ├── controller.rs     # Multi-listener controller
│   │   └── listener.rs       # Single-listener handler
│   └── Cargo.toml
│
├── empath-smtp/              # SMTP protocol
│   ├── src/
│   │   ├── lib.rs            # Protocol impl & FSM
│   │   ├── session.rs        # Session handler
│   │   ├── command.rs        # Command parsing
│   │   └── extensions/       # STARTTLS, SIZE, etc.
│   ├── benches/              # Performance benchmarks
│   └── Cargo.toml
│
├── empath-delivery/          # Outbound delivery
│   ├── src/
│   │   ├── lib.rs            # Delivery processor
│   │   ├── queue/            # Queue management
│   │   ├── dns.rs            # MX lookup
│   │   └── processor/        # Delivery logic
│   └── Cargo.toml
│
├── empath-spool/             # Message persistence
│   ├── src/
│   │   ├── spool.rs          # Spool trait
│   │   └── backends/
│   │       ├── file.rs       # Filesystem backend
│   │       └── memory.rs     # In-memory backend
│   └── Cargo.toml
│
├── empath-ffi/               # C API & modules
│   ├── src/
│   │   ├── lib.rs            # C exports
│   │   ├── modules/          # Module loading
│   │   └── string.rs         # FFI string types
│   ├── examples/
│   │   ├── example.c         # Validation module
│   │   └── event.c           # Event listener
│   └── Cargo.toml
│
└── empath-tracing/           # Instrumentation
    ├── src/lib.rs            # #[traced] macro
    └── Cargo.toml
```

---

## Protocol System

### Generic Protocol Architecture

The protocol system is generic, allowing new protocols (IMAP, POP3, etc.) to be added easily.

```mermaid
classDiagram
    class Protocol {
        <<trait>>
        +type Session
        +type Args
        +handle(stream, peer, context, args) Session
        +validate(args) Result
        +ty() &str
    }

    class FiniteStateMachine {
        <<trait>>
        +type Input
        +type Context
        +transition(self, input, context) Self
    }

    class SmtpProtocol {
        +handle() SmtpSession
        +validate() Result
    }

    class SmtpState {
        <<enum>>
        Connect
        Ehlo
        MailFrom
        RcptTo
        Data
        Reading
        PostDot
        Quit
    }

    class SmtpCommand {
        <<enum>>
        Helo(String)
        Ehlo(String)
        MailFrom(Reverse Path)
        RcptTo(Forward Path)
        Data
        Quit
    }

    Protocol <|.. SmtpProtocol
    FiniteStateMachine <|.. SmtpState
    SmtpProtocol --> SmtpState : uses
    SmtpState --> SmtpCommand : transitions on
```

**Key Concepts:**

- **Protocol trait**: Defines how to handle connections for a specific protocol
- **FiniteStateMachine trait**: Defines state transitions based on inputs
- **SmtpProtocol**: Concrete implementation for SMTP
- **Generic infrastructure**: Controller and Listener are generic over Protocol type

---

## Module/Plugin System

### FFI-Based Extension Architecture

```mermaid
graph LR
    subgraph "Empath Core (Rust)"
        ModLoader[Module Loader]
        Validator[Validation Dispatcher]
        EventDispatch[Event Dispatcher]
    end

    subgraph "Modules (C/C++/Rust FFI)"
        Module1[Spam Filter<br/>libspamfilter.so]
        Module2[Rate Limiter<br/>libratelimit.so]
        Module3[Auth Check<br/>libauth.so]
    end

    subgraph "Events"
        Connect[Connect]
        MailFrom[MailFrom]
        RcptTo[RcptTo]
        Data[Data]
        ConnOpen[ConnectionOpened]
        ConnClose[ConnectionClosed]
    end

    ModLoader -->|dlopen| Module1
    ModLoader -->|dlopen| Module2
    ModLoader -->|dlopen| Module3

    Connect --> Validator
    MailFrom --> Validator
    RcptTo --> Validator
    Data --> Validator

    Validator -->|Call| Module1
    Validator -->|Call| Module2
    Validator -->|Call| Module3

    ConnOpen --> EventDispatch
    ConnClose --> EventDispatch

    EventDispatch -->|Notify| Module1
    EventDispatch -->|Notify| Module2
    EventDispatch -->|Notify| Module3
```

**Module Types:**

1. **ValidationListener**: SMTP transaction validation
   - Return 0 = accept, non-zero = reject
   - Events: Connect, MailFrom, RcptTo, Data, StartTls

2. **EventListener**: Lifecycle notifications
   - Events: ConnectionOpened, ConnectionClosed

**Module Interface** (C API):
```c
Mod* declare_module(void);

typedef struct Mod {
    const char* name;
    int (*on_mail_from)(Context* ctx);
    int (*on_rcpt_to)(Context* ctx);
    // ...
} Mod;
```

---

## SMTP State Machine

### State Transitions

```mermaid
stateDiagram-v2
    [*] --> Connect
    Connect --> Ehlo : EHLO
    Connect --> Helo : HELO

    Ehlo --> MailFrom : MAIL FROM
    Ehlo --> StartTLS : STARTTLS
    Ehlo --> Quit : QUIT

    Helo --> MailFrom : MAIL FROM
    Helo --> Quit : QUIT

    StartTLS --> Ehlo : after TLS upgrade

    MailFrom --> RcptTo : RCPT TO
    MailFrom --> Quit : QUIT

    RcptTo --> RcptTo : RCPT TO (multiple recipients)
    RcptTo --> Data : DATA
    RcptTo --> Quit : QUIT

    Data --> Reading : 354 response sent
    Reading --> PostDot : . (terminator)

    PostDot --> MailFrom : next transaction
    PostDot --> Quit : QUIT

    Quit --> [*]
```

**State Validation:**

Each transition is validated by the module system:
- Modules can reject transitions by returning non-zero
- First module to reject wins
- Context is preserved across transitions

---

## Delivery Pipeline

### Outbound Message Processing

```mermaid
flowchart TD
    Start([Scan Timer Fires]) --> Scan[Scan Spool for Pending Messages]
    Scan --> HasMessages{Messages Found?}

    HasMessages -->|No| Wait[Wait for Next Scan]
    Wait --> Start

    HasMessages -->|Yes| CheckRetry{Check Retry Time}
    CheckRetry -->|Not Ready| Wait
    CheckRetry -->|Ready| DNS[DNS MX Lookup]

    DNS --> DNSCache{Cached?}
    DNSCache -->|Yes| UseCached[Use Cached MX Records]
    DNSCache -->|No| Resolve[Resolve MX Records]
    Resolve --> CacheMX[Cache with TTL]
    CacheMX --> UseCached

    UseCached --> Randomize[Randomize Equal Priority MX]
    Randomize --> TryMX[Try Next MX Server]

    TryMX --> Connect[TCP Connect]
    Connect --> ConnectOK{Success?}

    ConnectOK -->|No| NextMX{More MX?}
    NextMX -->|Yes| TryMX
    NextMX -->|No| TempFail[Temporary Failure]

    ConnectOK -->|Yes| TLS{STARTTLS?}
    TLS -->|Yes| UpgradeTLS[Upgrade to TLS]
    TLS -->|No| SMTPHandshake
    UpgradeTLS --> SMTPHandshake[SMTP Handshake]

    SMTPHandshake --> SendMAIL[MAIL FROM]
    SendMAIL --> SendRCPT[RCPT TO]
    SendRCPT --> SendDATA[DATA]
    SendDATA --> Success{Accepted?}

    Success -->|Yes| Delete[Delete from Spool]
    Success -->|No| PermFail{Permanent Error?}

    PermFail -->|Yes| MarkFailed[Mark as Failed]
    PermFail -->|No| TempFail

    TempFail --> Retry{Attempts < Max?}
    Retry -->|Yes| Schedule[Schedule Retry with Backoff]
    Retry -->|No| MarkFailed

    Schedule --> Wait
    MarkFailed --> Wait
    Delete --> Next{More Messages?}
    Next -->|Yes| CheckRetry
    Next -->|No| Wait
```

**Retry Schedule (Exponential Backoff):**
```
Attempt 1: Immediate
Attempt 2: 30 seconds
Attempt 3: 2 minutes
Attempt 4: 8 minutes
Attempt 5: 30 minutes
Attempt 6+: 1 hour each
Max: 25 attempts
```

---

## Control System

### Runtime Management via IPC

```mermaid
graph TB
    subgraph "empathctl CLI"
        CLI[Command Line Interface]
    end

    subgraph "Empath MTA"
        Socket[Unix Domain Socket<br/>/tmp/empath.sock]
        Handler[Control Handler]

        subgraph "Components"
            Queue[Delivery Queue]
            Spool[Message Spool]
            DNS[DNS Cache]
            Delivery[Delivery Processor]
        end
    end

    CLI -->|bincode protocol| Socket
    Socket --> Handler

    Handler -->|System Commands| System[System Status, Ping]
    Handler -->|DNS Commands| DNS
    Handler -->|Queue Commands| Queue
    Handler -->|Queue Commands| Spool

    DNS -->|list-cache<br/>clear-cache<br/>refresh| Handler
    Queue -->|list<br/>stats<br/>retry<br/>delete| Handler
    System -->|status<br/>ping| Handler

    Handler -->|Response| Socket
    Socket -->|bincode| CLI
```

**Available Commands:**

**DNS Management:**
- `dns list-cache` - List cached MX records
- `dns clear-cache` - Clear entire DNS cache
- `dns refresh <domain>` - Refresh specific domain
- `dns list-overrides` - List MX overrides

**Queue Management:**
- `queue list [--status=<status>]` - List messages
- `queue view <message-id>` - View message details
- `queue stats [--watch]` - Queue statistics
- `queue retry <message-id>` - Retry failed message
- `queue delete <message-id>` - Delete message
- `queue freeze` - Pause delivery
- `queue unfreeze` - Resume delivery

**System:**
- `system ping` - Health check
- `system status` - System status and metrics

---

## Key Architectural Patterns

### 1. Generic Protocol System
- `Controller<Proto: Protocol>` and `Listener<Proto: Protocol>` are generic
- New protocols implement the `Protocol` trait
- Reusable connection handling infrastructure

### 2. Finite State Machine
- States have explicit types (not strings/enums with data)
- Transitions are type-safe
- Invalid transitions caught at compile time

### 3. Module/Plugin System
- C FFI for language-agnostic extensions
- Synchronous dispatch (all modules called sequentially)
- First non-zero return rejects transaction

### 4. Controller/Listener Pattern
- Controller manages multiple listeners
- Broadcasts shutdown signals
- Coordinated graceful shutdown

### 5. Spool Abstraction
- `Spool` trait for different backends
- `FileBackedSpool` for production
- `MemoryBackedSpool` for testing

---

## Configuration Flow

```mermaid
flowchart LR
    Config[empath.config.ron] --> Parser[RON Parser]
    Parser --> Validate[Validate Config]
    Validate --> Modules[Load Modules]
    Modules --> SMTP[Create SMTP Controller]
    Modules --> Delivery[Create Delivery Processor]
    Modules --> Spool[Create Spool]
    Modules --> Control[Create Control Socket]

    SMTP --> Listeners[Spawn Listeners]
    Delivery --> Scanner[Start Spool Scanner]
    Delivery --> Processor[Start Queue Processor]
    Control --> Handler[Start Control Handler]

    Listeners --> Run[Main Event Loop]
    Scanner --> Run
    Processor --> Run
    Handler --> Run

    Run --> Shutdown{SIGTERM/SIGINT?}
    Shutdown -->|Yes| Graceful[Graceful Shutdown]
    Shutdown -->|No| Run

    Graceful --> StopListeners[Stop Accepting Connections]
    StopListeners --> WaitDelivery[Wait for In-Flight Delivery]
    WaitDelivery --> SaveState[Persist Queue State]
    SaveState --> Exit[Exit]
```

---

## Performance Considerations

### Optimizations

1. **Lock-Free Concurrency**: DashMap instead of `Arc<RwLock<HashMap>>`
2. **Zero-Copy Parsing**: Minimize allocations in hot paths
3. **Connection Pooling**: Reuse SMTP connections (planned)
4. **DNS Caching**: TTL-based with active eviction
5. **Metrics**: AtomicU64 counters (~90% overhead reduction)

### Benchmarks

- Command parsing: ~100-500ns
- FSM transitions: ~50-200ns
- Spool operations: ~10-50µs
- Full SMTP transaction: ~1-5ms

See [CLAUDE.md Benchmarking section](../CLAUDE.md#benchmarking) for details.

---

## Security Architecture

### Defense in Depth

1. **TLS Certificate Validation**: Enabled by default (configurable per-domain)
2. **Timeout Protection**: State-aware timeouts (RFC 5321 compliant)
3. **Module Sandboxing**: Modules run in-process but validated
4. **Control Socket Security**: Unix permissions, audit logging
5. **Input Validation**: Strict SMTP command parsing
6. **Resource Limits**: Connection limits, message size limits

See [CLAUDE.md Security Considerations](../CLAUDE.md#security-considerations) for details.

---

## Observability

### Metrics, Logs, and Traces

```mermaid
graph LR
    subgraph "Empath"
        Code[Application Code]
        Metrics[Metrics<br/>AtomicU64]
        Logs[Logs<br/>tracing]
        Traces[Traces<br/>OpenTelemetry]
    end

    subgraph "Collection"
        OTEL[OTEL Collector]
    end

    subgraph "Storage & Visualization"
        Prom[Prometheus]
        Jaeger[Jaeger/Tempo]
        Graf[Grafana]
    end

    Code --> Metrics
    Code --> Logs
    Code --> Traces

    Metrics -->|OTLP| OTEL
    Logs -->|OTLP| OTEL
    Traces -->|OTLP| OTEL

    OTEL -->|Metrics| Prom
    OTEL -->|Traces| Jaeger
    OTEL -->|Logs| Jaeger

    Prom --> Graf
    Jaeger --> Graf
```

**Key Metrics:**
- `empath_connections_total` - Total SMTP connections
- `empath_messages_received` - Messages accepted
- `empath_delivery_attempts_total` - Delivery attempts by domain/status
- `dns_cache_hits` / `dns_cache_misses` - DNS cache effectiveness

---

## Further Reading

- [README.md](../README.md) - Project overview and quick start
- [CLAUDE.md](../CLAUDE.md) - Detailed implementation guide
- [docs/ONBOARDING.md](ONBOARDING.md) - New developer guide
- [docs/TROUBLESHOOTING.md](TROUBLESHOOTING.md) - Common issues
- [CONTRIBUTING.md](../CONTRIBUTING.md) - Contribution guidelines
