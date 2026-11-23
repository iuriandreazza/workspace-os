#!/usr/bin/env bash
# Runs the intent shell on the host. The workspace builds for the bare-metal
# target by default, which std code cannot use, so this passes the host target.
# The session starts in $HOME; a request may name a path (~/Downloads, films, /etc)
# or change directory ("go to the Documents folder"), ADR 0018.
# After each result the model writes a short reply in the tone of a voice
# SOUL and personality (ADRs 0021 and 0028, docs/s8-soul-format.md); --no-reply
# turns it off, --soul FILE names the SOUL (default ~/.config/workspace-os/soul)
# and --personality FILE names the personality (default beside it).
# Arguments go to the shell: ./scripts/intent-shell.sh --administrator
set -euo pipefail

cd "$(dirname "$0")/.."
host_target="$(rustc -vV | sed -n 's/^host: //p')"
exec cargo run --quiet -p intent-shell --target "$host_target" -- "$@"
