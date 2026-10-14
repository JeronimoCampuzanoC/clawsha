output "internal_nlb_dns" {
  description = "Address of the leader for workers and gateways (port 50051)."
  value       = aws_lb.masters.dns_name
}

output "asg_names" {
  value = aws_autoscaling_group.master[*].name
}
