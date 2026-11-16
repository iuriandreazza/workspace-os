#!/usr/bin/env bash
# Runs every measurement of docs/s4-model-evaluation.md, one model server at a
# time, and writes the evidence under eval/results/.
#
#   LLAMA_SERVER=/path/to/llama-server \
#   BONSAI_8B=~/models/bonsai-8b/Bonsai-8B-Q1_0.gguf \
#   BONSAI_2_9B=~/models/bonsai-2-9b/Ternary-Bonsai-2-9B-Q2_0.gguf \
#   BONSAI_1_27B=.../Bonsai-27B-Q1_0.gguf GEMMA_4_12B=.../gemma-4-12B-it-QAT-Q4_0.gguf \
#   ./eval/run-matrix.sh [SECTION...]
#
# Sections (default: all but `thinking`): bonsai8b bonsai2 bonsai27b gemma
# sampling thinking. A model whose file variable is unset is skipped. It needs
# `uv` (for the jsonschema package) and `curl`.
set -euo pipefail

cd "$(dirname "$0")/.."

: "${LLAMA_SERVER:?set LLAMA_SERVER to a llama-server built from llama.cpp b11555 or newer}"
port=8199
server_pid=""
server_log="$(mktemp)"
failed=0

stop_server() {
    if [ -n "$server_pid" ]; then
        kill "$server_pid" 2>/dev/null || true
        wait "$server_pid" 2>/dev/null || true
        server_pid=""
    fi
}
trap 'stop_server; rm -f "$server_log"' EXIT

# Something already answering on the port would be measured in place of the model asked for.
if curl -sf "http://127.0.0.1:$port/health" >/dev/null 2>&1; then
    echo "run-matrix: something already answers on port $port; stop it first" >&2
    exit 1
fi

# serve MODEL_FILE CONTEXT: starts llama-server on $port and waits until it answers.
serve() {
    stop_server
    server_flags="llama-server -m $(basename "$1") -c $2 --parallel 1 --port $port --jinja -ngl 99"
    "$LLAMA_SERVER" -m "$1" -c "$2" --parallel 1 --port "$port" --jinja -ngl 99 >"$server_log" 2>&1 &
    server_pid=$!
    local waited=0
    until curl -sf "http://127.0.0.1:$port/health" >/dev/null; do
        if ! kill -0 "$server_pid" 2>/dev/null || [ "$waited" -ge 300 ]; then
            echo "run-matrix: the server did not start for $1; its log:" >&2
            tail -20 "$server_log" >&2
            exit 1
        fi
        sleep 2
        waited=$((waited + 2))
    done
    model_file="$1"
    # The server that answers must be the one just started, not another on the same port.
    if ! curl -sf "http://127.0.0.1:$port/props" | grep -qF "$(basename "$1")"; then
        echo "run-matrix: the server on port $port is not serving $(basename "$1")" >&2
        exit 1
    fi
}

# measure LABEL MODE [run_eval.py options...]
measure() {
    local label="$1" mode="$2"
    shift 2
    if [ -f "eval/results/$label/results.jsonl" ]; then
        if python3 -I eval/score.py complete "eval/results/$label"; then
            echo "skipping $label: eval/results/$label already holds a complete run"
            return 0
        fi
        echo "run-matrix: eval/results/$label holds an incomplete or failed run; remove it and run again" >&2
        failed=1
        return 0
    fi
    uv run --quiet --with jsonschema python eval/run_eval.py --url "http://127.0.0.1:$port" \
        --mode "$mode" --label "$label" --model-file "$model_file" --server-binary "$LLAMA_SERVER" --server-flags "$server_flags" "$@" | tail -1 || failed=1
}

# Both modes of a model, thinking off, one retry in the constrained mode; and the
# tools mode again with the harness deriving depends_on from the references.
both_modes() {
    measure "$1-grammar" grammar --no-thinking --retry
    measure "$1-tools" tools --no-thinking
    measure "$1-tools-derived" tools --no-thinking --derive-depends
}

selected() {
    [ "$#" -gt 0 ] || return 1
    local wanted="$1"
    shift
    for section in "${SECTIONS[@]}"; do
        [ "$section" = "$wanted" ] && return 0
    done
    return 1
}

SECTIONS=("$@")
[ "${#SECTIONS[@]}" -gt 0 ] || SECTIONS=(bonsai8b bonsai2 bonsai27b gemma sampling)
for section in "${SECTIONS[@]}"; do
    case "$section" in
        bonsai8b | bonsai2 | bonsai27b | gemma | sampling | thinking) ;;
        *) echo "run-matrix: unknown section '$section'" >&2; exit 1 ;;
    esac
done

if selected bonsai8b && [ -n "${BONSAI_8B:-}" ]; then
    serve "$BONSAI_8B" 8192
    # As the shell sends it: no chat template arguments. Twice, to show the run repeats.
    measure bonsai8b-grammar-shipped grammar --retry
    measure bonsai8b-grammar-shipped-repeat grammar --retry
    both_modes bonsai8b
fi

if selected bonsai2 && [ -n "${BONSAI_2_9B:-}" ]; then
    serve "$BONSAI_2_9B" 8192
    # As the shell sends it: the template's own default, which is thinking on.
    measure bonsai2-9b-grammar-shipped grammar --retry
    both_modes bonsai2-9b
fi

if selected bonsai27b && [ -n "${BONSAI_1_27B:-}" ]; then
    serve "$BONSAI_1_27B" 8192
    both_modes bonsai1-27b
fi

if selected gemma && [ -n "${GEMMA_4_12B:-}" ]; then
    serve "$GEMMA_4_12B" 8192
    both_modes gemma4-12b
fi

# Bonsai 2 9B with the sampler the vendor recommends for non-thinking mode, temperature 0.7: not repeatable.
if selected sampling && [ -n "${BONSAI_2_9B:-}" ]; then
    serve "$BONSAI_2_9B" 8192
    vendor='{"top_p":0.8,"top_k":20,"min_p":0,"presence_penalty":1.5,"repeat_penalty":1.0,"repeat_last_n":64}'
    measure bonsai2-9b-tools-vendor-nothink tools --no-thinking --temperature 0.7 --sampling "$vendor"
    measure bonsai2-9b-grammar-vendor-nothink grammar --no-thinking --temperature 0.7 --sampling "$vendor"
fi

# Bonsai 2 9B as the vendor runs it by default, thinking on. Slow: thousands of tokens per request.
if selected thinking && [ -n "${BONSAI_2_9B:-}" ]; then
    serve "$BONSAI_2_9B" 16384
    vendor='{"top_p":0.95,"top_k":20,"min_p":0,"presence_penalty":1.0,"repeat_penalty":1.0,"repeat_last_n":64}'
    measure bonsai2-9b-tools-thinking tools --temperature 1.0 --sampling "$vendor" --max-tokens 6000
fi

[ "$failed" -eq 0 ] || { echo "run-matrix: a run failed (see above); its directory was kept" >&2; exit 1; }
