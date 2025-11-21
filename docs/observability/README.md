# Empath MTA - Observability and Alerting

This directory contains production alerting rules, runbooks, and SLO definitions for Empath MTA.

## Contents

- **[prometheus-alerts.yml](prometheus-alerts.yml)** - 12 Prometheus alerting rules (5 Critical, 7 Warning)
- **[alertmanager.yml](alertmanager.yml)** - AlertManager routing and receiver configuration
- **[RUNBOOKS.md](RUNBOOKS.md)** - Detailed remediation procedures for each alert

## Quick Start

### Local Development Testing

The Docker Compose stack includes Prometheus and can load these alert rules:

```bash
# Start observability stack
just docker-up

# Verify Prometheus loaded alerts
open http://localhost:9090/alerts

# View Grafana dashboards
open http://localhost:3000  # (admin/admin)
```

### Production Deployment

1. **Deploy Prometheus with alert rules:**
   ```yaml
   # prometheus.yml
   rule_files:
     - "prometheus-alerts.yml"
   ```

2. **Deploy AlertManager:**
   ```bash
   alertmanager --config.file=alertmanager.yml
   ```

3. **Configure receivers in alertmanager.yml:**
   - Update PagerDuty service key
   - Update Slack webhook URL
   - Configure SMTP settings

4. **Deploy node_exporter (required for SpoolDiskSpaceLow alert):**
   ```bash
   node_exporter --collector.filesystem
   ```

---

## SLO Definitions

### Service Level Objectives (SLOs)

Empath MTA defines the following production SLOs:

#### 1. Delivery Success Rate

**Objective**: 99.5% of messages delivered successfully

**Measurement**:
```promql
empath_delivery_success_rate >= 0.995
```

**Calculation**:
- **Success**: Messages delivered with 2xx SMTP response
- **Failure**: Messages failed with 5xx SMTP response OR exhausted retry attempts
- **Excluded**: Messages still in retry queue (temporary failures)

**Error Budget**:
- **Monthly**: 0.5% = ~216 minutes of degraded service (30-day month)
- **Daily**: 0.5% = ~7 minutes of degraded service

**Alerts**:
- **Critical**: Success rate < 95% for 5 minutes (`DeliverySuccessRateLow`)
- **Warning**: Error rate > 5% for 15 minutes (`DeliveryErrorRateElevated`)

**Rationale**: 99.5% aligns with industry standards for email delivery services. Allows for transient failures (recipient servers down, network issues) while maintaining high reliability.

---

#### 2. Delivery Latency

**Objective**: p95 queue age < 5 minutes

**Measurement**:
```promql
histogram_quantile(0.95, rate(empath_delivery_queue_age_seconds_bucket[5m])) < 300
```

**Calculation**:
- **Queue age**: Time from message reception (SMTP DATA complete) to delivery attempt
- **p95**: 95th percentile of all messages

**Error Budget**:
- **Acceptable**: 5% of messages may exceed 5 minutes
- **Typical**: p50 < 30 seconds, p99 < 15 minutes

**Alerts**:
- **Warning**: p95 queue age > 10 minutes for 10 minutes (`DeliveryLatencyHigh`)
- **Warning**: Oldest message > 1 hour (`OldestMessageAgeHigh`)

**Rationale**: Email users expect near-instant delivery for most messages. 5-minute p95 allows for retry delays, rate limiting, and DNS lookups while maintaining responsiveness.

---

#### 3. System Availability

**Objective**: 99.9% uptime (SMTP listener accepting connections)

**Measurement**:
```promql
rate(empath_smtp_connections_total[5m]) > 0 OR empath_smtp_connections_active > 0
```

**Error Budget**:
- **Monthly**: 0.1% = ~43 minutes of downtime (30-day month)
- **Daily**: 0.1% = ~1.4 minutes of downtime

**Alerts**:
- **Critical**: No connections for 3 minutes (`SMTPListenerDown`)

**Rationale**: Email infrastructure requires high availability. 99.9% allows for planned maintenance windows and brief outages while minimizing user impact.

---

### SLO Measurement and Reporting

**Compliance Dashboard**:
- Grafana dashboard "Empath MTA - SLO Compliance"
- Shows real-time SLO adherence and error budget burn rate

