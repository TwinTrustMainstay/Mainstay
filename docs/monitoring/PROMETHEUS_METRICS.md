# Prometheus Metrics Exporter for Mainstay

This guide documents the Mainstay Prometheus metrics exporter, which periodically queries the Mainstay smart contracts and exposes health and collateral metrics suitable for fleet operator monitoring and alerting.

## Overview

The exporter collects metrics from the Mainstay contracts and exposes them via an HTTP endpoint (`/metrics`) that Prometheus can scrape at regular intervals.

## Running the Exporter

### Prerequisites

- Stellar CLI (`stellar` command) installed and in PATH
- Network connectivity to a Stellar RPC endpoint
- Contract IDs configured via environment variables or `.env` file

### Basic Usage

```bash
./scripts/monitoring/prometheus-exporter.sh
```

The exporter will start an HTTP server on port 9600 (default) and listen for Prometheus scrape requests.

### With Custom Port

```bash
./scripts/monitoring/prometheus-exporter.sh --port 8080
```

### Environment Configuration

Configure these variables in `.env` or export them:

```bash
# Stellar network configuration
STELLAR_NETWORK=testnet                    # or mainnet
STELLAR_RPC_URL=https://rpc.stellar.org   # RPC endpoint URL

# Contract addresses (required)
CONTRACT_ASSET_REGISTRY=CXXXXXXX...
CONTRACT_ENGINEER_REGISTRY=CXXXXXXX...
CONTRACT_LIFECYCLE=CXXXXXXX...

# Exporter settings
EXPORTER_PORT=9600                         # HTTP port for /metrics
SCRAPE_INTERVAL=60                         # Seconds between data collection
```

## Metrics Reference

### Aggregate Metrics

These metrics summarize data across all registered assets:

#### `mainstay_assets_total` (gauge)
Total number of registered assets in the system.

```
mainstay_assets_total 150
```

#### `mainstay_engineers_total` (gauge)
Total number of registered engineers who can perform maintenance.

```
mainstay_engineers_total 42
```

#### `mainstay_avg_collateral_score` (gauge)
Average collateral score (0-100) across all assets with non-zero scores.

```
mainstay_avg_collateral_score 75
```

#### `mainstay_scored_assets_count` (gauge)
Number of assets that have been scored (maintenance performed at least once).

```
mainstay_scored_assets_count 145
```

#### `mainstay_maintenance_records_total` (gauge)
Approximate total count of maintenance records across all assets.

```
mainstay_maintenance_records_total 1250
```

### Per-Asset Metrics

#### `mainstay_collateral_score` (gauge)
Individual collateral score for each asset. Ranges from 0-100.

Labeled by asset ID:

```
mainstay_collateral_score{asset_id="1"} 85
mainstay_collateral_score{asset_id="2"} 60
mainstay_collateral_score{asset_id="3"} 95
```

**Usage**: Monitor asset health, identify low-scoring assets that need service, track trends over time.

#### `mainstay_days_since_last_service` (gauge)
Number of days since the last maintenance service was recorded for each asset.

Special values:
- `-1` = Asset has never been serviced
- `0` = Serviced within the last 24 hours
- `n` = n days since last service

```
mainstay_days_since_last_service{asset_id="1"} 5
mainstay_days_since_last_service{asset_id="2"} -1
mainstay_days_since_last_service{asset_id="3"} 23
```

**Usage**: 
- Identify overdue maintenance (days_since > expected_interval)
- Find assets that have never been serviced
- Schedule preventive maintenance

### System Metrics

#### `mainstay_metrics_collection_seconds` (gauge)
Unix timestamp of the last successful metrics collection.

```
mainstay_metrics_collection_seconds 1726939145
```

#### `mainstay_exporter_up` (gauge)
Binary indicator: 1 = exporter is running, 0 = error or offline.

```
mainstay_exporter_up 1
```

## Prometheus Configuration

