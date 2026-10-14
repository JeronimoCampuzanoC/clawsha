# Two masters, one per AZ, each in its own ASG of exactly one instance: if the
# instance dies, its ASG brings up a new one in the same AZ. Both masters run
# all the time; the DynamoDB lease decides which one leads.
#
# The internal NLB sends gRPC only to the leader: its health check is
# /health/leader, which answers 200 on the leader and 503 on the standby.
# Workers and gateways always call the NLB's DNS name.

resource "aws_launch_template" "master" {
  name_prefix   = "${var.name}-master-"
  image_id      = var.ami_id
  instance_type = var.instance_type
  user_data     = base64encode(var.user_data)

  vpc_security_group_ids = [var.security_group_id]

  iam_instance_profile {
    name = var.instance_profile_name
  }

  # IMDSv2 only. Hop limit 2 so the container (one hop more) can still reach
  # the metadata service and get the instance profile's credentials.
  metadata_options {
    http_endpoint               = "enabled"
    http_tokens                 = "required"
    http_put_response_hop_limit = 2
  }

  tag_specifications {
    resource_type = "instance"
    tags          = { Name = "${var.name}-master", Role = "master" }
  }
}

resource "aws_lb" "masters" {
  name                             = "${var.name}-masters"
  internal                         = true
  load_balancer_type               = "network"
  subnets                          = var.subnet_ids
  enable_cross_zone_load_balancing = true # the leader can be in either AZ
}

resource "aws_lb_target_group" "grpc" {
  name                 = "${var.name}-masters-grpc"
  port                 = 50051
  protocol             = "TCP"
  vpc_id               = var.vpc_id
  deregistration_delay = 10

  health_check {
    protocol            = "HTTP"
    port                = "8080"
    path                = "/health/leader"
    matcher             = "200"
    interval            = 10
    healthy_threshold   = 2
    unhealthy_threshold = 2
  }
}

resource "aws_lb_listener" "grpc" {
  load_balancer_arn = aws_lb.masters.arn
  port              = 50051
  protocol          = "TCP"

  default_action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.grpc.arn
  }
}

resource "aws_autoscaling_group" "master" {
  count = length(var.subnet_ids)

  name                = "${var.name}-master-${count.index}"
  min_size            = 1
  max_size            = 1
  desired_capacity    = 1
  vpc_zone_identifier = [var.subnet_ids[count.index]]
  target_group_arns   = [aws_lb_target_group.grpc.arn]

  # EC2, not ELB: the standby fails the NLB health check on purpose, and an
  # ELB health check would make the ASG kill it.
  health_check_type = "EC2"

  launch_template {
    id      = aws_launch_template.master.id
    version = aws_launch_template.master.latest_version
  }

  tag {
    key                 = "Name"
    value               = "${var.name}-master-${count.index}"
    propagate_at_launch = true
  }
}
