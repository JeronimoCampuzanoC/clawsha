variable "name" {
  description = "Prefix for every resource name."
  type        = string
}

variable "vpc_cidr" {
  type = string
}

variable "azs" {
  description = "The two availability zones."
  type        = list(string)
}

variable "single_nat_gateway" {
  description = "true = one NAT for both AZs (cheaper); false = one NAT per AZ, as in the diagram."
  type        = bool
}
