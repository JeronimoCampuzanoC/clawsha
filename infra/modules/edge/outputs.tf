output "public_nlb_dns" {
  value = aws_lb.public.dns_name
}

output "api_url" {
  value = "https://${aws_lb.public.dns_name}"
}

output "api_ca_pem" {
  description = "The self-signed certificate; clients trust the API with it."
  value       = tls_self_signed_cert.api.cert_pem
}