Add this to your `prometheus.yml` config to scrape Mainstay metrics:

```yaml
scrape_configs:
  - job_name: 'mainstay'
    scrape_interval: 60s          # Match SCRAPE_INTERVAL
    scrape_timeout: 10s           # Time out individual scrapes
    static_configs:
      - targets: ['localhost:9600']
```

## Grafana Dashboard Setup

### Creating Alerts

Example Prometheus alert rules for monitoring asset health:

```yaml
groups:
  - name: mainstay_alerts
    rules:
      # Alert when an asset's score drops below 40 (poor health)
      - alert: LowCollateralScore
        expr: mainstay_collateral_score < 40
        for: 1h
        annotations:
          summary: "Asset {{ $labels.asset_id }} has low collateral score"
          description: "Score is {{ $value }}"

      # Alert when an asset hasn't been serviced in 60+ days
      - alert: OverdueService
        expr: mainstay_days_since_last_service{asset_id!=""}  > 60
        for: 1d
        annotations:
          summary: "Asset {{ $labels.asset_id }} is overdue for service"
          description: "Last serviced {{ $value }} days ago"

      # Alert if exporter stops collecting metrics
      - alert: MainstayExporterDown
        expr: increase(mainstay_metrics_collection_seconds[5m]) == 0
        for: 5m
        annotations:
          summary: "Mainstay metrics exporter is not updating"
```

### Useful Grafana Queries

**Collateral Score Distribution**:
```
histogram_quantile(0.95, mainstay_collateral_score)
```

**Assets by Health Category**:
```
count(mainstay_collateral_score > 80)   # Excellent
count(mainstay_collateral_score <= 80 and mainstay_collateral_score > 60)  # Good
count(mainstay_collateral_score <= 60)  # Poor
```

**Overdue Maintenance Count**:
```
count(mainstay_days_since_last_service > 60)
```

## Performance Considerations

### Scalability

The exporter iterates through all assets to collect per-asset metrics. For large fleets:

- **< 100 assets**: Default scrape interval (60s) is fine
- **100-500 assets**: Increase `SCRAPE_INTERVAL` to 120-180s
- **500+ assets**: Consider:
  - Increasing interval to 300-600s (5-10 minutes)
  - Implementing contract-level metric aggregation views
  - Using sampling or caching strategies
  - Filtering to a subset of critical assets

### Cost Optimization

- Adjust `SCRAPE_INTERVAL` based on monitoring needs (lower = more data, higher cost)
- Cache frequently-requested metrics in an intermediate layer
- Use contract batch query functions (`get_collateral_score_batch`, etc.) where available

## Troubleshooting

### Exporter Not Collecting Metrics

1. Verify environment variables:
   ```bash
   echo $CONTRACT_LIFECYCLE
   echo $STELLAR_NETWORK
   ```

2. Check contract connectivity:
   ```bash
   stellar contract invoke --id $CONTRACT_LIFECYCLE --network $STELLAR_NETWORK \
     --source any -- get_config
   ```

3. Review exporter logs (run in foreground):
   ```bash
   ./scripts/monitoring/prometheus-exporter.sh --port 9600
   ```

### Metrics Not Updating

1. Verify `SCRAPE_INTERVAL` is configured (default 60s)
2. Check that background collection loop is running:
   ```bash
   ps aux | grep prometheus-exporter
   ```
3. Confirm Prometheus can reach the endpoint:
   ```bash
   curl http://localhost:9600/metrics
   ```

### High Latency

- Reduce `SCRAPE_INTERVAL` if you can, or
- Implement contract-level optimizations
- Consider a caching proxy between exporter and Prometheus

## Related Documentation

- [Monitoring Guide](../monitoring-guide.md) - General Mainstay observability
- [Event Indexer Guide](../EVENT_INDEXER.md) - Off-chain event tracking
- [Backup Procedures](../backup-procedures.md) - Data backup procedures
