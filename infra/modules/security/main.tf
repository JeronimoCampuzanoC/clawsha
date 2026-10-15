# One security group per role. No port 22 anywhere: shell access is through
# SSM Session Manager.
#
# Inside the VPC the rules use the VPC CIDR instead of other security groups:
# the internal NLB health checks come from its own private IPs, which belong
# to no security group.

resource "aws_security_group" "gateway" {
  name        = "${var.name}-gateway"
  description = "Gateways: HTTP API from the internet (through the public NLB)"
  vpc_id      = var.vpc_id

  ingress {
    description = "Gateway API; the NLB keeps the client IP, so the source is the internet"
    from_port   = var.gateway_port
    to_port     = var.gateway_port
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = { Name = "${var.name}-gateway" }
}

resource "aws_security_group" "master" {
  name        = "${var.name}-master"
  description = "Masters: gRPC and health checks from inside the VPC"
  vpc_id      = var.vpc_id

  ingress {
    description = "gRPC (MasterService + ClusterService)"
    from_port   = 50051
    to_port     = 50051
    protocol    = "tcp"
    cidr_blocks = [var.vpc_cidr]
  }

  ingress {
    description = "/health and /health/leader (internal NLB)"
    from_port   = 8080
    to_port     = 8080
    protocol    = "tcp"
    cidr_blocks = [var.vpc_cidr]
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = { Name = "${var.name}-master" }
}

resource "aws_security_group" "worker" {
  name        = "${var.name}-worker"
  description = "Workers: WorkerService from gateways, masters and other workers"
  vpc_id      = var.vpc_id

  ingress {
    description = "gRPC (WorkerService)"
    from_port   = 50052
    to_port     = 50052
    protocol    = "tcp"
    cidr_blocks = [var.vpc_cidr]
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = { Name = "${var.name}-worker" }
}
