# Mainstay API Rate Limiting

> **Last updated:** 2026-09-25 · **Doc version:** 0.1.0

## Overview

Rate limiting protects Mainstay API servers from abuse and ensures fair resource allocation across all clients. Limits are enforced at the reverse proxy (nginx) layer before requests reach the API server.

## Rate Limiting Policy

### Per-IP Rate Limiting

| Limit | Value | Purpose |
|-------|-------|---------|
| Rate | 100 requests/minute | Prevent single IP from overwhelming the API |
| Burst | 20 requests | Allow short spikes above the base rate |
| Scope | Per client IP (`$remote_addr`, rewritten by `realip`) | Independent limits per client IP |

> **Note:** The API runs behind an AWS ALB, so the TCP peer address nginx sees
> is always an ALB node, not the client. The `realip` module rewrites
> `$remote_addr` from `X-Forwarded-For` for peers inside the trusted ALB
> subnets only (see [Real Client IP Resolution](#real-client-ip-resolution)).
> Rate limiting keys on `$remote_addr` *after* this rewrite, so each client
> gets its own bucket. Keying on `$binary_remote_addr` without `realip` would
> collapse every client into a single global 100 req/min limit.

### Per-API-Key Quota

| Limit | Value | Purpose |
|-------|-------|---------|
| Rate | 1000 requests/day (~0.7 req/min) | Enforce fair usage per API consumer |
| Burst | 50 requests | Allow batch operations |
| Scope | Per API key (`$http_x_api_key`) | Independent limits per API key |

### Limit Exceeded Response

When a client exceeds the rate limit, the API returns:

```
HTTP/1.1 429 Too Many Requests
Content-Type: application/json
Retry-After: 60

{
  "error": "rate_limit_exceeded",
  "message": "Too many requests. Please retry after 60 seconds.",
  "retry_after_seconds": 60
}
```

## Contract-level per-user limits

Maintenance submissions are protected by a rolling-hour limit per engineer.
The administrator can set an individual override with
`update_user_submission_limit(admin, user, limit)`. This limit is checked for
both single and batch submissions, so one user cannot bypass throttling by
using batches. Passing `0` removes the override and returns the user to the
contract-wide `max_submissions_per_hour` default.

## Nginx Configuration

### Real Client IP Resolution

The `realip` module must be configured so that `$remote_addr` reflects the
actual client, not the ALB. Only the ALB subnets are trusted; any
`X-Forwarded-For` value from an untrusted peer is ignored, so clients cannot
spoof their address to dodge rate limits.

```nginx
http {
    # Trust X-Forwarded-For only from the ALB subnets (replace with the
    # actual subnets of your ALB, e.g. from `aws ec2 describe-subnets`).
    set_real_ip_from 10.0.0.0/19;
    set_real_ip_from 10.0.32.0/19;
    set_real_ip_from 10.0.64.0/19;

    # Take the client address from the ALB's X-Forwarded-For header and walk
    # the chain right-to-left, skipping trusted proxies, to find the real
    # client address.
    real_ip_header X-Forwarded-For;
    real_ip_recursive on;
}
```

After this rewrite, `$remote_addr` holds the client address and is used for
both the rate-limit key and the access log.

### Main Configuration (`/etc/nginx/nginx.conf` additions)

```nginx
http {
    # Per-IP rate limiting zone: 10 MB shared memory, 100 req/min.
    # Keyed on $remote_addr, which realip has rewritten to the real client
    # address (see "Real Client IP Resolution" above).
    limit_req_zone $remote_addr zone=ip_limit:10m rate=100r/m;

    # Per-API-key rate limiting zone: 10 MB shared memory, ~0.7 req/min (1000/day)
    # Note: nginx rate is per-second; 1000/day = ~0.0116 req/s
    # For practical enforcement, use 17r/m (~1000/day) with burst
    limit_req_zone $http_x_api_key zone=key_limit:10m rate=17r/m;

    # Rate limit tracking for metrics (no rejection, just logging)
    limit_req_zone $remote_addr zone=ip_track:10m rate=100r/m;
}
```

### Site Configuration (`/etc/nginx/conf.d/mainstay-api.conf`)

```nginx
# Map to extract API key from header or query param
map $http_x_api_key $api_key {
    ""      "anonymous";
    default $http_x_api_key;
}

# Log format with rate limit info for metrics
log_format ratelimit '$remote_addr - $api_key [$time_local] '
                     '"$request" $status $body_bytes_sent '
                     '"$http_referer" "$http_user_agent" '
                     'limit_status=$limit_req_status';

upstream api_backend {
    least_conn;
    server 127.0.0.1:8080 max_fails=3 fail_timeout=30s;
    keepalive 32;
}

server {
    listen 443 ssl http2;
    server_name api.mainstay.io;

    ssl_certificate     /etc/nginx/ssl/fullchain.pem;
    ssl_certificate_key /etc/nginx/ssl/privkey.pem;
    ssl_protocols       TLSv1.2 TLSv1.3;
    ssl_ciphers         HIGH:!aNULL:!MD5;

    # Access log with rate limit data
    access_log /var/log/nginx/mainstay-api-access.log ratelimit buffer=32k flush=5s;
    error_log  /var/log/nginx/mainstay-api-error.log warn;

    # Rate limit exceeded response
    error_page 429 = @rate_limited;
    location @rate_limited {
        default_type application/json;
        add_header Retry-After 60 always;
        return 429 '{"error":"rate_limit_exceeded","message":"Too many requests. Please retry after 60 seconds.","retry_after_seconds":60}';
    }

    # API endpoints with rate limiting
    location /api/ {
        # Tier 1: Per-IP rate limiting (100 req/min + 20 burst)
        limit_req zone=ip_limit burst=20 nodelay;
        limit_req_status 429;

        # Tier 2: Per-API-key quota (17 req/min + 50 burst ≈ ~1000/day)
        limit_req zone=key_limit burst=50 nodelay;
        limit_req_status 429;

        # Dry-run tracking zone (logs but doesn't reject)
        limit_req zone=ip_track burst=200;
        limit_req_status 429;
        limit_req_dry_run on;

        proxy_pass http://api_backend;
        proxy_http_version 1.1;
        proxy_set_header Connection "";
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_set_header X-RateLimit-Remaining $limit_req_status;

        proxy_read_timeout 30s;
        proxy_connect_timeout 5s;
        proxy_send_timeout 30s;
    }

    # Health check endpoint (no rate limiting)
    location /health {
        proxy_pass http://api_backend;
        access_log off;
    }

    # Metrics endpoint (internal only, no rate limiting)
    location /metrics {
        allow 127.0.0.1;
        allow 10.0.0.0/8;
        allow 172.16.0.0/12;
        allow 192.168.0.0/16;
        deny all;
        proxy_pass http://api_backend;
    }
}
```

## Rate Limit Metrics

### Prometheus Metrics Export

Nginx rate limit metrics can be exported to Prometheus using `nginx-prometheus-exporter`:

```bash
# Install nginx-prometheus-exporter
docker run -d \
  --name nginx-exporter \
  --restart always \
  -p 9113:9113 \
  nginx/nginx-prometheus-exporter:latest \
  -nginx.scrape-uri http://127.0.0.1:8080/stub_status
```

### Key Metrics to Track

| Metric | Description |
|--------|-------------|
| `nginx_http_requests_total{status="429"}` | Total rate-limited requests |
| `nginx_http_requests_total` | Total requests per endpoint |
| `nginx_limit_req_delayed_total` | Requests delayed by rate limiting |
| `nginx_limit_req_rejected_total` | Requests rejected (429) |
| Rate limit hits by IP | Top IPs hitting rate limits |
| Rate limit hits by API key | Top API keys exceeding quotas |

### Alerting Rules

```yaml
# Prometheus alerting rules
groups:
  - name: rate_limiting
    rules:
      - alert: HighRateLimitRejectionRate
        expr: |
          rate(nginx_http_requests_total{status="429"}[5m]) 
          / rate(nginx_http_requests_total[5m]) > 0.1
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "More than 10% of requests are being rate limited"

      - alert: RateLimitSpike
        expr: rate(nginx_http_requests_total{status="429"}[5m]) > 50
        for: 2m
        labels:
          severity: critical
        annotations:
          summary: "Rate limit rejections exceeding 50/sec — possible DDoS"
```

## Testing Rate Limits

### Using `ab` (Apache Bench)

```bash
# Test per-IP rate limiting (110 requests — triggers 429 on the 101st+)
ab -n 110 -c 1 https://api.mainstay.io/api/v1/health

# Expected: ~100 successful (200), ~10 rate-limited (429)
```

### Using `wrk`

```bash
# Sustained load test at 2 req/s (within 100 req/min limit)
wrk -t 2 -c 2 -d 60s --rate 2 https://api.mainstay.io/api/v1/assets

# Over-limit test at 3 req/s
wrk -t 2 -c 2 -d 30s --rate 3 https://api.mainstay.io/api/v1/assets
```

### Verifying Separate Buckets per Client (Behind the ALB)

This test proves the `realip` configuration works: two clients behind the same
ALB must get independent rate-limit buckets. If `realip` were missing or
misconfigured, both clients would share one bucket and the second client would
see 429s while the first is still under its limit.

1. Confirm nginx sees the real client address (not the ALB node):

   ```bash
   # From each client host, hit the API and check the access log
   curl -s https://api.mainstay.io/api/v1/health >/dev/null
   tail -n 1 /var/log/nginx/mainstay-api-access.log
   # The leading address must be the client's public IP, not the ALB node.
   ```

2. Run the load test from **two different client hosts** (or two distinct
   egress IPs) at the same time:

   ```bash
   # On client A
   ab -n 110 -c 1 https://api.mainstay.io/api/v1/health

   # On client B, concurrently
   ab -n 110 -c 1 https://api.mainstay.io/api/v1/health
   ```

3. Expected result: **each** client sees ~100 successes and ~10 429s. If the
   buckets were shared, the combined 220 requests would produce ~120 429s
   split across both clients. A quick check of the 429 counts per client IP:

   ```bash
   awk '$9 == 429 {print $1}' /var/log/nginx/mainstay-api-access.log \
     | sort | uniq -c | sort -rn
   ```

   You should see roughly equal counts for both client IPs, each near 10,
   rather than one IP absorbing all rejections.

## Adjusting Limits

Limits can be adjusted at runtime without server restart:

```bash
# Reload nginx with updated config
nginx -t && nginx -s reload

# Or use dynamic API (nginx Plus / OpenResty)
curl -X POST "http://127.0.0.1:8080/api/nginx/limit_req" \
  -d '{"zone":"ip_limit","rate":"200r/m"}'
```
