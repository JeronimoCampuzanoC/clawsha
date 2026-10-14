# Workers in one ASG spread over the two private worker subnets.
#
# Each instance has a second EBS volume mounted on /data and bind-mounted into
# the container, so blocks survive a container crash or an instance reboot.
# If the instance itself is replaced, its volume goes with it and the master
# re-replicates the lost blocks from the other copies.

resource "aws_launch_template" "worker" {
  name_prefix   = "${var.name}-worker-"
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

  # CPU every minute instead of every 5, so the CPU alarm reacts in time.
  monitoring {
    enabled = true
  }

  block_device_mappings {
    device_name = "/dev/sdf"

    ebs {
      volume_size           = var.data_volume_gb
      volume_type           = "gp3"
      encrypted             = true
      delete_on_termination = true
    }
  }

  tag_specifications {
    resource_type = "instance"
    tags          = { Name = "${var.name}-worker", Role = "worker" }
  }

  tag_specifications {
    resource_type = "volume"
    tags          = { Name = "${var.name}-worker-data" }
  }
}

resource "aws_autoscaling_group" "workers" {
  name                = "${var.name}-workers"
  min_size            = var.min_size
  max_size            = var.max_size
  desired_capacity    = var.min_size
  vpc_zone_identifier = var.subnet_ids
  health_check_type   = "EC2"

  launch_template {
    id      = aws_launch_template.worker.id
    version = aws_launch_template.worker.latest_version
  }

  # Scaling changes the size; Terraform must not undo it on the next apply.
  lifecycle {
    ignore_changes = [desired_capacity]
  }

  tag {
    key                 = "Name"
    value               = "${var.name}-worker"
    propagate_at_launch = true
  }
}

# ----- scale out --------------------------------------------------------------
# Two alarms add one worker each: the cluster is filling up, or the workers
# are busy. There is no scale-in on purpose: a removed worker takes its blocks
# with it and the master would have to copy them again.
# The leader publishes DFSha/ClusterUsedPercent (master health module).
# Until it does, the alarm stays in INSUFFICIENT_DATA and nothing happens.

resource "aws_autoscaling_policy" "add_worker" {
  name                   = "${var.name}-add-worker"
  autoscaling_group_name = aws_autoscaling_group.workers.name
  adjustment_type        = "ChangeInCapacity"
  scaling_adjustment     = 1
  cooldown               = 300
}

resource "aws_cloudwatch_metric_alarm" "cluster_full" {
  alarm_name          = "${var.name}-cluster-used-percent"
  alarm_description   = "Cluster usage above ${var.scale_out_percent}% for 2 minutes: add one worker"
  namespace           = "DFSha"
  metric_name         = "ClusterUsedPercent"
  statistic           = "Average"
  period              = 60
  evaluation_periods  = 2
  comparison_operator = "GreaterThanThreshold"
  threshold           = var.scale_out_percent
  treat_missing_data  = "notBreaching"
  alarm_actions       = [aws_autoscaling_policy.add_worker.arn]
}

# EC2 measures CPU itself; the workers do not send anything for this.
resource "aws_cloudwatch_metric_alarm" "workers_busy" {
  alarm_name          = "${var.name}-workers-cpu"
  alarm_description   = "Average worker CPU above ${var.cpu_scale_out_percent}% for 3 minutes: add one worker"
  namespace           = "AWS/EC2"
  metric_name         = "CPUUtilization"
  statistic           = "Average"
  period              = 60
  evaluation_periods  = 3
  comparison_operator = "GreaterThanThreshold"
  threshold           = var.cpu_scale_out_percent
  treat_missing_data  = "notBreaching"
  alarm_actions       = [aws_autoscaling_policy.add_worker.arn]

  dimensions = {
    AutoScalingGroupName = aws_autoscaling_group.workers.name
  }
}
