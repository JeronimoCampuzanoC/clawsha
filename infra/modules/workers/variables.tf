variable "name" {
  type = string
}

variable "subnet_ids" {
  description = "The private worker subnets, one per AZ."
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
  description = "Rendered infra/scripts/worker.sh.tftpl."
  type        = string
}

variable "data_volume_gb" {
  description = "Size of the EBS volume mounted on /data, where the blocks live."
  type        = number
}

variable "min_size" {
  type = number
}

variable "max_size" {
  type = number
}

variable "scale_out_percent" {
  description = "ClusterUsedPercent above which one more worker is added."
  type        = number
}

variable "cpu_scale_out_percent" {
  description = "Average worker CPU above which one more worker is added."
  type        = number
}
