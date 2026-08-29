# Mainstay Multi-Region Deployment Guide

This guide covers deploying Mainstay contracts and supporting infrastructure across multiple AWS regions for redundancy, latency optimization, and disaster recovery.

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                      Route 53 (DNS)                          │
│              Latency-based / Geo-routing                      │
└──────────┬──────────────────────┬───────────────────────────┘
           │                      │
    ┌──────▼──────┐        ┌──────▼──────┐
    │ us-east-1   │        │ eu-west-1   │
    │ (Primary)   │◄──────►│ (Secondary) │
    │             │  sync  │             │
    │ Stellar RPC │        │ Stellar RPC │
    │ API Server  │        │ API Server  │
    │ Monitoring  │        │ Monitoring  │
    └─────────────┘        └─────────────┘
```

## 1. Multi-Region Stellar RPC Node Configuration

### 1.1 RPC Node Deployment

Each region should run its own Stellar RPC node (Horizon + Soroban RPC) to minimize latency for regional clients.

```bash
# On each regional instance, configure Stellar Core:
stellar-core --conf /etc/stellar/stellar-core.cfg

# Example stellar-core.cfg per region:
# us-east-1:
NETWORK_PASSPHRASE="Public Global Stellar Network ; September 2015"
KNOWN_PEERS=["core-us-east.stellar.org"]
HTTP_PORT=11626
PEER_PORT=11625

# eu-west-1:
NETWORK_PASSPHRASE="Public Global Stellar Network ; September 2015"
KNOWN_PEERS=["core-eu-west.stellar.org"]
HTTP_PORT=11626
PEER_PORT=11625
```

### 1.2 RPC Endpoint Configuration

Update `environments.toml` with multi-region entries:

```toml
[mainnet.us-east-1]
rpc_url = "https://soroban-mainnet-us-east.stellar.org"
network_passphrase = "Public Global Stellar Network ; September 2015"
priority = 1

[mainnet.eu-west-1]
rpc_url = "https://soroban-mainnet-eu-west.stellar.org"
network_passphrase = "Public Global Stellar Network ; September 2015"
priority = 2

[mainnet.ap-southeast-1]
rpc_url = "https://soroban-mainnet-ap-se.stellar.org"
network_passphrase = "Public Global Stellar Network ; September 2015"
priority = 3
```

## 2. Terraform Templates for Multi-Region AWS Deployment

### 2.1 Directory Structure

```
terraform/
├── main.tf              # Root module / provider configuration
├── variables.tf         # Shared variables
├── outputs.tf           # Shared outputs
├── modules/
│   └── api-server/
│       ├── main.tf      # EC2, ALB, security groups
│       ├── variables.tf
│       └── outputs.tf
├── us-east-1/
│   ├── terraform.tfvars
│   └── region.tf        # Region-specific resources
├── eu-west-1/
│   ├── terraform.tfvars
│   └── region.tf
└── ap-southeast-1/
    ├── terraform.tfvars
    └── region.tf
```

### 2.2 Root Module (`terraform/main.tf`)

```hcl
terraform {
  required_version = ">= 1.5.0"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }

  backend "s3" {
    bucket = "mainstay-terraform-state"
    key    = "multi-region/terraform.tfstate"
    region = "us-east-1"
    dynamodb_table = "mainstay-terraform-locks"
  }
}

provider "aws" {
  alias  = "us_east_1"
  region = "us-east-1"
}

provider "aws" {
  alias  = "eu_west_1"
  region = "eu-west-1"
}

provider "aws" {
  alias  = "ap_southeast_1"
  region = "ap-southeast-1"
}

# Global DNS (hosted in us-east-1)
resource "aws_route53_zone" "mainstay" {
  provider = aws.us_east_1
  name     = var.domain_name
}

resource "aws_route53_health_check" "regional" {
  for_each = var.regions

  provider = aws.us_east_1

  fqdn              = "api.${each.key}.${var.domain_name}"
  port              = 443
  type              = "HTTPS"
  resource_path     = "/health"
  failure_threshold = 3
  request_interval  = 30

  tags = {
    Name   = "mainstay-health-${each.key}"
    Region = each.key
  }
}
```

### 2.3 API Server Module (`terraform/modules/api-server/main.tf`)

```hcl
variable "region" {
  description = "AWS region"
  type        = string
}

