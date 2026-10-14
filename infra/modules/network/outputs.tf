output "vpc_id" {
  value = aws_vpc.this.id
}

output "vpc_cidr" {
  value = aws_vpc.this.cidr_block
}

output "public_subnet_ids" {
  value = aws_subnet.public[*].id
}

output "master_subnet_ids" {
  value = aws_subnet.master[*].id
}

output "worker_subnet_ids" {
  value = aws_subnet.worker[*].id
}
