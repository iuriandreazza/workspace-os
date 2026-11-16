---
title: "S1: Bonsai 8B running locally, measured"
type: report
status: done
created: 2026-11-15
updated: 2026-11-15
tags: [slm, bonsai, llama-cpp, benchmark, tool-calling]
---

# S1: Bonsai 8B running locally, measured

Roadmap item S1. Everything below was measured on the development machine in
one session; nothing is copied from the vendor.

## Setup

| | |
|---|---|
| Hardware | Apple M1 (4 performance + 4 efficiency cores), 8 GB unified memory, macOS (Darwin 27.2.0) |
| Runtime | upstream `llama.cpp` 1.0.0 from Homebrew, build 12359, commit `b1b770d90`, Metal backend |
| Model file | `Bonsai-8B-Q1_0.gguf` from `prism-ml/Bonsai-8B-gguf`, 1,158,654,496 bytes |
| SHA-256 | `284a335aa3fb2ced3b1b01fcb40b08aa783e3b70832767f0dd2e3fdfa134bd54` |

Upstream `llama.cpp` loads the Q1_0 file and runs it on Metal; the PrismML fork
was not needed. The weights and the binary are not in this repository.

Metal reports `MTL0: Apple M1 (5461 MiB)` as the GPU memory budget on this
8 GB machine. That budget decides what fits (see 32k below).

## Reproduce

```bash
brew install llama.cpp
mkdir -p ~/models/bonsai-8b && cd ~/models/bonsai-8b
curl -L -o Bonsai-8B-Q1_0.gguf \
  https://huggingface.co/prism-ml/Bonsai-8B-gguf/resolve/main/Bonsai-8B-Q1_0.gguf
llama-server --version
```

8k context (default f16 cache):

```bash
llama-server -m Bonsai-8B-Q1_0.gguf -c 8192 --parallel 1 --port 8089 --jinja -ngl 99
```

32k context (needs a quantized cache on this machine, see below):

```bash
llama-server -m Bonsai-8B-Q1_0.gguf -c 32768 --parallel 1 --port 8092 --jinja -ngl 99 \
  -fa on -ctk q8_0 -ctv q8_0
```

Measurement: for each run a prompt of repeated filler text was tokenized,
trimmed and sent to `/completion` with `cache_prompt: false`, `temperature: 0`,
`n_predict: 128`, streaming. Time to first token (TTFT) is the wall time from
sending the request to the first streamed content. Tokens per second come from
the server's own `timings`. Memory comes from `ps -o rss=` and `footprint <pid>`
(`phys_footprint`) on the server process after the run.

## Results

Prompt tokens are the real count the server processed (retokenizing the
trimmed text gave fewer tokens than targeted).

| Context | KV cache | Prompt tokens | TTFT | Prompt processing | Generation | Memory after run |
|---|---|---|---|---|---|---|
| 8k | f16 | 102 | 1.09 s | 93.5 tok/s | 28.4 tok/s | footprint 1303 MB, RSS 2256 MB (whole 8k run) |
| 8k | f16 | 4,899 | 47.3 s | 103.7 tok/s | 19.7 tok/s | same run |
| 32k | q8_0 | 102 | 1.27 s | 81.4 tok/s | 26.9 tok/s | idle footprint 2535 MB |
| 32k | q8_0 | 22,774 | 359.5 s | 63.4 tok/s | 7.9 tok/s | footprint 2620 MB, RSS 3601 MB |

Other measurements, 8k context, 1,233-token prompt:

| Backend | TTFT | Prompt processing | Generation | Footprint |
|---|---|---|---|---|
| Metal (`-ngl 99`) | 10.4 s | 119 tok/s | 24.5 tok/s | 1262 MB |
| CPU only (`-ngl 0`) | 42.9 s | 28.7 tok/s | 7.3 tok/s | 2274 MB |

### What the numbers say

- Short requests are usable: about one second to the first token and 25 to 28
  tokens per second. That fits the intent shell's budget in ADR 0002
  (hundreds of milliseconds to seconds) for short prompts.
- Prompt processing is the bottleneck, around 100 tok/s on this M1. A prompt of
  a few thousand tokens costs tens of seconds, and 22k tokens costs six
  minutes. The shell must keep prompts short, or reuse a cached prefix.
