#!/usr/bin/env bash
set -euo pipefail

crate_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
exec cargo run --quiet --locked --manifest-path "$crate_dir/Cargo.toml" \
  -p emela-codegen-emb --example flow_ir_inventory -- "$@"
