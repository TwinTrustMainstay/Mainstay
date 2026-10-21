# ── VPC ────────────────────────────────────────────────────
resource "aws_vpc" "main" {
  cidr_block           = var.vpc_cidr
  enable_dns_hostnames = true
  enable_dns_support   = true

  tags = { Name = "mainstay-vpc-${var.region}" }
}

resource "aws_internet_gateway" "main" {
  vpc_id = aws_vpc.main.id

  tags = { Name = "mainstay-igw-${var.region}" }
}

resource "aws_route_table" "public" {
  vpc_id = aws_vpc.main.id

  route {
    cidr_block = "0.0.0.0/0"
    gateway_id = aws_internet_gateway.main.id
  }

  tags = { Name = "mainstay-rt-${var.region}" }
}

# ── Subnets (2 AZs for HA) ────────────────────────────────
resource "aws_subnet" "public" {
  count                   = 2
  vpc_id                  = aws_vpc.main.id
  cidr_block              = cidrsubnet(var.vpc_cidr, 8, count.index)
  availability_zone       = data.aws_availability_zones.available.names[count.index]
  map_public_ip_on_launch = true

  tags = { Name = "mainstay-subnet-${var.region}-${count.index}" }
}

resource "aws_route_table_association" "public" {
  count          = 2
  subnet_id      = aws_subnet.public[count.index].id
  route_table_id = aws_route_table.public.id
}

# ── Private subnets and egress ──────────────────────────────
resource "aws_eip" "nat" {
  count  = 2
  domain = "vpc"

  tags = { Name = "mainstay-nat-eip-${var.region}-${count.index}" }
}

resource "aws_nat_gateway" "main" {
  count         = 2
  allocation_id = aws_eip.nat[count.index].id
  subnet_id     = aws_subnet.public[count.index].id

  depends_on = [aws_internet_gateway.main]
  tags       = { Name = "mainstay-nat-${var.region}-${count.index}" }
}

resource "aws_subnet" "private" {
  count                   = 2
  vpc_id                  = aws_vpc.main.id
  cidr_block              = cidrsubnet(var.vpc_cidr, 8, count.index + 2)
  availability_zone       = data.aws_availability_zones.available.names[count.index]
  map_public_ip_on_launch = false

  tags = { Name = "mainstay-private-${var.region}-${count.index}" }
}

resource "aws_route_table" "private" {
  count  = 2
  vpc_id = aws_vpc.main.id

  route {
    cidr_block     = "0.0.0.0/0"
    nat_gateway_id = aws_nat_gateway.main[count.index].id
  }

  tags = { Name = "mainstay-private-rt-${var.region}-${count.index}" }
}

resource "aws_route_table_association" "private" {
  count          = 2
  subnet_id      = aws_subnet.private[count.index].id
  route_table_id = aws_route_table.private[count.index].id
}

# ── Security groups ─────────────────────────────────────────
resource "aws_security_group" "alb" {
  name        = "mainstay-alb-${var.region}"
  description = "Public HTTPS entry point for the Mainstay API"
  vpc_id      = aws_vpc.main.id

  ingress {
    from_port   = 443
    to_port     = 443
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
    description = "HTTPS from internet"
  }

  ingress {
    from_port   = 80
    to_port     = 80
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
    description = "HTTP (redirect to HTTPS)"
  }

  # #1813: defining a security group with explicit ingress/egress blocks
  # makes Terraform remove AWS's default allow-all egress rule. Without
  # this, the ALB cannot open connections to its targets and forwarding
  # fails entirely.
  egress {
    from_port       = 80
    to_port         = 80
    protocol        = "tcp"
    security_groups = [aws_security_group.api.id]
    description     = "Forward to API targets (nginx)"
  }

  tags = { Name = "mainstay-alb-sg-${var.region}" }
}

resource "aws_security_group" "api" {
  name        = "mainstay-api-${var.region}"
  description = "Private security group for Mainstay API servers"
  vpc_id      = aws_vpc.main.id

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = { Name = "mainstay-sg-${var.region}" }
}