variable "instance_type" {
  description = "EC2 instance type for API server"
  type        = string
  default     = "t3.medium"
}

variable "vpc_cidr" {
  description = "VPC CIDR block"
  type        = string
}

variable "ami_id" {
  description = "AMI ID for the region (Amazon Linux 2023)"
  type        = string
}

variable "key_name" {
  description = "SSH key pair name"
  type        = string
}

# VPC
resource "aws_vpc" "main" {
  cidr_block           = var.vpc_cidr
  enable_dns_hostnames = true
  enable_dns_support   = true

  tags = { Name = "mainstay-vpc-${var.region}" }
}

# Subnets (across 2 AZs for HA)
resource "aws_subnet" "public" {
  count                   = 2
  vpc_id                  = aws_vpc.main.id
  cidr_block              = cidrsubnet(var.vpc_cidr, 8, count.index)
  availability_zone       = data.aws_availability_zones.available.names[count.index]
  map_public_ip_on_launch = true

  tags = { Name = "mainstay-public-${var.region}-${count.index}" }
}

# Security Group for API Server
resource "aws_security_group" "api_server" {
  name        = "mainstay-api-server-${var.region}"
  description = "Security group for Mainstay API servers"
  vpc_id      = aws_vpc.main.id

  ingress {
    from_port   = 443
    to_port     = 443
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
    description = "HTTPS"
  }

  ingress {
    from_port   = 8080
    to_port     = 8080
    protocol    = "tcp"
    cidr_blocks = ["10.0.0.0/8"]
    description = "Internal health checks"
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = { Name = "mainstay-api-sg-${var.region}" }
}

# Application Load Balancer
resource "aws_lb" "api" {
  name               = "mainstay-api-${replace(var.region, "-", "")}"
  internal           = false
  load_balancer_type = "application"
  security_groups    = [aws_security_group.api_server.id]
  subnets            = aws_subnet.public[*].id

  tags = { Name = "mainstay-alb-${var.region}" }
}

# Target Group
resource "aws_lb_target_group" "api" {
  name     = "mainstay-tg-${replace(var.region, "-", "")}"
  port     = 8080
  protocol = "HTTP"
  vpc_id   = aws_vpc.main.id

  health_check {
    path                = "/health"
    interval            = 30
    timeout             = 5
    healthy_threshold   = 3
    unhealthy_threshold = 3
    matcher             = "200"
  }

  tags = { Name = "mainstay-tg-${var.region}" }
}

# HTTPS Listener
resource "aws_lb_listener" "api" {
  load_balancer_arn = aws_lb.api.arn
  port              = 443
  protocol          = "HTTPS"
  ssl_policy        = "ELBSecurityPolicy-TLS13-1-2-2021-06"
  certificate_arn   = var.acm_certificate_arn

  default_action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.api.arn
  }
}

# Auto Scaling Group for API Server
resource "aws_launch_template" "api" {
  name_prefix   = "mainstay-api-${var.region}-"
  image_id      = var.ami_id
  instance_type = var.instance_type
  key_name      = var.key_name

  user_data = base64encode(templatefile("${path.module}/user_data.sh", {
    region        = var.region
    rpc_url       = var.rpc_url
    network       = var.network_passphrase
  }))

  vpc_security_group_ids = [aws_security_group.api_server.id]

  tag_specifications {
    resource_type = "instance"
    tags = { Name = "mainstay-api-${var.region}" }
  }
}

resource "aws_autoscaling_group" "api" {
  name               = "mainstay-asg-${var.region}"
  vpc_zone_identifier = aws_subnet.public[*].id
  min_size           = 2
  max_size           = 6
  desired_capacity   = 2

  launch_template {
    id      = aws_launch_template.api.id
    version = "$Latest"
  }

  target_group_arns = [aws_lb_target_group.api.arn]

  health_check_type         = "ELB"
  health_check_grace_period = 300

  tag {
    key                 = "Name"
    value               = "mainstay-api-${var.region}"
    propagate_at_launch = true
  }
}
```

### 2.4 User Data (`terraform/modules/api-server/user_data.sh`)

```bash
#!/bin/bash
set -euo pipefail

