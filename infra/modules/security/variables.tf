variable "name" {
  type = string
}

variable "vpc_id" {
  type = string
}

variable "vpc_cidr" {
  type = string
}

variable "frontend_port" {
  description = "Port nginx (frontend + /api proxy) publishes on the gateway nodes."
  type        = number
}