- Generation slows as the context fills: 28 tok/s nearly empty, 19.7 tok/s at
  about 5k tokens, 7.9 tok/s at about 23k tokens.
- `footprint` does not fully count GPU buffers, so it understates the total.
  RSS and footprint are both given; the real need lies between them. The
  vendor's memory estimates were not verified here.

### 32k does not fit with the default cache

With `-c 32768` and the default f16 KV cache, the server loads, then fails on
the first long prompt with
`Insufficient Memory (kIOGPUCommandBufferCallbackErrorOutOfMemory)`. The cache
alone is about 4.7 GB (idle footprint 4710 MB), plus 1.1 GB of weights, which
is more than the 5.4 GB Metal budget of an 8 GB machine. Quantizing the cache
to q8_0 (needs flash attention) brings idle footprint to 2.5 GB and the run
completes. The 8k and 32k rows therefore use different cache types and are not
a pure comparison of context size. The 32k run filled 22.9k of the 32k
positions, not all of them.

Not measured: output quality with a q8_0 cache, a full 32k prompt, and 64k.

## Tool call, end to end

The model received one tool definition and a user request. The server ran with
`--jinja`, so the GGUF's own chat template (Qwen3 style, with a `# Tools`
section) built the prompt. Server: 8k context, f16 cache, temperature 0.

Request 1 (`POST /v1/chat/completions`):

```json
{"temperature":0,"max_tokens":256,
 "messages":[{"role":"user","content":"List the five largest files in /var/log."}],
 "tools":[{"type":"function","function":{
   "name":"list_largest_files",
   "description":"List the largest files in a directory, sorted by size, descending.",
   "parameters":{"type":"object","properties":{
     "path":{"type":"string","description":"Directory to scan"},
     "count":{"type":"integer","description":"How many files to return"}},
     "required":["path","count"]}}}]}
```

Response 1 (trimmed to the relevant fields; 200 prompt tokens, 29 generated,
121 tok/s prompt, 28.1 tok/s generation):

```json
{"choices":[{"finish_reason":"tool_calls","index":0,"message":{
  "role":"assistant","content":"",
  "tool_calls":[{"type":"function","id":"5p0JVkCU0nzIhC7R4TguNwCmWDWwzt5f",
    "function":{"name":"list_largest_files",
                "arguments":"{\"path\": \"/var/log\", \"count\": 5}"}}]}}]}
```

The model chose the right tool and produced valid, schema-conforming
arguments (`path` string, `count` integer). The call was not executed; a fake
result was fed back to close the loop. Request 2 appended the assistant
message and a `tool` message:

```json
{"role":"tool","tool_call_id":"5p0JVkCU0nzIhC7R4TguNwCmWDWwzt5f",
 "content":"[{\"path\":\"/var/log/system.log\",\"bytes\":9123456},{\"path\":\"/var/log/install.log\",\"bytes\":512000}]"}
```

Response 2 (89 tokens, 28.9 tok/s):

```text
The five largest files in `/var/log` are:

1. `/var/log/system.log` - 9,123,456 bytes
2. `/var/log/install.log` - 512,000 bytes

Note: Only two files were returned, which might be due to the directory
containing fewer than five files. Let me know if you'd like to check another
directory or need additional details!
```

The model read the tool result correctly and noticed that fewer than five
files came back.

### What this does and does not show

It shows that Bonsai 8B, through upstream `llama.cpp` and the model's own
template, emits a structured tool call for one simple tool with two typed
arguments, and uses the result. It is one request at temperature 0, so it says
nothing about reliability across intents, several tools, ambiguous requests,
or malformed output. That is S4's job. S2 decides how output is constrained to
a schema (for example with grammar-constrained decoding in `llama.cpp`) instead
of relying on the model alone.

## Consequences for the next items

- S2 and S3 should assume short prompts: the action schema and any system text
  count against a prompt-processing rate of about 100 tok/s.
- Plan for an 8k context on 8 GB machines; 32k needs a quantized cache and is
  slow to fill.
- Reusing a cached prompt prefix (`cache_prompt`) matters: the second request
  above reprocessed only 87 of 283 tokens.
