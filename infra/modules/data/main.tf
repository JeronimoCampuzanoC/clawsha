# The seven tables of the master, exactly as crates/master/src/db/tables.rs
# defines them. The master runs with MASTER_CREATE_TABLES=false in AWS, so
# these must exist before it starts.

locals {
  tables = {
    users       = { hash = ["user_id", "S"], range = null }
    usernames   = { hash = ["username", "S"], range = null }
    inodes      = { hash = ["parent_id", "S"], range = ["name", "S"] }
    versions    = { hash = ["inode_id", "S"], range = ["version_no", "N"] }
    blocks      = { hash = ["version_id", "S"], range = ["idx", "N"] }
    cluster     = { hash = ["pk", "S"], range = null }
    path_leases = { hash = ["inode_id", "S"], range = null }
  }
}

resource "aws_dynamodb_table" "this" {
  for_each = local.tables

  name         = "${var.table_prefix}${each.key}"
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = each.value.hash[0]
  range_key    = each.value.range == null ? null : each.value.range[0]

  attribute {
    name = each.value.hash[0]
    type = each.value.hash[1]
  }

  dynamic "attribute" {
    for_each = each.value.range == null ? [] : [each.value.range]
    content {
      name = attribute.value[0]
      type = attribute.value[1]
    }
  }

  # versions: the GC lists versions by status, oldest first.
  dynamic "attribute" {
    for_each = each.key == "versions" ? [["status", "S"], ["updated_at", "N"]] : []
    content {
      name = attribute.value[0]
      type = attribute.value[1]
    }
  }

  dynamic "global_secondary_index" {
    for_each = each.key == "versions" ? ["by_status"] : []
    content {
      name            = global_secondary_index.value
      hash_key        = "status"
      range_key       = "updated_at"
      projection_type = "ALL"
    }
  }

  point_in_time_recovery {
    enabled = true
  }
}
