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
  from_port                = 8080
  to_port                  = 8080
  protocol                 = "tcp"
  description              = "HTTP only from the ALB"
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

resource "aws_sqs_queue" "data_subject_requests" {
  name                       = "mainstay-data-subject-requests-${var.region}"
  message_retention_seconds = var.request_retention_seconds
  receive_wait_time_seconds = 20
  visibility_timeout_seconds = 300
  sqs_managed_sse_enabled   = true

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
      }
    ]
  })
}

resource "aws_iam_instance_profile" "api" {
  name = "mainstay-api-${var.region}"
  role = aws_iam_role.api.name
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

    resource "aws_iam_role" "api" {
      name = "mainstay-api-${var.region}"

      assume_role_policy = jsonencode({
        Version = "2012-10-17"
        Statement = [{
          Effect    = "Allow"
          Principal = { Service = "ec2.amazonaws.com" }
          Action    = "sts:AssumeRole"
        }]
      })
    }

    resource "aws_iam_instance_profile" "api" {
      name = "mainstay-api-${var.region}"
      role = aws_iam_role.api.name
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
  iam_instance_profile {
    name = aws_iam_instance_profile.api.name
  }

  metadata_options {
    http_tokens   = "required"
    http_endpoint = "enabled"
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
  statistic           = "p95"
  threshold           = 2
  alarm_description   = "API p95 latency exceeding 2 seconds"

  dimensions = {
    LoadBalancer = aws_lb.api.arn_suffix
  }
}
