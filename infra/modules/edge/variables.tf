variable "name" {
  type = string
}

variable "vpc_id" {
  type = string
}

variable "public_subnet_ids" {
  description = "The public subnets, one per AZ, for the NLB."
  type        = list(string)
}

variable "subnet_ids" {
  description = "The private gateway subnets, one per AZ, for the instances."
  type        = list(string)
}

variable "security_group_id" {
  type = string
}

variable "ami_id" {
  type = string
}

variable "instance_type" {
  type = string
}

variable "instance_profile_name" {
  type = string
}

variable "user_data" {
  description = "Rendered infra/scripts/gateway.sh.tftpl."
  type        = string
}

variable "frontend_port" {
  description = "Port nginx publishes on the instance; the NLB forwards :443 to it after TLS."
  type        = number
}