**Weekly SLO Report**:
```promql
# Delivery Success Rate (7-day average)
avg_over_time(empath_delivery_success_rate[7d])

# Delivery Latency p95 (7-day)
histogram_quantile(0.95, rate(empath_delivery_queue_age_seconds_bucket[7d]))

# Availability (7-day uptime %)
100 * (1 - (sum(up{job="empath-mta"} == 0) / count(up{job="empath-mta"})))
```

---

## Alert Severity Levels

### Critical (severity: critical)

**Action**: **Page immediately** via PagerDuty/phone

**Response Time**: Within 15 minutes

**Characteristics**:
- User-facing impact (service degradation or outage)
- Potential data loss
- SLO violation or imminent violation

**Escalation**: If not resolved within 30 minutes, escalate to senior engineer

**Critical Alerts** (5):
1. DeliverySuccessRateLow - Delivery success rate < 95%
2. QueueBacklogCritical - Queue > 10,000 pending messages
3. SMTPListenerDown - No SMTP connections for 3 minutes
4. CircuitBreakerStormDetected - 5+ domains tripped in 5 minutes
5. SpoolDiskSpaceLow - Spool disk < 10% free

---

### Warning (severity: warning)

**Action**: **Create ticket** for investigation during business hours

**Response Time**: Within 4 hours (business hours)

**Characteristics**:
- Early warning of potential issue
- Isolated impact (single domain or component)
- Performance degradation within acceptable bounds

**Escalation**: If recurring or trending toward critical, escalate to on-call

**Warning Alerts** (7):
1. DeliveryLatencyHigh - p95 queue age > 10 minutes
2. OldestMessageAgeHigh - Oldest message > 1 hour
3. DnsCacheHitRateLow - DNS cache hit rate < 70%
4. RateLimitingExcessive - > 100 rate limit delays/min per domain
5. DeliveryErrorRateElevated - Error rate > 5%
6. QueueSizeGrowing - Queue growing > 10 msgs/min
7. CircuitBreakerOpen - Single domain circuit breaker open > 10 minutes

---

## Alert Philosophy

### Design Principles

1. **Actionable**: Every alert must have clear remediation steps (see RUNBOOKS.md)
2. **Symptom-based**: Alert on user impact, not implementation details
3. **Severity-appropriate**: Critical = page, Warning = ticket
4. **Context-rich**: Alerts include runbook links, dashboard links, and descriptions
5. **Noise-minimizing**: Inhibition rules prevent alert storms

### Alert Naming Convention

- **DeliverySuccessRateLow**: Component + Metric + Condition
- Consistent naming aids in correlation and automated handling

### Inhibition Strategy

Warnings are inhibited when related critical alerts fire:
- `DeliveryErrorRateElevated` silenced when `DeliverySuccessRateLow` fires
- `QueueSizeGrowing` silenced when `QueueBacklogCritical` fires
- `CircuitBreakerOpen` silenced when `CircuitBreakerStormDetected` fires

### Tuning Guidance

**If alerts are too noisy:**
- Increase `for` duration (e.g., 5m → 10m)
- Increase thresholds (e.g., success_rate < 0.95 → < 0.90)
- Add inhibition rules for known correlations

**If alerts are too quiet:**
- Decrease `for` duration
- Add additional warning-level alerts for early detection
- Lower thresholds

**Golden rule**: Tune for signal-to-noise ratio. Every page should require action.

---

## Metrics Reference

### Delivery Metrics

| Metric | Type | Description | Alert Usage |
|--------|------|-------------|-------------|
| `empath.delivery.success_rate` | Gauge | Pre-calculated delivery success rate (0-1) | DeliverySuccessRateLow |
| `empath.delivery.error_rate` | Gauge | Pre-calculated delivery error rate (0-1) | DeliveryErrorRateElevated |
| `empath.delivery.queue.size{status}` | Gauge | Queue size by status (pending/retry/failed) | QueueBacklogCritical |
| `empath.delivery.queue.age.seconds` | Histogram | Message queue age distribution | DeliveryLatencyHigh |
| `empath.delivery.queue.oldest.seconds` | Gauge | Age of oldest message in queue | OldestMessageAgeHigh |
| `empath.delivery.circuit_breaker.trips.total{domain}` | Counter | Circuit breaker trips per domain | CircuitBreakerStormDetected |
| `empath.delivery.circuit_breaker.state{domain}` | Gauge | Circuit breaker state (0=Closed, 1=Open, 2=HalfOpen) | CircuitBreakerOpen |
| `empath.delivery.rate_limited.total{domain}` | Counter | Rate limit delays per domain | RateLimitingExcessive |

