#!/usr/bin/env bash
# Runs the intent shell on the host. The workspace builds for the bare-metal
# target by default, which std code cannot use, so this passes the host target.
# Arguments go to the shell: ./scripts/intent-shell.sh --administrator
set -euo pipefail

cd "$(dirname "$0")/.."
host_target="$(rustc -vV | sed -n 's/^host: //p')"
exec cargo run --quiet -p intent-shell --target "$host_target" -- "$@"
