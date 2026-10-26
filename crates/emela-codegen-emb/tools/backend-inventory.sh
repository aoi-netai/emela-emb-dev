#!/usr/bin/env bash
set -euo pipefail
crate_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
case "${1:-}" in
  "") exec cargo test --quiet --locked --offline --manifest-path "$crate_dir/Cargo.toml" -p emela-codegen-emb --test backend_inventory regenerate_backend_inventory -- --ignored ;;
  --check) exec cargo test --quiet --locked --offline --manifest-path "$crate_dir/Cargo.toml" -p emela-codegen-emb --test backend_inventory backend_inventory_matches_all_definition_ids_sources_and_tests ;;
  *) echo "usage: $0 [--check]" >&2; exit 2 ;;
esac
