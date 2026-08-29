variable "domain_name" {
  description = "Root domain for Mainstay API endpoints"
  type        = string
  default     = "mainstay.io"
}

variable "regions" {
  description = "Map of region codes to deploy into"
  type        = map(string)
  default = {
    "us-east-1"      = "us-east-1"
    "eu-west-1"      = "eu-west-1"
    "ap-southeast-1" = "ap-southeast-1"
  }
}

variable "instance_type" {
  description = "EC2 instance type for API servers"
  type        = string
  default     = "t3.medium"
}

variable "key_name" {
  description = "SSH key pair name for EC2 instances"
  type        = string
  default     = "mainstay-deployer"
}

variable "ami_ids" {
  description = "AMI IDs per region (Amazon Linux 2023)"
  type        = map(string)
}

variable "vpc_cidrs" {
  description = "VPC CIDR blocks per region"
  type        = map(string)
  default = {
    "us-east-1"      = "10.0.0.0/16"
    "eu-west-1"      = "10.1.0.0/16"
    "ap-southeast-1" = "10.2.0.0/16"
  }
}

variable "acm_certificate_arns" {
  description = "ACM certificate ARNs per region for HTTPS"
  type        = map(string)
}

variable "rpc_urls" {
  description = "Stellar RPC URLs per region"
  type        = map(string)
  default = {
    "us-east-1"      = "https://soroban-mainnet-us-east.stellar.org"
    "eu-west-1"      = "https://soroban-mainnet-eu-west.stellar.org"
    "ap-southeast-1" = "https://soroban-mainnet-ap-se.stellar.org"
  }
}

variable "network_passphrase" {
  description = "Stellar network passphrase"
  type        = string
  default     = "Public Global Stellar Network ; September 2015"
}

# ── SQS queues for cross-region cache invalidation ──────────
resource "aws_sqs_queue" "cache_invalidation" {
  for_each = var.regions

  name                       = "mainstay-cache-invalidation-${each.key}"
  message_retention_seconds  = 86400
  receive_wait_time_seconds  = 20
  visibility_timeout_seconds = 60

  tags = {
    Project = "Mainstay"
    Region  = each.key
  }
}
