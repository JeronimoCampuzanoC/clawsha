# Remote state in S3, so the state survives between Learner Lab sessions.
# Create the bucket once by hand, then uncomment and run `terraform init`.
#
# terraform {
#   backend "s3" {
#     bucket = "<your-bucket>"
#     key    = "dfsha/terraform.tfstate"
#     region = "us-east-1"
#   }
# }
