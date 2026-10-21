#!/bin/bash
set -euo pipefail

REGION="${region}"
RPC_URL="${rpc_url}"
NETWORK="${network}"
VAULT_ADDR="${vault_address}"
VAULT_AWS_ROLE="${vault_aws_role}"
VAULT_SECRET_PATH="${vault_secret_path}"

echo "Bootstrapping Mainstay API server in $REGION"
echo "RPC: $RPC_URL"

# ── System updates ─────────────────────────────────────────
yum update -y
yum install -y docker git nginx jq amazon-cloudwatch-agent vault

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

# #1815: fail the bootstrap if the agent isn't actually running, rather than
# silently shipping no logs or metrics. Give it a few seconds to come up.
for i in $(seq 1 10); do
  if systemctl is-active --quiet amazon-cloudwatch-agent; then
    echo "amazon-cloudwatch-agent is running"
    break
  fi
  if [ "$i" -eq 10 ]; then
    echo "FATAL: amazon-cloudwatch-agent failed to start" >&2
    systemctl status amazon-cloudwatch-agent --no-pager >&2 || true
    exit 1
  fi
  sleep 2
done

# Authenticate with Vault using the instance IAM role. Secrets are rendered
# locally with restrictive permissions and never enter Terraform state, user
# data, Docker arguments, or proxy logs.
mkdir -p /etc/vault.d /run/mainstay
cat > /etc/vault.d/mainstay-agent.hcl <<VAULT
pid_file = "/run/vault-agent.pid"
vault {
  address = "${VAULT_ADDR}"
}
auto_auth {
  method "aws" {
    mount_path = "auth/aws"
    config = {
      type = "iam"
      role = "${VAULT_AWS_ROLE}"
    }
  }
  sink "file" {
    config = { path = "/run/mainstay/vault-token" }
  }
}
template {
  source      = "/etc/vault.d/mainstay.env.tpl"
  destination = "/run/mainstay/mainstay.env"
  perms       = "0600"
}
VAULT
cat > /etc/vault.d/mainstay.env.tpl <<'VAULT_TEMPLATE'
{{- with secret "${VAULT_SECRET_PATH}" }}
STELLAR_RPC_URL={{ .Data.data.stellar_rpc_url }}
THIRD_PARTY_API_KEY={{ .Data.data.third_party_api_key }}
{{- end }}
VAULT_TEMPLATE
vault agent -config=/etc/vault.d/mainstay-agent.hcl >/var/log/vault-agent.log 2>&1 &

# ── Wait for Vault to render secrets ───────────────────────
# The container must not start until the Vault agent has authenticated and
# rendered the env file, otherwise it would run without its secrets.
ENV_FILE=/run/mainstay/mainstay.env
ENV_FILE_TIMEOUT=120
elapsed=0
until [ -s "$ENV_FILE" ]; do
  if [ "$elapsed" -ge "$ENV_FILE_TIMEOUT" ]; then
    echo "ERROR: timed out after $${ENV_FILE_TIMEOUT}s waiting for Vault to render $ENV_FILE" >&2
    echo "Check /var/log/vault-agent.log for authentication or template errors." >&2
    exit 1
  fi
  sleep 2
  elapsed=$((elapsed + 2))
done
echo "Vault rendered $ENV_FILE after $${elapsed}s"

# ── Pull API server container ──────────────────────────────
# The image reference is pinned to an immutable tag/digest supplied by
# Terraform so every instance runs the same reviewed build.
IMAGE="ghcr.io/mainstay/api-server:${image_tag}"
docker pull "$IMAGE"

# Verify the image signature before running it.
if command -v cosign >/dev/null 2>&1; then
  cosign verify "$IMAGE" \
    --certificate-identity-regexp '^https://github.com/mainstay/.*$' \
    --certificate-oidc-issuer https://token.actions.githubusercontent.com \
    >/var/log/cosign-verify.log 2>&1 || {
      echo "ERROR: cosign signature verification failed for $IMAGE" >&2
      exit 1
    }
else
  echo "WARNING: cosign not installed; skipping image signature verification" >&2
fi

docker run -d \
  --name mainstay-api \
  --restart always \
  -p 127.0.0.1:8080:8080 \
  -e STELLAR_RPC_URL="$RPC_URL" \
  -e STELLAR_NETWORK_PASSPHRASE="$NETWORK" \
  -e DEPLOY_REGION="$REGION" \
  -e DSAR_TABLE_NAME="${dsar_table_name}" \
  -e DSAR_QUEUE_URL="${dsar_queue_url}" \
  -e RUST_LOG=info \
  --env-file "$ENV_FILE" \
  --memory="512m" \
  --cpus="1" \
  "$IMAGE"

# ── Nginx reverse proxy with rate limiting ─────────────────

