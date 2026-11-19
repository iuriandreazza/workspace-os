#!/usr/bin/env bash
# Runs the intent shell on the host. The workspace builds for the bare-metal
# target by default, which std code cannot use, so this passes the host target.
# The session starts in $HOME; a request may name a path (~/Downloads, films, /etc)
# or change directory ("go to the Documents folder"), ADR 0018.
# After each result the model writes a short reply in the tone of a voice
# profile (ADR 0021, docs/s8a-answer-voice.md); --no-reply turns it off and
# --voice-profile FILE names the profile file (default ~/.config/workspace-os/voice-profile).
# Arguments go to the shell: ./scripts/intent-shell.sh --administrator
set -euo pipefail

cd "$(dirname "$0")/.."
host_target="$(rustc -vV | sed -n 's/^host: //p')"
exec cargo run --quiet -p intent-shell --target "$host_target" -- "$@"
