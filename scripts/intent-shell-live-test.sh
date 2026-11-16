#!/usr/bin/env bash
# Runs the intent shell test that needs a real model. Not part of check.sh:
# CI has no model. Start llama-server first (docs/s3-intent-shell-prototype.md);
# INTENT_SHELL_MODEL=host:port overrides the default 127.0.0.1:8089.
set -euo pipefail

cd "$(dirname "$0")/.."
host_target="$(rustc -vV | sed -n 's/^host: //p')"
exec cargo test -p intent-shell --target "$host_target" --test live_model -- --ignored --nocapture
