#!/bin/bash
set -euo pipefail

REGION="${region}"
RPC_URL="${rpc_url}"
NETWORK="${network}"

echo "Bootstrapping Mainstay API server in $REGION"
echo "RPC: $RPC_URL"

# ── System updates ─────────────────────────────────────────
yum update -y
yum install -y docker git nginx jq amazon-cloudwatch-agent

# ── Docker ─────────────────────────────────────────────────
systemctl enable docker
systemctl start docker
usermod -aG docker ec2-user

# ── CloudWatch agent ───────────────────────────────────────
cat > /opt/aws/amazon-cloudwatch-agent/etc/amazon-cloudwatch-agent.json <<'CWAGENT'
{
  "logs": {
    "logs_collected": {
      "files": {
        "collect_list": [
          {
            "file_path": "/var/log/nginx/access.log",
            "log_group_name": "/mainstay/nginx/access",
            "log_stream_name": "{instance_id}"
          },
          {
            "file_path": "/var/log/nginx/error.log",
            "log_group_name": "/mainstay/nginx/error",
            "log_stream_name": "{instance_id}"
          }
        ]
      }
    }
  },
  "metrics": {
    "metrics_collected": {
      "mem": { "measurement": ["mem_used_percent"] },
      "disk": { "measurement": ["used_percent"], "resources": ["/"] }
    }
  }
}
CWAGENT
systemctl enable amazon-cloudwatch-agent
systemctl start amazon-cloudwatch-agent

# ── Pull API server container ──────────────────────────────
docker pull ghcr.io/mainstay/api-server:latest

docker run -d \
  --name mainstay-api \
  --restart always \
  -p 8080:8080 \
  -e STELLAR_RPC_URL="$RPC_URL" \
  -e STELLAR_NETWORK_PASSPHRASE="$NETWORK" \
  -e DEPLOY_REGION="$REGION" \
  -e RUST_LOG=info \
  --memory="512m" \
  --cpus="1" \
  ghcr.io/mainstay/api-server:latest

# ── Nginx reverse proxy with rate limiting ─────────────────

# Rate limiting zones
#  - ip_limit:  100 req/min per IP
#  - key_limit: 1000 req/day ≈ 17 req/min per API key
cat > /etc/nginx/conf.d/mainstay-api.conf <<'NGINX'
limit_req_zone $binary_remote_addr zone=ip_limit:10m rate=100r/m;
limit_req_zone $http_x_api_key zone=key_limit:10m rate=17r/m;

log_format ratelimit '$remote_addr [$time_local] "$request" $status '
                     'limit_req=$limit_req_status '
                     'api_key="$http_x_api_key"';

upstream api_backend {
    least_conn;
    server 127.0.0.1:8080 max_fails=3 fail_timeout=30s;
    keepalive 32;
}

server {
    listen 80;
    return 301 https://$host$request_uri;
}

server {
    listen 443 ssl http2;
    server_name _;

    ssl_certificate     /etc/nginx/ssl/cert.pem;
    ssl_certificate_key /etc/nginx/ssl/key.pem;
    ssl_protocols       TLSv1.2 TLSv1.3;
    ssl_ciphers         HIGH:!aNULL:!MD5;

    access_log /var/log/nginx/access.log ratelimit buffer=32k flush=5s;
    error_log  /var/log/nginx/error.log warn;

    error_page 429 = @rate_limited;

    location @rate_limited {
        default_type application/json;
        add_header Retry-After 60 always;
        return 429 '{"error":"rate_limit_exceeded","message":"Too many requests. Retry after 60 seconds.","retry_after_seconds":60}';
    }

    location /api/ {
        limit_req zone=ip_limit burst=20 nodelay;
        limit_req_status 429;
        limit_req zone=key_limit burst=50 nodelay;
        limit_req_status 429;

        proxy_pass http://api_backend;
        proxy_http_version 1.1;
        proxy_set_header Connection "";
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        proxy_read_timeout 30s;
        proxy_connect_timeout 5s;
    }

    location /health {
        proxy_pass http://api_backend;
        access_log off;
    }

    location /metrics {
        allow 127.0.0.1;
        allow 10.0.0.0/8;
        deny all;
        proxy_pass http://api_backend;
    }
}
NGINX

# Self-signed certificate placeholder (replace with real cert via ACM/Let's Encrypt)
mkdir -p /etc/nginx/ssl
openssl req -x509 -nodes -days 30 -newkey rsa:2048 \
  -keyout /etc/nginx/ssl/key.pem \
  -out /etc/nginx/ssl/cert.pem \
  -subj "/CN=api.${REGION}.mainstay.io"

systemctl enable nginx
systemctl restart nginx

echo "Mainstay API server bootstrap complete in $REGION"