REGION="${region}"
RPC_URL="${rpc_url}"
NETWORK="${network}"

# Install dependencies
yum update -y
yum install -y docker git nginx

# Start Docker
systemctl enable docker
systemctl start docker

# Pull and run Mainstay API container
docker run -d \
  --name mainstay-api \
  --restart always \
  -p 8080:8080 \
  -e STELLAR_RPC_URL="$RPC_URL" \
  -e STELLAR_NETWORK_PASSPHRASE="$NETWORK" \
  -e DEPLOY_REGION="$REGION" \
  ghcr.io/mainstay/api-server:latest

# Configure nginx as reverse proxy with rate limiting
cat > /etc/nginx/conf.d/mainstay.conf <<'NGINX'
# Rate limiting zones
limit_req_zone $binary_remote_addr zone=api_limit:10m rate=100r/m;
limit_req_zone $http_x_api_key zone=key_limit:10m rate=17r/m;

upstream api_backend {
    server 127.0.0.1:8080;
}

server {
    listen 443 ssl;
    server_name api.${region}.mainstay.io;

    ssl_certificate     /etc/nginx/ssl/cert.pem;
    ssl_certificate_key /etc/nginx/ssl/key.pem;

    # Per-IP rate limiting (100 req/min)
    location /api/ {
        limit_req zone=api_limit burst=20 nodelay;
        limit_req_status 429;

        # Per-API-key quota (1000 req/day ≈ 0.7 req/min per key)
        limit_req zone=key_limit burst=50 nodelay;
        limit_req_status 429;

        proxy_pass http://api_backend;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    }

    # Health check endpoint (no rate limiting)
    location /health {
        proxy_pass http://api_backend;
    }

    # Metrics endpoint (internal only)
    location /metrics {
        allow 10.0.0.0/8;
        deny all;
        proxy_pass http://api_backend;
    }
}
NGINX

systemctl enable nginx
systemctl start nginx
```

## 3. Data Sync Strategy (Eventual Consistency)

### 3.1 Overview

Mainstay contracts are deployed on the Stellar network which provides global consensus — all regions read from the same canonical blockchain state. However, local state (caches, analytics databases, event indexes) requires synchronization.

### 3.2 Synchronization Strategy

```
                    ┌──────────────┐
                    │   Stellar    │
                    │   Network    │
                    │  (Global)    │
                    └──────┬───────┘
                           │
              ┌────────────┼────────────┐
              │            │            │
        ┌─────▼─────┐ ┌───▼─────┐ ┌───▼─────┐
        │ us-east-1 │ │eu-west-1│ │ ap-se-1 │
        │  Indexer  │ │Indexer  │ │ Indexer │
        └─────┬─────┘ └───┬─────┘ └───┬─────┘
              │            │            │
        ┌─────▼─────┐ ┌───▼─────┐ ┌───▼─────┐
        │  Local    │ │ Local   │ │ Local   │
        │   Cache   │ │  Cache  │ │  Cache  │
        └───────────┘ └─────────┘ └─────────┘
```

**Key principles:**

1. **Stellar is the source of truth.** All contract state reads go to the Stellar network (via local RPC node). No cross-region state replication needed for on-chain data.

2. **Eventual consistency for off-chain data.** Each region runs an independent indexer that subscribes to Stellar events and populates a local read-optimized database (e.g., PostgreSQL with read replicas).

3. **Cross-region cache invalidation.** When a write occurs in one region, publish a cache invalidation event via SNS/SQS to other regions.

### 3.3 Cache Invalidation Implementation

```python
# Invalidation publisher (called after successful write)
import boto3
import json

sns = boto3.client('sns')
TOPIC_ARN = 'arn:aws:sns:us-east-1:123456789012:mainstay-cache-invalidation'

def invalidate_cache(asset_id: int, region: str):
    message = {
        'type': 'cache_invalidation',
        'asset_id': asset_id,
        'source_region': region,
        'timestamp': int(time.time())
    }
    sns.publish(TopicArn=TOPIC_ARN, Message=json.dumps(message))
```

### 3.4 Stale Read Tolerance

- **Contract state reads**: Always fresh (read directly from Stellar RPC)
- **Analytics/score history**: Eventually consistent (up to 10s lag)
- **Event subscriptions**: Real-time via SSE from each regional RPC node

## 4. Health Checks for Regional Failover

### 4.1 Health Check Endpoints

Each API server must expose:

```
GET /health          → 200 OK (comprehensive)
GET /health/ready    → 200 OK (ready for traffic)
GET /health/live     → 200 OK (process alive)
```

### 4.2 Health Check Implementation

```rust
// Example health check handler for the API server
#[derive(Serialize)]
struct HealthResponse {
    status: String,
    region: String,
    rpc_connected: bool,
    latest_ledger: u32,
    uptime_seconds: u64,
}

async fn health_check() -> Json<HealthResponse> {
    let rpc_status = check_stellar_rpc_connection().await;
    Json(HealthResponse {
        status: if rpc_status { "healthy".into() } else { "degraded".into() },
        region: std::env::var("DEPLOY_REGION").unwrap_or_default(),
        rpc_connected: rpc_status,
        latest_ledger: get_latest_ledger().await.unwrap_or(0),
        uptime_seconds: get_uptime(),
    })
}
```

### 4.3 Route 53 DNS Failover

```hcl
# Latency-based routing with health check failover
resource "aws_route53_record" "api" {
  for_each = var.regions

  provider       = aws.us_east_1
  zone_id        = aws_route53_zone.mainstay.zone_id
  name           = "api.${each.key}"
  type           = "A"
  set_identifier = each.key

  alias {
    name                   = module.api_server[each.key].alb_dns_name
    zone_id                = module.api_server[each.key].alb_zone_id
    evaluate_target_health = true
  }

  latency_routing_policy {
    region = each.key
  }

  health_check_id = aws_route53_health_check.regional[each.key].id
}
```

### 4.4 Failover Procedure

1. **Health check detects failure** in region after 3 consecutive failures (90s)
2. **Route 53 removes unhealthy endpoint** from DNS responses
3. **Traffic shifts** to next-closest healthy region
4. **CloudWatch alarm triggers** → PagerDuty alert to ops team
5. **Auto Scaling Group** attempts to replace unhealthy instances
6. **Manual verification** required before declaring region healthy again

## 5. Deployment Checklist

- [ ] Terraform state bucket and lock table created
- [ ] ACM certificates provisioned in each region
- [ ] Stellar RPC nodes deployed and synced in each region
- [ ] API server AMI built and tested
- [ ] Health check endpoints verified
- [ ] Route 53 health checks passing
- [ ] Cross-region SNS/SQS cache invalidation tested
- [ ] Load test run against multi-region endpoints
- [ ] Failover drill completed (simulate region failure)
- [ ] Monitoring dashboards configured in each region
- [ ] PagerDuty escalation policies set up

## 6. Monitoring and Alerting

### 6.1 Regional Dashboard Metrics

- API request latency (p50, p95, p99)
- RPC node sync status
- Cache hit ratio
- Rate limit hits per region
- Active WebSocket/SSE connections

### 6.2 CloudWatch Alarms

```hcl
resource "aws_cloudwatch_metric_alarm" "api_5xx" {
  alarm_name          = "mainstay-api-5xx-${var.region}"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 2
  metric_name         = "HTTPCode_Target_5XX_Count"
  namespace           = "AWS/ApplicationELB"
  period              = 60
  statistic           = "Sum"
  threshold           = 5
  alarm_description   = "High 5xx error rate in ${var.region}"
}

resource "aws_cloudwatch_metric_alarm" "health_check_failure" {
  alarm_name          = "mainstay-health-failure-${var.region}"
  comparison_operator = "LessThanThreshold"
  evaluation_periods  = 1
  metric_name         = "HealthCheckStatus"
  namespace           = "AWS/Route53"
  period              = 60
  statistic           = "Minimum"
  threshold           = 1
  alarm_description   = "Health check failing in ${var.region}"
}
```
