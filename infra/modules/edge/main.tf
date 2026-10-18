# Public entry point: an internet-facing NLB on :443 (public subnets) in front
# of one gateway node per AZ (private subnets, no public IP). The NLB terminates
# TLS and forwards plain TCP to nginx on the node, which serves the frontend and
# proxies /api/ to the gateway container; both keep speaking HTTP. There is no :80 listener: an NLB cannot redirect, and plain
# HTTP would carry the JWT in the clear.

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
  subnets                          = var.public_subnet_ids
  enable_cross_zone_load_balancing = true
}

# Without a domain there is no public certificate: generate a self-signed one
# for the NLB's DNS name and import it into ACM for the TLS listener.
resource "tls_private_key" "api" {
  algorithm   = "ECDSA"
  ecdsa_curve = "P256"
}

resource "tls_self_signed_cert" "api" {
  private_key_pem = tls_private_key.api.private_key_pem

  subject {
    common_name  = aws_lb.public.dns_name
    organization = var.name
  }

  dns_names             = [aws_lb.public.dns_name]
  validity_period_hours = 24 * 365
  early_renewal_hours   = 24 * 30

  allowed_uses = ["digital_signature", "key_encipherment", "server_auth"]
}

resource "aws_acm_certificate" "api" {
  private_key      = tls_private_key.api.private_key_pem
  certificate_body = tls_self_signed_cert.api.cert_pem

  lifecycle {
    create_before_destroy = true
  }
}

resource "aws_lb_target_group" "api" {
  name                 = "${var.name}-gateways-api"
  port                 = var.frontend_port
  protocol             = "TCP"
  vpc_id               = var.vpc_id
  deregistration_delay = 30

  # HTTP, not TCP: docker-proxy accepts the connection even when nginx is down.
  health_check {
    protocol            = "HTTP"
    path                = "/healthz"
    interval            = 10
    healthy_threshold   = 2
    unhealthy_threshold = 2
  }
}

resource "aws_lb_listener" "https" {
  load_balancer_arn = aws_lb.public.arn
  port              = 443
  protocol          = "TLS"
  certificate_arn   = aws_acm_certificate.api.arn
  ssl_policy        = "ELBSecurityPolicy-TLS13-1-2-2021-06"
  alpn_policy       = "HTTP1Only"

  default_action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.api.arn
  }
}

resource "aws_autoscaling_group" "gateways" {
  name                = "${var.name}-gateways"
  min_size            = length(var.subnet_ids)
  max_size            = length(var.subnet_ids)
  desired_capacity    = length(var.subnet_ids)
  vpc_zone_identifier = var.subnet_ids
  target_group_arns   = [aws_lb_target_group.api.arn]
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
