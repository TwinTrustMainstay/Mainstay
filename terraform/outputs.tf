output "route53_zone_id" {
  description = "Route53 hosted zone ID"
  value       = aws_route53_zone.mainstay.zone_id
}

output "api_endpoints" {
  description = "Regional API endpoint URLs"
  value = {
    "us-east-1"      = "https://${aws_route53_record.api_us_east_1.name}"
    "eu-west-1"      = "https://${aws_route53_record.api_eu_west_1.name}"
    "ap-southeast-1" = "https://${aws_route53_record.api_ap_southeast_1.name}"
  }
}

output "alb_dns_names" {
  description = "ALB DNS names per region"
  value = {
    "us-east-1"      = module.api_server_us_east_1.alb_dns_name
    "eu-west-1"      = module.api_server_eu_west_1.alb_dns_name
    "ap-southeast-1" = module.api_server_ap_southeast_1.alb_dns_name
  }
}

output "health_check_ids" {
  description = "Route53 health check IDs per region"
  value = {
    "us-east-1"      = aws_route53_health_check.api_us_east_1.id
    "eu-west-1"      = aws_route53_health_check.api_eu_west_1.id
    "ap-southeast-1" = aws_route53_health_check.api_ap_southeast_1.id
  }
}

output "sns_topic_arn" {
  description = "SNS topic ARN for cache invalidation"
  value       = aws_sns_topic.cache_invalidation.arn
}
