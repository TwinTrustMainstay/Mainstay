output "alb_dns_name" {
  description = "DNS name of the application load balancer"
  value       = aws_lb.api.dns_name
}

output "alb_zone_id" {
  description = "Route 53 zone ID of the ALB"
  value       = aws_lb.api.zone_id
}

output "asg_name" {
  description = "Name of the auto scaling group"
  value       = aws_autoscaling_group.api.name
}

output "target_group_arn" {
  description = "ARN of the ALB target group"
  value       = aws_lb_target_group.api.arn
}