# Rate limiting zones
#  - ip_limit:  100 req/min per IP
#  - key_limit: 1000 req/day ≈ 17 req/min per API key
cat > /etc/nginx/conf.d/mainstay-api.conf <<'NGINX'
map $http_x_request_id $mainstay_request_id {
    default $http_x_request_id;
    ""      $request_id;
}

map $http_origin $cors_origin {
    default "";
%{ for origin in allowed_origins ~}
    "${origin}" "${origin}";
%{ endfor ~}
}

proxy_cache_path /var/cache/nginx/mainstay-api
                 levels=1:2
                 keys_zone=mainstay_api:10m
                 max_size=256m
                 inactive=60m
                 use_temp_path=off;

# Trust the ALB as the source of the real client address. Only the ALB
# subnets may set X-Forwarded-For; anything else keeps its socket address.
%{ for cidr in alb_subnet_cidrs ~}
set_real_ip_from ${cidr};
%{ endfor ~}
real_ip_header X-Forwarded-For;
real_ip_recursive on;

# Key the per-IP limit on the resolved client address ($remote_addr after
# realip processing), not the ALB node address.
limit_req_zone $remote_addr zone=ip_limit:10m rate=100r/m;
limit_req_zone $http_x_api_key zone=key_limit:10m rate=17r/m;

log_format ratelimit '$remote_addr [$time_local] request_id=$mainstay_request_id "$request_method $uri" $status '
                     'limit_req=$limit_req_status api_key="$http_x_api_key"';

upstream api_backend {
    least_conn;
    server 127.0.0.1:8080 max_fails=3 fail_timeout=30s;
    keepalive 32;
}

server {
    # #1812: this is the ALB's target (port 80). The ALB terminates TLS
    # with the real ACM certificate and forwards plaintext HTTP to nginx
    # inside the VPC; the instance's security group only accepts traffic
    # from the ALB, so there is no direct-internet path to this listener.
    listen 80;
    server_name _;

    add_header Strict-Transport-Security "max-age=31536000; includeSubDomains" always;
    add_header X-Content-Type-Options "nosniff" always;
    add_header X-Frame-Options "DENY" always;
    add_header Referrer-Policy "no-referrer" always;
    add_header Permissions-Policy "camera=(), microphone=(), geolocation=()" always;

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
        proxy_set_header X-Request-ID $mainstay_request_id;
        add_header X-Request-ID $mainstay_request_id always;

        proxy_cache mainstay_api;
        proxy_cache_methods GET HEAD;
        proxy_cache_key "$scheme$request_method$host$request_uri$http_x_api_key";
        proxy_cache_bypass $http_authorization;
        proxy_no_cache $http_authorization $upstream_http_set_cookie;
        proxy_cache_valid 200 5s;
        proxy_cache_lock on;
        proxy_cache_use_stale error timeout updating;
        add_header X-Cache-Status $upstream_cache_status always;

        add_header Strict-Transport-Security "max-age=31536000; includeSubDomains" always;
        add_header X-Content-Type-Options "nosniff" always;
        add_header X-Frame-Options "DENY" always;
        add_header Referrer-Policy "no-referrer" always;
        add_header Permissions-Policy "camera=(), microphone=(), geolocation=()" always;
        add_header Access-Control-Allow-Origin $cors_origin always;
        add_header Access-Control-Allow-Credentials "true" always;
        add_header Access-Control-Allow-Methods "GET, POST, PUT, PATCH, DELETE, OPTIONS" always;
        add_header Access-Control-Allow-Headers "Authorization, Content-Type, X-API-Key" always;
        add_header Vary "Origin" always;

        if ($request_method = OPTIONS) {
            return 204;
        }

        proxy_read_timeout 30s;
        proxy_connect_timeout 5s;
    }

    location /health {
        proxy_pass http://api_backend;
        access_log off;
    }

    # /metrics is intentionally not served on the public listener.
    location /metrics {
        return 404;
    }
}

# Private metrics listener. Bound to the internal interface only and
# reachable solely from the Prometheus host; never exposed via the ALB.
server {
    listen ${metrics_private_ip}:${metrics_port};
    server_name _;

    access_log /var/log/nginx/metrics-access.log;
    error_log  /var/log/nginx/metrics-error.log warn;

    location /metrics {
        allow 127.0.0.1;
%{ for cidr in prometheus_allowed_cidrs ~}
        allow ${cidr};
%{ endfor ~}
        deny all;
        proxy_pass http://api_backend;
    }

    location / {
        return 404;
    }
}
NGINX

# #1812: nginx no longer terminates TLS itself — the ALB does, with the
# real ACM certificate — so the self-signed placeholder cert this instance
# used to generate for its own :443 listener is no longer needed.

systemctl enable nginx
systemctl restart nginx

echo "Mainstay API server bootstrap complete in $REGION"