resource "aws_security_group_rule" "api_from_alb" {
  type                     = "ingress"
  security_group_id        = aws_security_group.api.id
  source_security_group_id = aws_security_group.alb.id
  from_port                = 80
  to_port                  = 80
  protocol                 = "tcp"
  # #1812: the ALB targets nginx (port 80) so rate limiting, security
  # headers, CORS and the /metrics ACL actually apply. The API container
  # itself is bound to loopback only and is not reachable from the ALB.
  description = "HTTP to nginx only from the ALB"
}

resource "aws_security_group_rule" "api_metrics_from_prometheus" {
  type                     = "ingress"
  security_group_id        = aws_security_group.api.id
  source_security_group_id = var.prometheus_security_group_id
  from_port                = var.metrics_port
  to_port                  = var.metrics_port
  protocol                 = "tcp"
  description              = "Metrics scraping only from the Prometheus host"
}

# ── Data-subject request processing ─────────────────────────
resource "aws_dynamodb_table" "data_subject_requests" {
  name         = "mainstay-data-subject-requests-${var.region}"
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "request_id"

  attribute {
    name = "request_id"
    type = "S"
  }

  ttl {
    attribute_name = "expires_at"
    enabled        = true
  }

  server_side_encryption {
    enabled = true
  }

  point_in_time_recovery {
    enabled = true
  }

  tags = {
    Name    = "mainstay-data-subject-requests-${var.region}"
    Project = "Mainstay"
  }
}

resource "aws_cloudwatch_log_group" "nginx_access" {
  name              = "/mainstay/nginx/access"
  retention_in_days = var.log_retention_days

  tags = {
    Project = "Mainstay"
    Region  = var.region
  }
}

resource "aws_cloudwatch_log_group" "nginx_error" {
  name              = "/mainstay/nginx/error"
  retention_in_days = var.log_retention_days

  tags = {
    Project = "Mainstay"
    Region  = var.region
  }
}

# Dead-letter queue: isolates DSAR messages that repeatedly fail processing
# so they can be inspected and replayed instead of stalling the main queue.
resource "aws_sqs_queue" "data_subject_requests_dlq" {
  name                       = "mainstay-data-subject-requests-dlq-${var.region}"
  message_retention_seconds  = var.request_retention_seconds
  visibility_timeout_seconds = 300
  sqs_managed_sse_enabled    = true

  tags = {
    Project = "Mainstay"
    Region  = var.region
  }
}

resource "aws_sqs_queue" "data_subject_requests" {
  name                       = "mainstay-data-subject-requests-${var.region}"
  message_retention_seconds = var.request_retention_seconds
  receive_wait_time_seconds = 20
  visibility_timeout_seconds = 300
  sqs_managed_sse_enabled   = true

  redrive_policy = jsonencode({
    deadLetterTargetArn = aws_sqs_queue.data_subject_requests_dlq.arn
    maxReceiveCount     = 5
  })

  tags = {
    Project = "Mainstay"
    Region  = var.region
  }
}

resource "aws_iam_role" "api" {
  name = "mainstay-api-${var.region}"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Principal = { Service = "ec2.amazonaws.com" }
      Action = "sts:AssumeRole"
    }]
  })
}

resource "aws_iam_role_policy" "api_privacy" {
  name = "mainstay-api-privacy-${var.region}"
  role = aws_iam_role.api.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect   = "Allow"
        Action   = ["dynamodb:PutItem", "dynamodb:GetItem", "dynamodb:UpdateItem"]
        Resource = aws_dynamodb_table.data_subject_requests.arn
      },
      {
        Effect   = "Allow"
        Action   = ["sqs:SendMessage", "sqs:ReceiveMessage", "sqs:DeleteMessage", "sqs:GetQueueAttributes"]
        Resource = aws_sqs_queue.data_subject_requests.arn
      },
      {
        Effect   = "Allow"
        Action   = ["sqs:SendMessage", "sqs:ReceiveMessage", "sqs:DeleteMessage", "sqs:GetQueueAttributes"]
        Resource = aws_sqs_queue.data_subject_requests_dlq.arn
      }
    ]
  })
}

