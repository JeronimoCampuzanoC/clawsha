output "masters_internal_dns" {
  description = "Internal NLB: MASTER_ADDR for workers and gateways (port 50051)."
  value       = module.control.internal_nlb_dns
}

output "public_dns" {
  description = "Public NLB of the gateways (port 443); null until gateway_image is set."
  value       = one(module.edge[*].public_nlb_dns)
}

output "dynamodb_tables" {
  value = module.data.table_names
}

output "worker_asg" {
  value = module.workers.asg_name
}
