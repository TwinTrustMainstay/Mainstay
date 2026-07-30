# Monitoring Guide

This guide explains how to set up monitoring for Mainstay smart contracts
using Prometheus and Grafana.  The setup tracks contract health metrics,
asset growth, maintenance activity, and collateral scores.

## Architecture

```
┌────────────┐      ┌──────────────────┐      ┌─────────────┐
│  Stellar   │◄────►│  prometheus-     │      │             │
│  RPC Node  │      │  exporter.sh     │─────►│  Prometheus │
└────────────┘      └──────────────────┘      └──────┬──────┘
                                                     │
                                            ┌────────▼──────┐
                                            │    Grafana     │
                                            │  Dashboard     │
                                            └───────┬────────┘
                                                    │
                                            ┌───────▼────────┐
                                            │   Alertmanager  │
                                            │  (Slack, Pager) │
                                            └────────────────┘
```

## Prerequisites

- A running Stellar RPC node (or access to a public RPC endpoint)
- Prometheus 2.x installed
- Grafana 10.x installed
- Stellar CLI installed and configured
- Bash 4+ with `jq`, `socat` or `python3` for the exporter HTTP server

## Step 1: Configure the Exporter

### 1.1 Environment Variables

Set the required environment variables in `.env`:

```bash
# Stellar connection
STELLAR_NETWORK=mainnet
STELLAR_RPC_URL=https://mainnet.sorobanrpc.com

# Contract addresses (from deployment)
CONTRACT_ASSET_REGISTRY=<your-asset-registry-contract-id>
CONTRACT_ENGINEER_REGISTRY=<your-engineer-registry-contract-id>
CONTRACT_LIFECYCLE=<your-lifecycle-contract-id>

# Exporter configuration
EXPORTER_PORT=9600
SCRAPE_INTERVAL=60
```

### 1.2 Run the Exporter

```bash
# Make the script executable
chmod +x scripts/monitoring/prometheus-exporter.sh

# Start the exporter
./scripts/monitoring/prometheus-exporter.sh
```

The exporter starts an HTTP server on port 9600 (configurable) and serves
Prometheus-style metrics at `http://localhost:9600/metrics`.

### 1.3 Verify the Exporter

```bash
curl http://localhost:9600/metrics
```

Expected output:
```
# HELP mainstay_assets_total Total number of registered assets.
# TYPE mainstay_assets_total gauge
mainstay_assets_total 42

# HELP mainstay_engineers_total Total number of registered engineers.
# TYPE mainstay_engineers_total gauge
mainstay_engineers_total 15

# HELP mainstay_avg_collateral_score Average collateral score.
# TYPE mainstay_avg_collateral_score gauge
mainstay_avg_collateral_score 72

# HELP mainstay_exporter_up Whether the exporter is running.
# TYPE mainstay_exporter_up gauge
mainstay_exporter_up 1
```

## Step 2: Configure Prometheus

### 2.1 Add Scrape Job

Add the following to your `prometheus.yml` configuration file:

```yaml
scrape_configs:
  - job_name: 'mainstay'
    scrape_interval: 60s
    scrape_timeout: 50s
    static_configs:
      - targets: ['localhost:9600']
        labels:
          network: 'mainnet'
          project: 'mainstay'
```

### 2.2 Set Up Alerting Rules

Copy the alerting rules file to your Prometheus rules directory:

```bash
cp scripts/monitoring/alerting-rules.yml /etc/prometheus/rules/mainstay.yml
```

Add to `prometheus.yml`:

```yaml
rule_files:
  - '/etc/prometheus/rules/mainstay.yml'
```

### 2.3 Reload Prometheus

```bash
# Reload configuration
curl -X POST http://localhost:9090/-/reload

# Verify the target is healthy
curl http://localhost:9090/api/v1/targets | jq '.data.activeTargets[] | select(.labels.job=="mainstay")'
```

## Step 3: Import Grafana Dashboard

### 3.1 Import the Dashboard

1. Open Grafana (usually at `http://localhost:3000`).
2. Navigate to **Dashboards → Import**.
3. Upload `scripts/monitoring/grafana-dashboard.json`.
4. Select the Prometheus data source.
5. Click **Import**.

### 3.2 Dashboard Panels

The dashboard includes the following panels:

