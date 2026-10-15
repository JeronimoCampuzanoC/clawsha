# DFSha on AWS: one region, two AZs. Per AZ:
#   public subnet   gateway + NAT gateway
#   private subnet  master (one leads, the other waits as standby)
#   private subnet  workers (blocks on an EBS volume of the instance)
# plus the DynamoDB tables, an internal NLB that always points at the leading
# master, and a public NLB in front of the gateways.

provider "aws" {
  region = var.region

  default_tags {
    tags = { Project = "dfsha" }
  }
}

# Latest Amazon Linux 2023; every node only needs Docker.
data "aws_ami" "al2023" {
  most_recent = true
  owners      = ["amazon"]

  filter {
    name   = "name"
    values = ["al2023-ami-2023.*-x86_64"]
  }
}

module "network" {
  source = "./modules/network"

  name               = var.name
  vpc_cidr           = var.vpc_cidr
  azs                = var.azs
  single_nat_gateway = var.single_nat_gateway
}

module "security" {
  source = "./modules/security"

  name         = var.name
  vpc_id       = module.network.vpc_id
  vpc_cidr     = module.network.vpc_cidr
  gateway_port = var.gateway_port
}

module "data" {
  source = "./modules/data"

  table_prefix = var.table_prefix
}

module "control" {
  source = "./modules/control"

  name                  = var.name
  vpc_id                = module.network.vpc_id
  subnet_ids            = module.network.master_subnet_ids
  security_group_id     = module.security.master_sg_id
  ami_id                = data.aws_ami.al2023.id
  instance_type         = var.master_instance_type
  instance_profile_name = var.instance_profile_name

  user_data = templatefile("${path.module}/scripts/master.sh.tftpl", {
    image        = var.master_image
    table_prefix = var.table_prefix
    region       = var.region
  })

  # The master refuses to start without its tables.
  depends_on = [module.data]
}

module "workers" {
  source = "./modules/workers"

  name                  = var.name
  subnet_ids            = module.network.worker_subnet_ids
  security_group_id     = module.security.worker_sg_id
  ami_id                = data.aws_ami.al2023.id
  instance_type         = var.worker_instance_type
  instance_profile_name = var.instance_profile_name
  data_volume_gb        = var.worker_data_volume_gb
  min_size              = var.worker_min
  max_size              = var.worker_max
  scale_out_percent     = var.scale_out_percent
  cpu_scale_out_percent = var.worker_cpu_scale_out_percent

  user_data = templatefile("${path.module}/scripts/worker.sh.tftpl", {
    image       = var.worker_image
    quota_bytes = var.worker_quota_bytes
    master_addr = module.control.internal_nlb_dns
  })
}

module "edge" {
  source = "./modules/edge"
  count  = var.gateway_image == "" ? 0 : 1

  name                  = var.name
  vpc_id                = module.network.vpc_id
  subnet_ids            = module.network.public_subnet_ids
  security_group_id     = module.security.gateway_sg_id
  ami_id                = data.aws_ami.al2023.id
  instance_type         = var.gateway_instance_type
  instance_profile_name = var.instance_profile_name
  gateway_port          = var.gateway_port

  user_data = templatefile("${path.module}/scripts/gateway.sh.tftpl", {
    image       = var.gateway_image
    port        = var.gateway_port
    master_addr = module.control.internal_nlb_dns
    jwt_secret  = random_password.jwt_secret.result
  })
}

# Secret the gateways sign the session tokens with. Generated once and kept in
# the Terraform state, so both gateways share it and it survives re-applies.
resource "random_password" "jwt_secret" {
  length  = 48
  special = false
}