### SMTP Metrics

| Metric | Type | Description | Alert Usage |
|--------|------|-------------|-------------|
| `empath.smtp.connections.total` | Counter | Total SMTP connections accepted | SMTPListenerDown |
| `empath.smtp.connections.active` | UpDownCounter | Currently active SMTP connections | SMTPListenerDown |
| `empath.smtp.messages.received.total` | Counter | Messages received via SMTP | Queue growth analysis |

### DNS Metrics

| Metric | Type | Description | Alert Usage |
|--------|------|-------------|-------------|
| `empath.dns.cache.hits.total` | Counter | DNS cache hits | DnsCacheHitRateLow |
| `empath.dns.cache.misses.total` | Counter | DNS cache misses | DnsCacheHitRateLow |
| `empath.dns.lookup.duration.seconds{query_type}` | Histogram | DNS lookup duration | Performance monitoring |

### System Metrics (via node_exporter)

| Metric | Type | Description | Alert Usage |
|--------|------|-------------|-------------|
| `node_filesystem_avail_bytes{mountpoint}` | Gauge | Available filesystem bytes | SpoolDiskSpaceLow |
| `node_filesystem_size_bytes{mountpoint}` | Gauge | Total filesystem bytes | SpoolDiskSpaceLow |

**Note**: `node_exporter` must be deployed for filesystem alerts.

---

## Integration Guide

### Prerequisites

- Prometheus 2.x or later
- AlertManager 0.20 or later (if using alerting)
- Grafana 8.x or later (for dashboards)
- node_exporter (for disk space alerts)

### Step 1: Configure Prometheus

**Add alert rules:**
```yaml
# prometheus.yml
rule_files:
  - "prometheus-alerts.yml"

alerting:
  alertmanagers:
    - static_configs:
        - targets: ['alertmanager:9093']
```

**Validate configuration:**
```bash
promtool check config prometheus.yml
promtool check rules prometheus-alerts.yml
```

**Reload Prometheus:**
```bash
curl -X POST http://localhost:9090/-/reload
# Or: systemctl reload prometheus
```

**Verify alerts loaded:**
```bash
curl http://localhost:9090/api/v1/rules | jq '.data.groups[] | select(.name == "empath_critical")'
```

### Step 2: Configure AlertManager

**Deploy configuration:**
```bash
# Copy alertmanager.yml to /etc/alertmanager/
cp alertmanager.yml /etc/alertmanager/

# Update receiver configurations (see CUSTOMIZATION GUIDE in alertmanager.yml)
vi /etc/alertmanager/alertmanager.yml
```

**Validate configuration:**
```bash
amtool check-config /etc/alertmanager/alertmanager.yml
```

**Reload AlertManager:**
```bash
curl -X POST http://localhost:9093/-/reload
# Or: systemctl reload alertmanager
```

### Step 3: Configure Receivers

**PagerDuty:**
1. Create Prometheus integration in PagerDuty
2. Copy integration key to `pagerduty_configs.service_key`
3. Test: Silence alert in Prometheus, wait for page

**Slack:**
1. Create incoming webhook: https://api.slack.com/messaging/webhooks
2. Copy webhook URL to `slack_configs.api_url`
3. Test: `amtool alert add alertname=test severity=warning`

**Email:**
1. Configure SMTP settings in `global` section
2. Update receiver email addresses
3. Test: Send test alert via `amtool`

### Step 4: Deploy node_exporter

**Required for SpoolDiskSpaceLow alert:**
```bash
# Install
apt-get install prometheus-node-exporter  # Debian/Ubuntu
yum install node_exporter                  # RHEL/CentOS

# Configure Prometheus to scrape
# prometheus.yml:
scrape_configs:
  - job_name: 'node'
    static_configs:
      - targets: ['localhost:9100']
```

**Verify metrics:**
```bash
curl http://localhost:9100/metrics | grep node_filesystem
```

### Step 5: Verify End-to-End

**1. Check Prometheus is scraping Empath metrics:**
```bash
curl http://localhost:9090/api/v1/query?query=empath_delivery_success_rate
```

