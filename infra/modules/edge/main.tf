# Public entry point: an internet-facing NLB on :443 in front of one gateway
# per AZ. The NLB only forwards TCP; the gateway terminates TLS itself.

resource "aws_launch_template" "gateway" {
  name_prefix   = "${var.name}-gateway-"
  image_id      = var.ami_id
  instance_type = var.instance_type
  user_data     = base64encode(var.user_data)

  vpc_security_group_ids = [var.security_group_id]

  iam_instance_profile {
    name = var.instance_profile_name
  }

  metadata_options {
    http_endpoint               = "enabled"
    http_tokens                 = "required"
    http_put_response_hop_limit = 2
  }

  tag_specifications {
    resource_type = "instance"
    tags          = { Name = "${var.name}-gateway", Role = "gateway" }
  }
}

resource "aws_lb" "public" {
  name                             = "${var.name}-public"
  internal                         = false
  load_balancer_type               = "network"
  subnets                          = var.subnet_ids
  enable_cross_zone_load_balancing = true
}

resource "aws_lb_target_group" "https" {
  name                 = "${var.name}-gateways-https"
  port                 = 443
  protocol             = "TCP"
  vpc_id               = var.vpc_id
  deregistration_delay = 30

  health_check {
    protocol            = "TCP"
    interval            = 10
    healthy_threshold   = 2
    unhealthy_threshold = 2
  }
}

resource "aws_lb_listener" "https" {
  load_balancer_arn = aws_lb.public.arn
  port              = 443
  protocol          = "TCP"

  default_action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.https.arn
  }
}

resource "aws_autoscaling_group" "gateways" {
  name                = "${var.name}-gateways"
  min_size            = length(var.subnet_ids)
  max_size            = length(var.subnet_ids)
  desired_capacity    = length(var.subnet_ids)
  vpc_zone_identifier = var.subnet_ids
  target_group_arns   = [aws_lb_target_group.https.arn]
  health_check_type   = "EC2"

  launch_template {
    id      = aws_launch_template.gateway.id
    version = aws_launch_template.gateway.latest_version
  }

  tag {
    key                 = "Name"
    value               = "${var.name}-gateway"
    propagate_at_launch = true
  }
}
