variable "name" {
  type = string
}

variable "vpc_id" {
  type = string
}

variable "subnet_ids" {
  description = "The public subnets, one per AZ."
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
