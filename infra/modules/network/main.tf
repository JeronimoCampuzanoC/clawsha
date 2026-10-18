# VPC with four subnets per AZ:
#   public   public NLB, NAT gateway        10.0.0.0/24,  10.0.1.0/24
#   master   one master per AZ (private)    10.0.10.0/24, 10.0.11.0/24
#   worker   workers (private)              10.0.20.0/24, 10.0.21.0/24
#   gateway  gateways (private)             10.0.30.0/24, 10.0.31.0/24

resource "aws_vpc" "this" {
  cidr_block           = var.vpc_cidr
  enable_dns_support   = true
  enable_dns_hostnames = true

  tags = { Name = var.name }
}

resource "aws_internet_gateway" "this" {
  vpc_id = aws_vpc.this.id
  tags   = { Name = var.name }
}

# ----- subnets ---------------------------------------------------------------

resource "aws_subnet" "public" {
  count                   = length(var.azs)
  vpc_id                  = aws_vpc.this.id
  availability_zone       = var.azs[count.index]
  cidr_block              = cidrsubnet(var.vpc_cidr, 8, count.index)
  map_public_ip_on_launch = true

  tags = { Name = "${var.name}-public-${var.azs[count.index]}" }
}

resource "aws_subnet" "master" {
  count             = length(var.azs)
  vpc_id            = aws_vpc.this.id
  availability_zone = var.azs[count.index]
  cidr_block        = cidrsubnet(var.vpc_cidr, 8, 10 + count.index)

  tags = { Name = "${var.name}-master-${var.azs[count.index]}" }
}

resource "aws_subnet" "worker" {
  count             = length(var.azs)
  vpc_id            = aws_vpc.this.id
  availability_zone = var.azs[count.index]
  cidr_block        = cidrsubnet(var.vpc_cidr, 8, 20 + count.index)

  tags = { Name = "${var.name}-worker-${var.azs[count.index]}" }
}

# No public IPs: the only way in from the internet is the public NLB on :443.
resource "aws_subnet" "gateway" {
  count             = length(var.azs)
  vpc_id            = aws_vpc.this.id
  availability_zone = var.azs[count.index]
  cidr_block        = cidrsubnet(var.vpc_cidr, 8, 30 + count.index)

  tags = { Name = "${var.name}-gateway-${var.azs[count.index]}" }
}

# ----- public routing: straight to the internet gateway ---------------------

resource "aws_route_table" "public" {
  vpc_id = aws_vpc.this.id

  route {
    cidr_block = "0.0.0.0/0"
    gateway_id = aws_internet_gateway.this.id
  }

  tags = { Name = "${var.name}-public" }
}

resource "aws_route_table_association" "public" {
  count          = length(var.azs)
  subnet_id      = aws_subnet.public[count.index].id
  route_table_id = aws_route_table.public.id
}

# ----- NAT: private instances reach the internet (Docker images) through it --

locals {
  nat_count = var.single_nat_gateway ? 1 : length(var.azs)
}

resource "aws_eip" "nat" {
  count  = local.nat_count
  domain = "vpc"
  tags   = { Name = "${var.name}-nat-${var.azs[count.index]}" }
}

resource "aws_nat_gateway" "this" {
  count         = local.nat_count
  allocation_id = aws_eip.nat[count.index].id
  subnet_id     = aws_subnet.public[count.index].id

  tags       = { Name = "${var.name}-nat-${var.azs[count.index]}" }
  depends_on = [aws_internet_gateway.this]
}

# ----- private routing: one table per AZ, shared by its private subnets

resource "aws_route_table" "private" {
  count  = length(var.azs)
  vpc_id = aws_vpc.this.id

  route {
    cidr_block     = "0.0.0.0/0"
    nat_gateway_id = aws_nat_gateway.this[var.single_nat_gateway ? 0 : count.index].id
  }

  tags = { Name = "${var.name}-private-${var.azs[count.index]}" }
}

resource "aws_route_table_association" "master" {
  count          = length(var.azs)
  subnet_id      = aws_subnet.master[count.index].id
  route_table_id = aws_route_table.private[count.index].id
}

resource "aws_route_table_association" "worker" {
  count          = length(var.azs)
  subnet_id      = aws_subnet.worker[count.index].id
  route_table_id = aws_route_table.private[count.index].id
}

resource "aws_route_table_association" "gateway" {
  count          = length(var.azs)
  subnet_id      = aws_subnet.gateway[count.index].id
  route_table_id = aws_route_table.private[count.index].id
}

# DynamoDB traffic stays inside AWS instead of going through the NAT (free, faster).
data "aws_region" "current" {}

resource "aws_vpc_endpoint" "dynamodb" {
  vpc_id            = aws_vpc.this.id
  service_name      = "com.amazonaws.${data.aws_region.current.name}.dynamodb"
  vpc_endpoint_type = "Gateway"
  route_table_ids   = aws_route_table.private[*].id

  tags = { Name = "${var.name}-dynamodb" }
}