resource "aws_iam_instance_profile" "api" {
  name = "mainstay-api-${var.region}"
  role = aws_iam_role.api.name
}

# #1815: user_data.sh starts amazon-cloudwatch-agent, which ships nginx
# logs and host metrics, but no policy granted PutLogEvents/PutMetricData,
# so the agent's calls were silently denied.
resource "aws_iam_role_policy_attachment" "api_cloudwatch_agent" {
  role       = aws_iam_role.api.name
  policy_arn = "arn:aws:iam::aws:policy/CloudWatchAgentServerPolicy"
}

# ── Application Load Balancer ──────────────────────────────
resource "aws_lb" "api" {
  name               = "mainstay-api-${replace(var.region, "-", "")}"
  internal           = false
  load_balancer_type = "application"
  security_groups    = [aws_security_group.alb.id]
  subnets            = aws_subnet.public[*].id

  tags = { Name = "mainstay-alb-${var.region}" }
}

resource "aws_lb_target_group" "api" {
  name     = "mainstay-tg-${substr(replace(var.region, "-", ""), 0, 10)}"
  # #1812: target nginx (80), not the API container directly, so the ALB
  # traffic actually goes through nginx's rate limiting, security headers,
  # CORS policy and /metrics ACL.
  port     = 80
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

  stickiness {
    type    = "lb_cookie"
    enabled = false
  }

  tags = { Name = "mainstay-tg-${var.region}" }
}

