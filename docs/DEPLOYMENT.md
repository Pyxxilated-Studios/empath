# Deployment Guide

This guide provides comprehensive instructions for deploying and operating Empath MTA in production environments.

## Table of Contents

- [System Requirements](#system-requirements)
- [Installation](#installation)
  - [From Source](#from-source)
  - [Docker Deployment](#docker-deployment)
  - [Kubernetes Deployment](#kubernetes-deployment)
- [Configuration](#configuration)
  - [Production Configuration](#production-configuration)
  - [TLS Setup](#tls-setup)
  - [Performance Tuning](#performance-tuning)
- [Monitoring and Observability](#monitoring-and-observability)
  - [OpenTelemetry Setup](#opentelemetry-setup)
  - [Prometheus Integration](#prometheus-integration)
  - [Grafana Dashboards](#grafana-dashboards)
  - [Health Checks](#health-checks)
- [Operational Procedures](#operational-procedures)
  - [Starting and Stopping](#starting-and-stopping)
  - [Queue Management](#queue-management)
  - [DNS Cache Management](#dns-cache-management)
  - [Backup and Recovery](#backup-and-recovery)
- [Troubleshooting](#troubleshooting)
- [Maintenance](#maintenance)
- [Scaling](#scaling)

---

## System Requirements

### Hardware Requirements

**Minimum (Development/Testing):**
- CPU: 2 cores
- RAM: 2GB
- Disk: 10GB available space
- Network: 100 Mbps

**Recommended (Production):**
- CPU: 4-8 cores
- RAM: 8-16GB (depending on queue size and throughput)
- Disk: 100GB+ SSD (for spool and logs)
- Network: 1 Gbps+

**Disk Performance:**
- SSD strongly recommended for spool directory
- IOPS requirements: 1000+ for production workloads
- Latency requirements: <10ms write latency

### Software Requirements

**Operating System:**
- Linux (Ubuntu 20.04+, Debian 11+, RHEL 8+, or similar)
- macOS 11+ (development only)
- Windows (limited support, development only)

**Rust Toolchain:**
- Rust nightly (channel: nightly)
- Version: rustc 1.93.0-nightly or later
- Components: rustfmt, clippy
- Install: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`

**Build Tools:**
- mold linker (Linux only, optional but recommended for 40-60% faster builds)
  ```bash
  # Ubuntu/Debian
  sudo apt install mold

  # RHEL/Fedora
  sudo dnf install mold
  ```

- just task runner (optional but recommended)
  ```bash
  cargo install just
  ```

**Runtime Dependencies:**
- None (statically linked binaries)
- OpenSSL/LibreSSL (for TLS support)

### Network Requirements

**Ports:**
- **25** - SMTP (production)
- **587** - SMTP Submission (optional)
- **8080** - Health checks (Kubernetes)
- **4318** - OpenTelemetry OTLP HTTP (monitoring)

**Firewall Configuration:**
```bash
# Allow SMTP
sudo ufw allow 25/tcp comment 'SMTP'

# Allow health checks (from Kubernetes nodes only)
sudo ufw allow from 10.0.0.0/8 to any port 8080 proto tcp comment 'Health checks'

# Allow metrics (from monitoring network only)
sudo ufw allow from 10.1.0.0/16 to any port 4318 proto tcp comment 'OTLP metrics'
```

---

## Installation

### From Source

#### 1. Install Rust Toolchain

```bash
# Install rustup
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Configure current shell
source $HOME/.cargo/env

# Install nightly toolchain
rustup install nightly
rustup default nightly

# Verify installation
rustc --version  # Should show 1.93.0-nightly or later
```

#### 2. Clone Repository

```bash
git clone https://github.com/Pyxxilated-Studios/empath.git
cd empath
```

#### 3. Build Release Binary

```bash
# Option 1: Using just task runner
just setup        # Install development tools
just build-release

# Option 2: Using cargo directly
cargo build --release

# Binaries will be in target/release/
# - empath        (MTA daemon)
# - empathctl     (CLI utility)
```

#### 4. Install System-Wide

```bash
# Install binaries
sudo cp target/release/empath /usr/local/bin/
sudo cp target/release/empathctl /usr/local/bin/

# Create directories
sudo mkdir -p /etc/empath
sudo mkdir -p /var/spool/empath
sudo mkdir -p /var/log/empath
sudo mkdir -p /var/run/empath

# Create empath user
sudo useradd -r -s /bin/false -d /var/spool/empath empath

# Set permissions
sudo chown -R empath:empath /var/spool/empath
sudo chown -R empath:empath /var/log/empath
sudo chown empath:empath /var/run/empath
sudo chmod 700 /var/spool/empath
sudo chmod 755 /var/run/empath
```

#### 5. Create Configuration

```bash
# Copy example configuration
sudo cp examples/config/production.ron /etc/empath/empath.config.ron
sudo chown empath:empath /etc/empath/empath.config.ron
sudo chmod 600 /etc/empath/empath.config.ron

# Edit configuration
sudo nano /etc/empath/empath.config.ron
```

#### 6. Create Systemd Service

Create `/etc/systemd/system/empath.service`:

```ini
[Unit]
Description=Empath Mail Transfer Agent
Documentation=https://github.com/Pyxxilated-Studios/empath
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=empath
Group=empath
ExecStart=/usr/local/bin/empath /etc/empath/empath.config.ron
ExecReload=/bin/kill -HUP $MAINPID
Restart=on-failure
RestartSec=5s

# Security hardening
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/spool/empath /var/run/empath /var/log/empath

# Resource limits
LimitNOFILE=65536
LimitNPROC=512

# Environment
Environment=RUST_LOG=info
Environment=RUST_BACKTRACE=1

[Install]
WantedBy=multi-user.target
```

Enable and start:

```bash
sudo systemctl daemon-reload
sudo systemctl enable empath
sudo systemctl start empath
sudo systemctl status empath
```

---

### Docker Deployment

#### Quick Start

```bash
# Clone repository
git clone https://github.com/Pyxxilated-Studios/empath.git
cd empath/docker

# Start full stack (Empath + OpenTelemetry + Prometheus + Grafana)
docker-compose -f compose.dev.yml up -d

# View logs
docker-compose -f compose.dev.yml logs -f empath

# Access Grafana
open http://localhost:3000  # admin/admin
```

#### Production Docker Build

**Dockerfile** (multi-stage build with cargo-chef):

```bash
# Build production image
docker build -t empath:latest -f docker/Dockerfile .

# Run container
docker run -d \
  --name empath \
  -p 25:25 \
  -p 8080:8080 \
  -v /var/spool/empath:/var/spool/empath \
  -v /etc/empath:/etc/empath:ro \
  -e RUST_LOG=info \
  empath:latest
```

#### Docker Compose Production

Create `docker-compose.prod.yml`:

```yaml
version: '3.8'

services:
  empath:
    image: empath:latest
    container_name: empath
    restart: always
    ports:
      - "25:25"
      - "8080:8080"
    volumes:
      - /var/spool/empath:/var/spool/empath
      - /etc/empath/empath.config.ron:/app/empath.config.ron:ro
      - /etc/empath/tls:/etc/empath/tls:ro
    environment:
      - RUST_LOG=info
      - RUST_BACKTRACE=1
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:8080/health/live"]
      interval: 10s
      timeout: 2s
      retries: 3
      start_period: 30s
    networks:
      - empath-network

networks:
  empath-network:
    driver: bridge
```

Start production stack:

```bash
docker-compose -f docker-compose.prod.yml up -d
```

---

### Kubernetes Deployment

#### Namespace and ConfigMap

```yaml
apiVersion: v1
kind: Namespace
metadata:
  name: empath

---
apiVersion: v1
kind: ConfigMap
metadata:
  name: empath-config
  namespace: empath
data:
  empath.config.ron: |
    Empath(
      smtp_controller: (
        listeners: [
          {
            socket: "[::]:25",
            context: {
              "service": "smtp",
            },
            timeouts: (
              command_secs: 300,
              data_init_secs: 120,
              data_block_secs: 180,
              data_termination_secs: 600,
              connection_secs: 1800,
            ),
            extensions: [
              { "size": 25000000 },
            ],
          },
        ],
      ),
      spool: (
        path: "/var/spool/empath",
      ),
      delivery: (
        scan_interval_secs: 30,
        process_interval_secs: 10,
        max_attempts: 25,
        accept_invalid_certs: false,
      ),
      health: (
        enabled: true,
        listen_address: "[::]:8080",
        max_queue_size: 10000,
      ),
      metrics: (
        enabled: true,
        endpoint: "http://otel-collector:4318/v1/metrics",
      ),
      control_socket: "/var/run/empath.sock",
    )
```

#### Deployment

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: empath
  namespace: empath
spec:
  replicas: 3
  selector:
    matchLabels:
      app: empath
  template:
    metadata:
      labels:
        app: empath
    spec:
      containers:
      - name: empath
        image: empath:latest
        ports:
        - containerPort: 25
          name: smtp
          protocol: TCP
        - containerPort: 8080
          name: health
          protocol: TCP
        volumeMounts:
        - name: config
          mountPath: /app/empath.config.ron
          subPath: empath.config.ron
          readOnly: true
        - name: spool
          mountPath: /var/spool/empath
        env:
        - name: RUST_LOG
          value: "info"
        - name: RUST_BACKTRACE
          value: "1"
        resources:
          requests:
            cpu: 500m
            memory: 512Mi
          limits:
            cpu: 2000m
            memory: 2Gi
        livenessProbe:
          httpGet:
            path: /health/live
            port: 8080
          initialDelaySeconds: 10
          periodSeconds: 10
          timeoutSeconds: 1
          failureThreshold: 3
        readinessProbe:
          httpGet:
            path: /health/ready
            port: 8080
          initialDelaySeconds: 5
          periodSeconds: 5
          timeoutSeconds: 1
          failureThreshold: 3
        securityContext:
          runAsNonRoot: true
          runAsUser: 1000
          allowPrivilegeEscalation: false
          capabilities:
            drop:
            - ALL
      volumes:
      - name: config
        configMap:
          name: empath-config
      - name: spool
        persistentVolumeClaim:
          claimName: empath-spool-pvc
```

#### Service

```yaml
apiVersion: v1
kind: Service
metadata:
  name: empath-smtp
  namespace: empath
spec:
  type: LoadBalancer
  ports:
  - port: 25
    targetPort: 25
    protocol: TCP
    name: smtp
  selector:
    app: empath

---
apiVersion: v1
kind: Service
metadata:
  name: empath-health
  namespace: empath
spec:
  type: ClusterIP
  ports:
  - port: 8080
    targetPort: 8080
    protocol: TCP
    name: health
  selector:
    app: empath
```

#### Persistent Volume Claim

```yaml
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: empath-spool-pvc
  namespace: empath
spec:
  accessModes:
  - ReadWriteOnce
  resources:
    requests:
      storage: 100Gi
  storageClassName: ssd
```

Deploy to Kubernetes:

```bash
kubectl apply -f kubernetes/namespace.yaml
kubectl apply -f kubernetes/configmap.yaml
kubectl apply -f kubernetes/pvc.yaml
kubectl apply -f kubernetes/deployment.yaml
kubectl apply -f kubernetes/service.yaml

# Verify deployment
kubectl -n empath get pods
kubectl -n empath logs -f deployment/empath
```

---

## Configuration

### Production Configuration

**File:** `/etc/empath/empath.config.ron`

```ron
Empath(
    // SMTP Controller - Accept incoming mail
    smtp_controller: (
        listeners: [
            {
                // Production SMTP on standard port
                socket: "[::]:25",

                // Context metadata for sessions
                context: {
                    "service": "smtp",
                    "environment": "production",
                },

                // RFC 5321 compliant timeouts (prevents DoS)
                timeouts: (
                    command_secs: 300,          // 5 min for regular commands
                    data_init_secs: 120,        // 2 min for DATA command
                    data_block_secs: 180,       // 3 min between data chunks
                    data_termination_secs: 600, // 10 min for processing after final dot
                    connection_secs: 1800,      // 30 min maximum session lifetime
                ),

                // SMTP Extensions
                extensions: [
                    // SIZE - Declare maximum message size (RFC 1870)
                    { "size": 25000000 },  // 25MB limit

                    // STARTTLS - Opportunistic encryption (RFC 3207)
                    {
                        "starttls": {
                            "key": "/etc/empath/tls/private.key",
                            "certificate": "/etc/empath/tls/certificate.crt",
                        }
                    },
                ],
            },
        ],
    ),

    // Spool Configuration - Message persistence
    spool: (
        path: "/var/spool/empath",
    ),

    // Delivery Configuration - Outbound mail
    delivery: (
        // Queue processing intervals
        scan_interval_secs: 30,      // How often to scan spool for new messages
        process_interval_secs: 10,   // How often to process delivery queue

        // Retry configuration
        max_attempts: 25,             // Maximum delivery attempts before giving up
        base_retry_delay_secs: 60,    // Initial retry delay (1 minute)
        max_retry_delay_secs: 86400,  // Maximum retry delay (24 hours)
        retry_jitter_factor: 0.2,     // ±20% randomness to prevent thundering herd

        // TLS certificate validation (SECURITY CRITICAL)
        accept_invalid_certs: false,  // NEVER set to true in production!

        // SMTP client timeouts
        smtp_timeouts: (
            connect_secs: 30,      // Connection establishment
            ehlo_secs: 30,         // EHLO/HELO command
            starttls_secs: 30,     // STARTTLS command
            mail_from_secs: 30,    // MAIL FROM command
            rcpt_to_secs: 30,      // RCPT TO command
            data_secs: 120,        // DATA command and message transmission
            quit_secs: 10,         // QUIT command
        ),

        // Per-domain configuration
        domains: {
            // Example: Internal relay with self-signed cert
            "internal.company.com": (
                mx_override: "relay.company.com:25",
                accept_invalid_certs: false,  // Still validate certs!
            ),
        },
    ),

    // Health Check Configuration (Kubernetes)
    health: (
        enabled: true,
        listen_address: "[::]:8080",
        max_queue_size: 10000,  // Readiness fails if queue exceeds this
    ),

    // Metrics Configuration (OpenTelemetry)
    metrics: (
        enabled: true,
        endpoint: "http://otel-collector:4318/v1/metrics",
    ),

    // Control Socket (empathctl CLI)
    control_socket: "/var/run/empath.sock",

    // Optional: Dynamic modules (FFI plugins)
    modules: [
        // Example: Spam filter module
        // (
        //     type: "SharedLibrary",
        //     name: "/etc/empath/modules/spam_filter.so",
        //     arguments: ["--threshold", "5.0"],
        // ),
    ],
)
```

**Production Deployment Checklist:**

- [ ] TLS certificates installed in `/etc/empath/tls/`
- [ ] Spool directory created: `mkdir -p /var/spool/empath`
- [ ] Set proper ownership: `chown -R empath:empath /var/spool/empath`
- [ ] Control socket directory: `mkdir -p /var/run/empath`
- [ ] Firewall: allow port 25 inbound
- [ ] Security: run as dedicated 'empath' user (not root)
- [ ] Monitoring: OpenTelemetry Collector configured
- [ ] Health checks: Kubernetes probes configured
- [ ] Backup: Automated spool backup configured
- [ ] Logging: Log rotation configured (logrotate)
- [ ] DNS: Verify DNS resolution for delivery domains
- [ ] TLS validation: `accept_invalid_certs: false` (verify!)

---

### TLS Setup

#### Generate Self-Signed Certificate (Testing Only)

```bash
# Generate private key
openssl genrsa -out /etc/empath/tls/private.key 4096

# Generate self-signed certificate (valid for 1 year)
openssl req -new -x509 -key /etc/empath/tls/private.key \
  -out /etc/empath/tls/certificate.crt -days 365 \
  -subj "/C=US/ST=State/L=City/O=Organization/CN=mail.example.com"

# Set permissions
chmod 600 /etc/empath/tls/private.key
chmod 644 /etc/empath/tls/certificate.crt
chown -R empath:empath /etc/empath/tls
```

#### Let's Encrypt Certificate (Production)

```bash
# Install certbot
sudo apt install certbot  # Debian/Ubuntu
sudo dnf install certbot  # RHEL/Fedora

# Obtain certificate (HTTP-01 challenge)
sudo certbot certonly --standalone -d mail.example.com

# Certificates will be in:
# /etc/letsencrypt/live/mail.example.com/privkey.pem
# /etc/letsencrypt/live/mail.example.com/fullchain.pem

# Create symlinks (or copy)
sudo ln -s /etc/letsencrypt/live/mail.example.com/privkey.pem \
  /etc/empath/tls/private.key
sudo ln -s /etc/letsencrypt/live/mail.example.com/fullchain.pem \
  /etc/empath/tls/certificate.crt

# Set permissions
sudo chown -h empath:empath /etc/empath/tls/*.{key,crt}

# Auto-renewal (certbot installs systemd timer automatically)
sudo certbot renew --dry-run  # Test renewal
```

#### Certificate Renewal with Reload

Create `/etc/letsencrypt/renewal-hooks/deploy/empath-reload.sh`:

```bash
#!/bin/bash
# Reload Empath after certificate renewal

systemctl reload empath
logger "Empath MTA: TLS certificate renewed and reloaded"
```

Make executable:

```bash
chmod +x /etc/letsencrypt/renewal-hooks/deploy/empath-reload.sh
```

#### Verify TLS Configuration

```bash
# Test STARTTLS from command line
openssl s_client -starttls smtp -connect mail.example.com:25

# Should show certificate details and TLS protocol version
# Look for: "Protocol  : TLSv1.3" or "TLSv1.2"

# Check certificate expiration
openssl s_client -starttls smtp -connect mail.example.com:25 2>/dev/null | \
  openssl x509 -noout -dates
```

---

### Performance Tuning

#### System Limits

Edit `/etc/security/limits.conf`:

```
empath soft nofile 65536
empath hard nofile 65536
empath soft nproc 4096
empath hard nproc 4096
```

Edit `/etc/sysctl.conf`:

```
# TCP tuning
net.core.somaxconn = 4096
net.ipv4.tcp_max_syn_backlog = 4096
net.ipv4.ip_local_port_range = 1024 65535

# Connection tracking
net.netfilter.nf_conntrack_max = 262144

# File system
fs.file-max = 500000
```

Apply changes:

```bash
sudo sysctl -p
```

#### Empath Configuration Tuning

**High-Throughput Workload:**

```ron
delivery: (
    scan_interval_secs: 10,      // Faster spool scanning
    process_interval_secs: 5,    // Faster queue processing
    max_attempts: 15,             // Fewer retries for faster failure

    smtp_timeouts: (
        connect_secs: 10,         // Tighter timeouts
        data_secs: 60,            // Faster for small messages
    ),
)
```

**Large Message Handling:**

```ron
extensions: [
    { "size": 52428800 },  // 50MB limit for large attachments
],

smtp_timeouts: (
    data_secs: 300,  // 5 minutes for large message transmission
)
```

**Low-Resource Environment:**

```ron
delivery: (
    scan_interval_secs: 60,      // Less frequent scanning
    process_interval_secs: 30,   // Less frequent processing
)

health: (
    max_queue_size: 5000,  // Lower queue threshold
)
```

#### Spool Performance

**SSD Optimization:**

```bash
# Mount spool with noatime for better performance
# Add to /etc/fstab:
/dev/sdb1  /var/spool/empath  ext4  noatime,data=ordered  0  2

# Remount
sudo mount -o remount /var/spool/empath
```

**Directory Structure:**

For very high throughput (>1000 msg/s), consider partitioning spool:

```bash
# Create subdirectories (0-9, a-f)
for i in {0..9} {a..f}; do
  mkdir -p /var/spool/empath/$i
done

# Distribute messages by first character of message ID
# (Requires code modification - future enhancement)
```

---

## Monitoring and Observability

### OpenTelemetry Setup

#### Install OpenTelemetry Collector

**Docker:**

```yaml
version: '3.8'

services:
  otel-collector:
    image: otel/opentelemetry-collector-contrib:latest
    container_name: otel-collector
    command: ["--config=/etc/otel-collector.yml"]
    volumes:
      - ./otel-collector.yml:/etc/otel-collector.yml:ro
    ports:
      - "4317:4317"  # OTLP gRPC
      - "4318:4318"  # OTLP HTTP
      - "8888:8888"  # Prometheus metrics (collector's own)
      - "8889:8889"  # Prometheus exporter
    networks:
      - monitoring
```

**Configuration** (`otel-collector.yml`):

```yaml
receivers:
  otlp:
    protocols:
      grpc:
        endpoint: 0.0.0.0:4317
      http:
        endpoint: 0.0.0.0:4318

processors:
  batch:
    timeout: 10s
    send_batch_size: 1024

  resource:
    attributes:
      - key: service.name
        value: empath-mta
        action: upsert
      - key: deployment.environment
        value: production
        action: upsert

  memory_limiter:
    check_interval: 1s
    limit_mib: 512

exporters:
  prometheus:
    endpoint: "0.0.0.0:8889"
    namespace: empath

  logging:
    loglevel: info

service:
  pipelines:
    metrics:
      receivers: [otlp]
      processors: [memory_limiter, resource, batch]
      exporters: [prometheus, logging]
```

Start collector:

```bash
docker-compose up -d otel-collector
```

---

### Prometheus Integration

#### Install Prometheus

**Docker:**

```yaml
prometheus:
  image: prom/prometheus:latest
  container_name: prometheus
  command:
    - '--config.file=/etc/prometheus/prometheus.yml'
    - '--storage.tsdb.path=/prometheus'
    - '--storage.tsdb.retention.time=30d'
  volumes:
    - ./prometheus.yml:/etc/prometheus/prometheus.yml:ro
    - prometheus-data:/prometheus
  ports:
    - "9090:9090"
  networks:
    - monitoring

volumes:
  prometheus-data:
```

**Configuration** (`prometheus.yml`):

```yaml
global:
  scrape_interval: 15s
  evaluation_interval: 15s

scrape_configs:
  # Scrape OpenTelemetry Collector
  - job_name: 'empath-mta'
    static_configs:
      - targets: ['otel-collector:8889']
        labels:
          service: 'empath'
          environment: 'production'

  # Scrape Collector's own metrics
  - job_name: 'otel-collector'
    static_configs:
      - targets: ['otel-collector:8888']
```

Start Prometheus:

```bash
docker-compose up -d prometheus
```

Access Prometheus UI: `http://localhost:9090`

---

### Grafana Dashboards

#### Install Grafana

**Docker:**

```yaml
grafana:
  image: grafana/grafana:latest
  container_name: grafana
  ports:
    - "3000:3000"
  volumes:
    - grafana-data:/var/lib/grafana
    - ./grafana/provisioning:/etc/grafana/provisioning:ro
  environment:
    - GF_SECURITY_ADMIN_PASSWORD=admin
    - GF_INSTALL_PLUGINS=
  networks:
    - monitoring

volumes:
  grafana-data:
```

#### Provision Prometheus Datasource

Create `grafana/provisioning/datasources/prometheus.yml`:

```yaml
apiVersion: 1

datasources:
  - name: Prometheus
    type: prometheus
    access: proxy
    url: http://prometheus:9090
    isDefault: true
    editable: false
```

#### Empath MTA Dashboard

The project includes a pre-built Grafana dashboard with panels for:

**SMTP Metrics:**
- Total connections
- Active connections (gauge)
- Errors by SMTP code
- Session duration (histogram)
- Command processing time
- Messages received
- Message size distribution

**Delivery Metrics:**
- Delivery attempts by status and domain
- Delivery duration by domain
- Queue size by status (gauge)
- Active outbound connections
- Successful deliveries
- Failed deliveries by reason
- Messages in retry state
- Retry count distribution

**DNS Metrics:**
- DNS lookup duration
- Total lookups by query type
- Cache hits and misses
- DNS errors by type
- Cache evictions

**Import Dashboard:**

1. Access Grafana: `http://localhost:3000` (admin/admin)
2. Navigate to **Dashboards** → **Import**
3. Upload `docker/grafana/provisioning/dashboards/grafana-dashboard.json`
4. Select Prometheus datasource
5. Click **Import**

Dashboard will be available at: **Dashboards** → **Empath MTA**

---

### Health Checks

#### Kubernetes Liveness Probe

```yaml
livenessProbe:
  httpGet:
    path: /health/live
    port: 8080
  initialDelaySeconds: 10
  periodSeconds: 10
  timeoutSeconds: 1
  failureThreshold: 3
```

**What it checks:**
- HTTP server is responding
- Application is not deadlocked

**Failure action:** Kubernetes restarts the pod

#### Kubernetes Readiness Probe

```yaml
readinessProbe:
  httpGet:
    path: /health/ready
    port: 8080
  initialDelaySeconds: 5
  periodSeconds: 5
  timeoutSeconds: 1
  failureThreshold: 3
```

**What it checks:**
- SMTP listeners bound and accepting connections
- Spool is writable
- Delivery processor is running
- DNS resolver is operational
- Queue size below threshold (default: 10,000 messages)

**Failure action:** Kubernetes removes pod from service endpoints

#### Manual Health Check

```bash
# Liveness
curl http://localhost:8080/health/live
# Returns: OK (200)

# Readiness
curl http://localhost:8080/health/ready
# Returns: OK (200) if ready
# Returns: 503 with JSON details if not ready

# Example not-ready response:
# {
#   "alive": true,
#   "ready": false,
#   "smtp_ready": true,
#   "spool_ready": true,
#   "delivery_ready": false,
#   "dns_ready": true,
#   "queue_size": 15000,
#   "max_queue_size": 10000
# }
```

---

## Operational Procedures

### Starting and Stopping

#### Systemd

```bash
# Start service
sudo systemctl start empath

# Stop service (graceful shutdown with 30s delivery timeout)
sudo systemctl stop empath

# Restart service
sudo systemctl restart empath

# Reload configuration (SIGHUP - not yet implemented)
# sudo systemctl reload empath

# Check status
sudo systemctl status empath

# View logs
sudo journalctl -u empath -f
```

#### Docker

```bash
# Start container
docker start empath

# Stop container (graceful shutdown)
docker stop empath

# Restart container
docker restart empath

# View logs
docker logs -f empath
```

#### Kubernetes

```bash
# Scale deployment
kubectl -n empath scale deployment empath --replicas=5

# Rolling restart
kubectl -n empath rollout restart deployment empath

# View logs
kubectl -n empath logs -f deployment/empath

# Check pod status
kubectl -n empath get pods
kubectl -n empath describe pod <pod-name>
```

---

### Queue Management

#### Using empathctl CLI

**List Messages:**

```bash
# List all messages in queue
empathctl queue list

# Filter by status
empathctl queue list --status=failed
empathctl queue list --status=retry

# Available statuses:
# - pending
# - in-progress
# - completed
# - failed
# - retry
# - expired
```

**View Message Details:**

```bash
# View specific message
empathctl queue view 01JCXYZ...

# Output includes:
# - Message ID, sender, recipients
# - Delivery status and attempts
# - Next retry time
# - Last error message
# - Message headers
# - Body preview (first 1KB)
```

**Retry Failed Delivery:**

```bash
# Retry specific message
empathctl queue retry 01JCXYZ...

# Force retry even if not failed
empathctl queue retry 01JCXYZ... --force
```

**Delete Message:**

```bash
# Delete message (with confirmation)
empathctl queue delete 01JCXYZ...

# Delete without confirmation
empathctl queue delete 01JCXYZ... --yes
```

**Queue Statistics:**

```bash
# Show statistics
empathctl queue stats

# Output includes:
# - Total messages
# - Messages by status
# - Messages by domain
# - Oldest message age

# Watch mode (live updates)
empathctl queue stats --watch --interval 2
```

**Manual Queue Processing:**

```bash
# Trigger immediate queue processing
empathctl queue process-now

# Note: Currently returns informational message
# Manual triggering to be implemented in future release
```

---

### DNS Cache Management

```bash
# List DNS cache with TTLs
empathctl dns list-cache

# Clear entire DNS cache (nuclear option)
empathctl dns clear-cache

# Refresh specific domain
empathctl dns refresh example.com

# List MX overrides
empathctl dns list-overrides
```

**Use Cases:**

- **Refresh domain:** After DNS records change (TTL bypass)
- **Clear cache:** After DNS infrastructure changes
- **List cache:** Debugging delivery issues

---

### Backup and Recovery

#### Spool Backup

**Daily Backup Script** (`/etc/cron.daily/empath-backup`):

```bash
#!/bin/bash
# Empath spool backup script

BACKUP_DIR="/backup/empath"
SPOOL_DIR="/var/spool/empath"
RETENTION_DAYS=30

# Create backup directory
mkdir -p "$BACKUP_DIR"

# Backup spool with compression
tar -czf "$BACKUP_DIR/spool-$(date +%Y%m%d-%H%M%S).tar.gz" \
  -C /var/spool empath

# Backup queue state
cp "$SPOOL_DIR/queue_state.bin" \
  "$BACKUP_DIR/queue_state-$(date +%Y%m%d-%H%M%S).bin"

# Remove old backups
find "$BACKUP_DIR" -name "spool-*.tar.gz" -mtime +$RETENTION_DAYS -delete
find "$BACKUP_DIR" -name "queue_state-*.bin" -mtime +$RETENTION_DAYS -delete

# Log backup
logger "Empath spool backup completed"
```

Make executable:

```bash
chmod +x /etc/cron.daily/empath-backup
```

#### Restore from Backup

```bash
# Stop Empath
sudo systemctl stop empath

# Restore spool
sudo tar -xzf /backup/empath/spool-20250115-120000.tar.gz -C /var/spool

# Restore queue state (optional - for retry schedules)
sudo cp /backup/empath/queue_state-20250115-120000.bin \
  /var/spool/empath/queue_state.bin

# Set permissions
sudo chown -R empath:empath /var/spool/empath

# Start Empath
sudo systemctl start empath

# Verify messages restored
empathctl queue stats
```

#### Configuration Backup

```bash
# Backup configuration
sudo cp /etc/empath/empath.config.ron \
  /backup/empath/empath.config-$(date +%Y%m%d).ron

# Backup TLS certificates
sudo tar -czf /backup/empath/tls-$(date +%Y%m%d).tar.gz \
  -C /etc/empath tls
```

---

## Troubleshooting

### Common Issues

#### 1. Empath Won't Start

**Symptom:** `systemctl start empath` fails

**Diagnosis:**

```bash
# Check logs
sudo journalctl -u empath -n 50

# Check configuration syntax
/usr/local/bin/empath /etc/empath/empath.config.ron --dry-run

# Check permissions
ls -la /var/spool/empath
ls -la /etc/empath/empath.config.ron

# Check if port 25 is already in use
sudo netstat -tulpn | grep :25
```

**Solutions:**

- **Configuration error:** Fix syntax in `empath.config.ron`
- **Permission denied:** `chown empath:empath /var/spool/empath`
- **Port already in use:** Stop conflicting service (postfix, sendmail)
- **Missing directories:** `mkdir -p /var/spool/empath /var/run/empath`

---

#### 2. Messages Stuck in Queue

**Symptom:** Messages not being delivered, queue growing

**Diagnosis:**

```bash
# Check queue statistics
empathctl queue stats

# List failed messages
empathctl queue list --status=failed

# View specific message
empathctl queue view <message-id>

# Check delivery processor logs
sudo journalctl -u empath | grep delivery

# Check DNS resolution
dig +short mx example.com
empathctl dns list-cache
```

**Solutions:**

- **DNS failure:** Check DNS resolver configuration, refresh cache
- **Network connectivity:** Check firewall rules, routing
- **Recipient server down:** Wait for retry (exponential backoff)
- **Certificate validation failed:** Check TLS configuration
- **Message size exceeded:** Recipient server SIZE limit

**Force retry:**

```bash
# Retry specific message
empathctl queue retry <message-id> --force
```

---

#### 3. High Memory Usage

**Symptom:** Empath consuming excessive RAM

**Diagnosis:**

```bash
# Check memory usage
ps aux | grep empath
top -p $(pgrep empath)

# Check queue size
empathctl queue stats

# Check DNS cache size
empathctl dns list-cache | wc -l
```

**Solutions:**

- **Large queue:** Process or delete old messages
- **DNS cache bloat:** Clear cache with `empathctl dns clear-cache`
- **Memory leak:** Restart Empath, report issue with logs

---

#### 4. TLS Handshake Failures

**Symptom:** STARTTLS failures in logs

**Diagnosis:**

```bash
# Test TLS from client
openssl s_client -starttls smtp -connect localhost:25

# Check certificate validity
openssl x509 -in /etc/empath/tls/certificate.crt -noout -dates

# Check certificate permissions
ls -la /etc/empath/tls/
```

**Solutions:**

- **Certificate expired:** Renew certificate (Let's Encrypt or manual)
- **Permission denied:** `chmod 600 private.key; chown empath:empath private.key`
- **Invalid certificate:** Generate new certificate
- **Certificate/key mismatch:** Verify certificate matches private key

---

#### 5. Control Socket Connection Failed

**Symptom:** `empathctl` commands fail with connection error

**Diagnosis:**

```bash
# Check if Empath is running
systemctl status empath

# Check socket file
ls -la /var/run/empath.sock

# Test socket manually
echo 'test' | nc -U /var/run/empath.sock
```

**Solutions:**

- **Empath not running:** `systemctl start empath`
- **Socket file missing:** Check configuration, restart Empath
- **Permission denied:** Add user to empath group or adjust socket permissions
- **Stale socket:** Remove socket file and restart Empath

---

### Log Analysis

#### Enable Debug Logging

```bash
# Temporary (environment variable)
sudo RUST_LOG=debug systemctl restart empath

# Permanent (systemd service)
sudo systemctl edit empath
```

Add:

```ini
[Service]
Environment=RUST_LOG=debug
```

Restart:

```bash
sudo systemctl daemon-reload
sudo systemctl restart empath
```

#### Search Logs

```bash
# Filter by level
sudo journalctl -u empath | grep ERROR
sudo journalctl -u empath | grep WARN

# Filter by component
sudo journalctl -u empath | grep smtp
sudo journalctl -u empath | grep delivery
sudo journalctl -u empath | grep dns

# Search for specific message ID
sudo journalctl -u empath | grep 01JCXYZ...

# Follow logs in real-time
sudo journalctl -u empath -f
```

---

## Maintenance

### Log Rotation

Create `/etc/logrotate.d/empath`:

```
/var/log/empath/*.log {
    daily
    missingok
    rotate 30
    compress
    delaycompress
    notifempty
    create 0640 empath empath
    sharedscripts
    postrotate
        systemctl reload empath > /dev/null 2>&1 || true
    endscript
}
```

### Update Procedure

```bash
# Pull latest code
cd /path/to/empath
git pull origin main

# Build new release
cargo build --release

# Stop service
sudo systemctl stop empath

# Backup current binary
sudo cp /usr/local/bin/empath /usr/local/bin/empath.old

# Install new binary
sudo cp target/release/empath /usr/local/bin/
sudo cp target/release/empathctl /usr/local/bin/

# Start service
sudo systemctl start empath

# Verify
sudo systemctl status empath
empathctl system status
```

### Security Updates

```bash
# Check for security advisories
cargo audit

# Update dependencies
cargo update

# Rebuild and deploy (follow update procedure above)
```

---

## Scaling

### Horizontal Scaling (Kubernetes)

Empath supports horizontal scaling with shared spool storage:

```bash
# Scale to 5 replicas
kubectl -n empath scale deployment empath --replicas=5

# Use autoscaling
kubectl -n empath autoscale deployment empath \
  --min=3 --max=10 --cpu-percent=70
```

**Requirements:**

- **Shared spool:** Use ReadWriteMany (RWX) PersistentVolume
  - NFS, CephFS, or cloud-native (EFS, Azure Files, GCE Persistent Disk)
- **Control socket:** Unique per pod (use hostname in path)
- **Health checks:** Configured for each pod

**Limitations:**

- **Queue processing:** Only one pod processes delivery queue at a time (future: distributed locking)
- **DNS cache:** Not shared between pods (future: Redis cache)

### Vertical Scaling

Increase resources for single instance:

```yaml
resources:
  requests:
    cpu: 2000m      # 2 cores
    memory: 4Gi
  limits:
    cpu: 4000m      # 4 cores
    memory: 8Gi
```

### Load Balancing

**Layer 4 (TCP) Load Balancer:**

```
         ┌──────────────┐
         │ Load Balancer │ (HAProxy, NGINX, or Cloud LB)
         └───────┬───────┘
                 │
       ┌─────────┼─────────┐
       │         │         │
   ┌───▼───┐ ┌──▼────┐ ┌──▼────┐
   │Empath1│ │Empath2│ │Empath3│
   └───────┘ └───────┘ └───────┘
```

**HAProxy Example:**

```
frontend smtp
    bind *:25
    mode tcp
    default_backend empath_cluster

backend empath_cluster
    mode tcp
    balance roundrobin
    option tcp-check
    server empath1 10.0.1.10:25 check
    server empath2 10.0.1.11:25 check
    server empath3 10.0.1.12:25 check
```

---

## Additional Resources

- **SECURITY.md:** Security features, threat model, and best practices
- **CLAUDE.md:** Development guide with architecture details
- **docker/README.md:** Docker development environment guide
- **examples/config/README.md:** Configuration documentation
- **GitHub Issues:** https://github.com/Pyxxilated-Studios/empath/issues

---

**Document Version:** 1.0
**Last Updated:** 2025-11-16
**Maintainer:** Empath MTA Project
