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
}

variable "acm_certificate_arns" {
  description = "ACM certificate ARNs per region for HTTPS"
  type        = map(string)
}

variable "rpc_urls" {
  description = "Stellar RPC URLs per region"
  type        = map(string)
}

variable "network_passphrase" {
  description = "Stellar network passphrase"
  type        = string
}

variable "allowed_origins" {
  description = "Browser origins allowed to call the API"
  type        = list(string)
  default     = ["https://app.mainstay.io"]
}

variable "log_retention_days" {
  description = "Number of days to retain API access and error logs"
  type        = number
  default     = 30
}

variable "request_retention_seconds" {
  description = "Maximum retention for data-subject request messages"
  type        = number
  default     = 604800
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
