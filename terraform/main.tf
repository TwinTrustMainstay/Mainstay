terraform {
  required_version = ">= 1.5.0"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }

  backend "s3" {
    bucket         = "mainstay-terraform-state"
    key            = "multi-region/terraform.tfstate"
    region         = "us-east-1"
    encrypt        = true
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

# ── Global DNS ──────────────────────────────────────────────
resource "aws_route53_zone" "mainstay" {
  provider = aws.us_east_1
  name     = var.domain_name

  tags = {
    Project = "Mainstay"
  }
}

# ── Health Checks per Region ────────────────────────────────
resource "aws_route53_health_check" "api_us_east_1" {
  provider = aws.us_east_1

  fqdn              = "api.us-east-1.${var.domain_name}"
  port              = 443
  type              = "HTTPS"
  resource_path     = "/health"
  failure_threshold = 3
  request_interval  = 30

  tags = {
    Name   = "mainstay-hc-us-east-1"
    Region = "us-east-1"
  }
}

resource "aws_route53_health_check" "api_eu_west_1" {
  provider = aws.us_east_1

  fqdn              = "api.eu-west-1.${var.domain_name}"
  port              = 443
  type              = "HTTPS"
  resource_path     = "/health"
  failure_threshold = 3
  request_interval  = 30

  tags = {
    Name   = "mainstay-hc-eu-west-1"
    Region = "eu-west-1"
  }
}

resource "aws_route53_health_check" "api_ap_southeast_1" {
  provider = aws.us_east_1

  fqdn              = "api.ap-southeast-1.${var.domain_name}"
  port              = 443
  type              = "HTTPS"
  resource_path     = "/health"
  failure_threshold = 3
  request_interval  = 30

  tags = {
    Name   = "mainstay-hc-ap-southeast-1"
    Region = "ap-southeast-1"
  }
}

# ── Latency-based DNS records ───────────────────────────────
resource "aws_route53_record" "api_us_east_1" {
  provider       = aws.us_east_1
  zone_id        = aws_route53_zone.mainstay.zone_id
  name           = "api.us-east-1"
  type           = "A"
  set_identifier = "us-east-1"

  alias {
    name                   = module.api_server_us_east_1.alb_dns_name
    zone_id                = module.api_server_us_east_1.alb_zone_id
    evaluate_target_health = true
  }

  latency_routing_policy {
    region = "us-east-1"
  }

  health_check_id = aws_route53_health_check.api_us_east_1.id
}

resource "aws_route53_record" "api_eu_west_1" {
  provider       = aws.us_east_1
  zone_id        = aws_route53_zone.mainstay.zone_id
  name           = "api.eu-west-1"
  type           = "A"
  set_identifier = "eu-west-1"

  alias {
    name                   = module.api_server_eu_west_1.alb_dns_name
    zone_id                = module.api_server_eu_west_1.alb_zone_id
    evaluate_target_health = true
  }

  latency_routing_policy {
    region = "eu-west-1"
  }

  health_check_id = aws_route53_health_check.api_eu_west_1.id
}

resource "aws_route53_record" "api_ap_southeast_1" {
  provider       = aws.us_east_1
  zone_id        = aws_route53_zone.mainstay.zone_id
  name           = "api.ap-southeast-1"
  type           = "A"
  set_identifier = "ap-southeast-1"

  alias {
    name                   = module.api_server_ap_southeast_1.alb_dns_name
    zone_id                = module.api_server_ap_southeast_1.alb_zone_id
    evaluate_target_health = true
  }

  latency_routing_policy {
    region = "ap-southeast-1"
  }

  health_check_id = aws_route53_health_check.api_ap_southeast_1.id
}

# ── SNS topic for cross-region cache invalidation ─────────────
resource "aws_sns_topic" "cache_invalidation" {
  provider = aws.us_east_1
  name     = "mainstay-cache-invalidation"

  tags = {
    Project = "Mainstay"
  }
}

resource "aws_sns_topic_subscription" "cache_invalidation_eu" {
  provider  = aws.eu_west_1
  topic_arn = aws_sns_topic.cache_invalidation.arn
  protocol  = "sqs"
  endpoint  = aws_sqs_queue.cache_invalidation["eu-west-1"].arn
}

resource "aws_sns_topic_subscription" "cache_invalidation_ap" {
  provider  = aws.ap_southeast_1
  topic_arn = aws_sns_topic.cache_invalidation.arn
  protocol  = "sqs"
  endpoint  = aws_sqs_queue.cache_invalidation["ap-southeast-1"].arn
}

# ── Per-region API server modules ───────────────────────────
# Each region uses a separate module block because Terraform
# requires explicit provider aliases (for_each can't dynamically
# select providers).

module "api_server_us_east_1" {
  source = "./modules/api-server"

  region              = "us-east-1"
  instance_type       = var.instance_type
  key_name            = var.key_name
  ami_id              = var.ami_ids["us-east-1"]
  vpc_cidr            = var.vpc_cidrs["us-east-1"]
  acm_certificate_arn = var.acm_certificate_arns["us-east-1"]
  rpc_url             = var.rpc_urls["us-east-1"]
  network_passphrase  = var.network_passphrase

  providers = {
    aws = aws.us_east_1
  }
}

module "api_server_eu_west_1" {
  source = "./modules/api-server"

  region              = "eu-west-1"
  instance_type       = var.instance_type
  key_name            = var.key_name
  ami_id              = var.ami_ids["eu-west-1"]
  vpc_cidr            = var.vpc_cidrs["eu-west-1"]
  acm_certificate_arn = var.acm_certificate_arns["eu-west-1"]
  rpc_url             = var.rpc_urls["eu-west-1"]
  network_passphrase  = var.network_passphrase

  providers = {
    aws = aws.eu_west_1
  }
}

module "api_server_ap_southeast_1" {
  source = "./modules/api-server"

  region              = "ap-southeast-1"
  instance_type       = var.instance_type
  key_name            = var.key_name
  ami_id              = var.ami_ids["ap-southeast-1"]
  vpc_cidr            = var.vpc_cidrs["ap-southeast-1"]
  acm_certificate_arn = var.acm_certificate_arns["ap-southeast-1"]
  rpc_url             = var.rpc_urls["ap-southeast-1"]
  network_passphrase  = var.network_passphrase

  providers = {
    aws = aws.ap_southeast_1
  }
}