resource "aws_lb_listener" "https" {
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

# Deny /metrics at the ALB layer: never forwarded to the target group.
resource "aws_lb_listener_rule" "deny_metrics" {
  listener_arn = aws_lb_listener.https.arn
  priority     = 1

  action {
    type = "fixed-response"

    fixed_response {
      content_type = "text/plain"
      message_body = "Not Found"
      status_code  = "404"
    }
  }

  condition {
    path_pattern {
      values = ["/metrics", "/metrics/*"]
    }
  }

  tags = { Name = "mainstay-deny-metrics-${var.region}" }
}

resource "aws_lb_listener" "http" {
  load_balancer_arn = aws_lb.api.arn
  port              = 80
  protocol          = "HTTP"

  default_action {
    type = "redirect"
    redirect {
      protocol    = "HTTPS"
      port        = "443"
      status_code = "HTTP_301"
    }
  }
}

# ── Launch Template ────────────────────────────────────────
resource "aws_launch_template" "api" {
  name_prefix   = "mainstay-api-${var.region}-"
  image_id      = var.ami_id
  instance_type = var.instance_type
  key_name      = var.key_name

  user_data = base64encode(templatefile("${path.module}/user_data.sh", {
    region          = var.region
    rpc_url         = var.rpc_url
    network         = var.network_passphrase
    allowed_origins = var.allowed_origins
    dsar_table_name = aws_dynamodb_table.data_subject_requests.name
    dsar_queue_url  = aws_sqs_queue.data_subject_requests.url
    image_tag       = var.image_tag
    metrics_port    = var.metrics_port
  }))

  iam_instance_profile {
    name = aws_iam_instance_profile.api.name
  }

  vpc_security_group_ids = [aws_security_group.api.id]
  block_device_mappings {
    device_name = "/dev/xvda"

    ebs {
      encrypted = true
    }
  }

  metadata_options {
    http_tokens   = "required"
    http_endpoint = "enabled"
    # #1814: the API runs in a Docker bridge container, one network hop
    # beyond the host. The default hop limit of 1 blocks IMDSv2 token
    # requests made from inside the container, so the AWS SDK default
    # credential chain (used for DynamoDB/SQS calls) would time out.
    http_put_response_hop_limit = 2
  }

  tag_specifications {
    resource_type = "instance"
    tags = { Name = "mainstay-api-${var.region}" }
  }
}

# ── Auto Scaling Group ─────────────────────────────────────
resource "aws_autoscaling_group" "api" {
  name                = "mainstay-asg-${var.region}"
  vpc_zone_identifier = aws_subnet.private[*].id
  min_size            = 2
  max_size            = 6
  desired_capacity    = 2

  launch_template {
    id      = aws_launch_template.api.id
    version = "$Latest"
  }

  target_group_arns = [aws_lb_target_group.api.arn]

  health_check_type         = "ELB"
  health_check_grace_period = 300

  instance_refresh {
    strategy = "Rolling"
    preferences {
      min_healthy_percentage = 50
    }
  }

  tag {
    key                 = "Name"
    value               = "mainstay-api-${var.region}"
    propagate_at_launch = true
  }

  tag {
    key                 = "Project"
    value               = "Mainstay"
    propagate_at_launch = true
  }
}

# ── CloudWatch Alarms ──────────────────────────────────────
resource "aws_sns_topic" "alarms" {
  name = "mainstay-api-alarms-${var.region}"

  tags = {
    Project = "Mainstay"
    Region  = var.region
  }
}

resource "aws_sns_topic_subscription" "alarms_on_call" {
  topic_arn = aws_sns_topic.alarms.arn
  protocol  = "email"
  endpoint  = var.on_call_email
}

resource "aws_cloudwatch_metric_alarm" "api_5xx" {
  alarm_name          = "mainstay-api-5xx-${var.region}"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 2
  metric_name         = "HTTPCode_Target_5XX_Count"
  namespace           = "AWS/ApplicationELB"
  period              = 60
  statistic           = "Sum"
  threshold           = 5
  alarm_description   = "High 5xx error rate on API ALB"
  alarm_actions       = [aws_sns_topic.alarms.arn]
  ok_actions          = [aws_sns_topic.alarms.arn]

  dimensions = {
    LoadBalancer = aws_lb.api.arn_suffix
  }
}

resource "aws_cloudwatch_metric_alarm" "api_latency" {
  alarm_name          = "mainstay-api-latency-${var.region}"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 2
  metric_name         = "TargetResponseTime"
  namespace           = "AWS/ApplicationELB"
  period              = 60
  extended_statistic  = "p95"
  threshold           = 2
  alarm_description   = "API p95 latency exceeding 2 seconds"
  alarm_actions       = [aws_sns_topic.alarms.arn]
  ok_actions          = [aws_sns_topic.alarms.arn]

  dimensions = {
    LoadBalancer = aws_lb.api.arn_suffix
  }
}

# ── Outputs ────────────────────────────────────────────────
output "metrics_port" {
  description = "Private port serving Prometheus metrics (not exposed via the ALB)"
  value       = var.metrics_port
}

output "metrics_target" {
  description = "Private metrics scrape target for the Prometheus host"
  value       = "mainstay-api-${var.region}:${var.metrics_port}"
}

# Any message in the DSAR DLQ means a data-subject request failed repeatedly
# and needs manual attention. See docs/runbooks/dsar-dlq.md for replay steps.
resource "aws_cloudwatch_metric_alarm" "dsar_dlq_depth" {
  alarm_name          = "mainstay-dsar-dlq-depth-${var.region}"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "ApproximateNumberOfMessagesVisible"
  namespace           = "AWS/SQS"
  period              = 300
  statistic           = "Maximum"
  threshold           = 0
  treat_missing_data  = "notBreaching"
  alarm_description   = "DSAR dead-letter queue is non-empty; failed data-subject requests require replay or manual processing"

  dimensions = {
    QueueName = aws_sqs_queue.data_subject_requests_dlq.name
  }

  tags = {
    Project = "Mainstay"
    Region  = var.region
  }
}
