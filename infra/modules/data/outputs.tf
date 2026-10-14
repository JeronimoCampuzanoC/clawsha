output "table_names" {
  value = [for t in aws_dynamodb_table.this : t.name]
}