| Panel | Type | Description |
|-------|------|-------------|
| Asset Growth | Timeseries | Total asset count over time |
| Collateral Score Metrics | Timeseries | Average score + scored asset count |
| Current Average Score | Gauge | Live gauge of current average score (0–100) |
| Total Assets | Stat | Current total asset count |
| Total Maintenance Records | Stat | Current total maintenance records |
| Engineer Growth | Timeseries | Total registered engineers |
| Maintenance Frequency | Bar chart | Maintenance record counts over time |

## Step 4: Configure Alerts

### 4.1 Alert Descriptions

| Alert | Severity | Description |
|-------|----------|-------------|
| `MainstayExporterDown` | Critical | Exporter process is not responding |
| `NoNewAssets` | Warning | No new assets registered in 7 days |
| `AssetsDecreasing` | Critical | Asset count has dropped |
| `ZeroMaintenanceRecords` | Warning | No maintenance records in 24 hours |
| `NoMaintenanceActivity` | Warning | No activity in 30 minutes |
| `HighMaintenanceFailureRate` | Warning | >10% submission failure rate |
| `AvgCollateralScoreDropping` | Warning | Score dropped >20% in 24 hours |
| `NoScoredAssets` | Warning | All scores are zero |
| `EngineerCountDropping` | Warning | Engineer count decreasing |
| `StaleMetrics` | Critical | Metrics not updated in >2 minutes |

### 4.2 Configure Alertmanager

If using Alertmanager, route alerts to your notification channels
(Slack, PagerDuty, email, etc.).  Example:

```yaml
# alertmanager.yml
route:
  receiver: 'default'
  routes:
    - match:
        severity: critical
      receiver: 'pagerduty-critical'
      repeat_interval: 5m
    - match:
        severity: warning
      receiver: 'slack-warnings'

receivers:
  - name: 'pagerduty-critical'
    pagerduty_configs:
      - routing_key: '<pagerduty-integration-key>'
  - name: 'slack-warnings'
    slack_configs:
      - api_url: '<slack-webhook-url>'
        channel: '#mainstay-monitoring'
```

## Step 5: Run as a Systemd Service

Create a systemd service file for the exporter to ensure it starts on boot
and restarts on failure:

```ini
# /etc/systemd/system/mainstay-exporter.service
[Unit]
Description=Mainstay Prometheus Metrics Exporter
After=network.target

[Service]
Type=simple
User=mainstay
WorkingDirectory=/opt/mainstay
EnvironmentFile=/opt/mainstay/.env
ExecStart=/opt/mainstay/scripts/monitoring/prometheus-exporter.sh
Restart=always
RestartSec=10
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
```

Enable and start:

```bash
sudo systemctl daemon-reload
sudo systemctl enable mainstay-exporter
sudo systemctl start mainstay-exporter
sudo systemctl status mainstay-exporter
```

## Metrics Reference

| Metric | Type | Description |
|--------|------|-------------|
| `mainstay_assets_total` | Gauge | Total registered assets |
| `mainstay_engineers_total` | Gauge | Total registered engineers |
| `mainstay_maintenance_records_total` | Gauge | Total maintenance records |
| `mainstay_avg_collateral_score` | Gauge | Average collateral score (0–100) |
| `mainstay_scored_assets_count` | Gauge | Assets with non-zero scores |
| `mainstay_metrics_collection_seconds` | Gauge | Unix timestamp of last collection |
| `mainstay_exporter_up` | Gauge | 1 if exporter is running |

## Troubleshooting

### Exporter fails to start

- Verify `STELLAR_NETWORK` and contract IDs are set correctly.
- Check that Stellar CLI is installed: `stellar --version`.
- Ensure the Stellar RPC endpoint is reachable: `curl $STELLAR_RPC_URL`.

### Metrics show as 0

- The contracts may not have any data yet — register test assets and engineers.
- Check the Stellar CLI output for errors when querying contracts.

### Grafana shows "No data"

- Verify the Prometheus job `mainstay` is scraping successfully.
- Check the metric names match exactly what the exporter outputs.
- Verify time ranges in dashboard queries overlap with actual data.

### Dashboard performance

- The per-asset iteration for `mainstay_maintenance_records_total` can be expensive
  for large numbers of assets.  Increase `SCRAPE_INTERVAL` or use the caching
  variant of the exporter for production deployments with 1,000+ assets.
