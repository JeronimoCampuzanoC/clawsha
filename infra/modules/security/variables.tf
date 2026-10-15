variable "name" {
  type = string
}

variable "vpc_id" {
  type = string
}

variable "vpc_cidr" {
  type = string
}

variable "gateway_port" {
  description = "Port the gateway container listens on."
  type        = number
}
