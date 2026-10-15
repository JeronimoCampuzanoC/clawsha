variable "name" {
  description = "Prefix for every resource name."
  type        = string
  default     = "dfsha"
}

variable "region" {
  description = "The master has us-east-1 fixed in its code."
  type        = string
  default     = "us-east-1"
}

variable "azs" {
  type    = list(string)
  default = ["us-east-1a", "us-east-1b"]
}

variable "vpc_cidr" {
  type    = string
  default = "10.0.0.0/16"
}

variable "single_nat_gateway" {
  description = "true = one NAT for both AZs (saves about 33 USD/month); false = one per AZ, as in the diagram."
  type        = bool
  default     = false
}

variable "instance_profile_name" {
  description = "Learner Lab does not allow creating IAM roles; use the one it provides."
  type        = string
  default     = "LabInstanceProfile"
}

variable "table_prefix" {
  description = "Same value as MASTER_TABLE_PREFIX."
  type        = string
  default     = "dfsha_"
}

# ----- images (built and pushed by CI) ---------------------------------------

variable "master_image" {
  description = "e.g. ghcr.io/<org>/dfsha-master:latest"
  type        = string
}

variable "worker_image" {
  description = "e.g. ghcr.io/<org>/dfsha-worker:latest"
  type        = string
}

variable "gateway_image" {
  description = "Empty = do not create the gateways or the public NLB yet."
  type        = string
  default     = ""
}

# ----- sizes -----------------------------------------------------------------

variable "master_instance_type" {
  type    = string
  default = "t3.small"
}

variable "gateway_instance_type" {
  type    = string
  default = "t3.small"
}

variable "worker_instance_type" {
  type    = string
  default = "t3.micro"
}

variable "worker_min" {
  description = "Two per AZ, as in the diagram."
  type        = number
  default     = 4
}

variable "worker_max" {
  description = "Learner Lab denies (SCP) any ASG whose max size is above 6."
  type        = number
  default     = 6
}

variable "worker_data_volume_gb" {
  type    = number
  default = 10
}

variable "worker_quota_bytes" {
  description = "Quota each worker announces; a bit below the volume size (default 9 GiB)."
  type        = number
  default     = 9663676416
}

variable "scale_out_percent" {
  type    = number
  default = 70
}

variable "worker_cpu_scale_out_percent" {
  type    = number
  default = 70
}

variable "gateway_port" {
  description = "Port the gateway container listens on (GATEWAY_ADDR)."
  type        = number
  default     = 3000
}