**2. Check alerts are loaded:**
```bash
open http://localhost:9090/alerts
```

**3. Fire test alert:**
```bash
# Temporarily set success rate threshold to 100% in prometheus-alerts.yml
# This will cause DeliverySuccessRateLow to fire
# Verify alert appears in AlertManager and triggers notification
```

**4. Verify runbook links work:**
- Click alert in Prometheus UI
- Follow runbook_url annotation
- Verify RUNBOOKS.md loads correctly

---

## Customization

### Adjusting Thresholds

**Delivery Success Rate:**
```yaml
# Increase tolerance for temporary failures
expr: empath_delivery_success_rate < 0.90  # Was 0.95
```

**Queue Backlog:**
```yaml
# Adjust for higher-volume deployments
expr: empath_delivery_queue_size{status="pending"} > 50000  # Was 10000
```

**Delivery Latency:**
```yaml
# Stricter SLO for low-latency requirements
expr: histogram_quantile(0.95, rate(empath_delivery_queue_age_seconds_bucket[5m])) > 300  # Was 600
```

### Adding Custom Alerts

**Template:**
```yaml
- alert: CustomAlertName
  expr: <promql expression>
  for: <duration>
  labels:
    severity: critical|warning
    component: smtp|delivery|dns|spool
  annotations:
    summary: "Short description ({{ $value }})"
    description: "Detailed description with context"
    runbook_url: "https://github.com/yourorg/empath/blob/main/docs/observability/RUNBOOKS.md#customalertname"
    dashboard_url: "http://grafana:3000/d/empath-component"
```

**Add to appropriate group:**
- Critical alerts → `empath_critical` group
- Warning alerts → `empath_warning` group

**Document remediation:**
- Add section to RUNBOOKS.md
- Include diagnostic steps and remediation procedures

### Domain-Specific Alerts

**Example: Alert on specific high-value domain failures:**
```yaml
- alert: GmailDeliveryFailing
  expr: |
    rate(empath_delivery_attempts_total{status="failed",domain=~".*gmail.com"}[5m]) > 10
  for: 5m
  labels:
    severity: warning
    component: delivery
  annotations:
    summary: "Gmail deliveries failing"
    description: "High failure rate for Gmail deliveries"
```

---

## Operational Procedures

### Regular Maintenance

**Weekly:**
- Review SLO compliance dashboard
- Check for alerts trending toward critical
- Review long-term queue age trends

**Monthly:**
- Review alert thresholds (too noisy or too quiet?)
- Update runbooks based on incident learnings
- Test AlertManager integrations (PagerDuty, Slack)

**Quarterly:**
- Review and update SLOs based on business requirements
- Conduct alert fire drills
- Update escalation contacts

### Incident Response

1. **Alert fires** → Notification sent via AlertManager
2. **On-call acknowledges** → Open runbook link
3. **Follow diagnostic steps** → Identify root cause
4. **Execute remediation** → Fix issue
5. **Verify resolution** → Alert auto-resolves
6. **Document learnings** → Update runbook if needed

### Troubleshooting Alerts

**Alert not firing when it should:**
- Check Prometheus is scraping metrics: `up{job="empath-mta"}`
- Verify expr syntax: Test in Prometheus UI
- Check `for` duration hasn't suppressed alert

**Alert firing incorrectly:**
- Review metric values in Prometheus
- Adjust threshold or `for` duration
- Add inhibition rule if correlated with other alert

**Notification not received:**
- Check AlertManager is running: `systemctl status alertmanager`
- Verify receiver configuration: `amtool config routes show`
- Check AlertManager logs: `journalctl -u alertmanager`
- Test receiver: `amtool alert add alertname=test`

---

## Resources

- **Prometheus Documentation**: https://prometheus.io/docs/
- **AlertManager Documentation**: https://prometheus.io/docs/alerting/latest/
- **Empath Metrics**: See `empath-metrics/src/` for metric definitions
- **Empath Configuration**: See `empath.config.ron` for tunable parameters
- **Support**: File issues at https://github.com/yourorg/empath/issues

---

## Changelog

- **2025-11-21**: Initial release (NEW-05 completion)
  - 12 alerts (5 Critical, 7 Warning)
  - Comprehensive runbooks
  - SLO definitions (99.5% success, p95 < 5min)
  - Integration guide for production deployment
