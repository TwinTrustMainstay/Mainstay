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
  description = "AMI ID (Amazon Linux 2023)"
  type        = string
}

variable "key_name" {
  description = "SSH key pair name"
  type        = string
}

variable "acm_certificate_arn" {
  description = "ACM certificate ARN for HTTPS"
  type        = string
}

variable "rpc_url" {
  description = "Stellar RPC URL for this region"
  type        = string
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

# Data source for AZs in the region
data "aws_availability_zones" "available" {
  state = "available"
}
