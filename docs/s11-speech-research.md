---
title: "S11-1 research: local speech recognition and synthesis, their licences and the audio path"
type: research
status: done
created: 2026-11-23
updated: 2026-11-23
tags: [research, speech, asr, tts, voice, licence, audio, s11]
---

# S11-1 research: local speech recognition and synthesis, their licences and the audio path

Roadmap item S11-1. Input to ADR 0031, which settles what this document only
lays out. It does not reopen ADR 0002 (the model proposes, deterministic code
decides), ADR 0006 (the spoken voice belongs to each user's personality), ADR
0007 (root confirmation) or ADR 0028 (the personality file). Voice confirmation
of root actions is S11b and is out of scope.

## Method and limits

Every statement carries one of three labels, as in `docs/b1-boot-model-research.md`:

- **source**: read in this session, with the link or command that was read.
- **measured**: run on this machine, with the command in "Commands".
- **vendor**: a figure or claim from the project that makes the thing; not
  checked here.
- **unverified**: recalled, or seen only in a search result. Do not build a
  decision on it.

How the sources were read:

- Licence files, READMEs and model cards were fetched verbatim with `curl`
  (GitHub raw files, the Hugging Face `raw/main` files and `api/models`
  metadata, the crates.io API) into a scratch directory and read there. Search
  results were used to find sources, never as a source.
- The licence a model card or API field declares is trusted as written. This
  is a reading of the licence text by the project, not legal advice (the same
  caveat as ADR 0017).
- Nothing downloaded was executed except inside a throw-away Python virtual
  environment (`uv venv`, wheels from PyPI) in the session scratch directory;
  nothing was installed system-wide. Nothing was added to the repository.
- The microphone was never opened.
- No file over about 500 MB was downloaded. Candidates above that size are
  marked "not measured" for that reason.

Machine: Apple M1, 8 GB, macOS 27.2 (`sysctl -n machdep.cpu.brand_string
hw.memsize`). Python 3.12.13 in the throw-away environment.

Pinned heads at the time of reading:

| Source | Pin |
|---|---|
| `ggml-org/whisper.cpp` | `96ec4d9823e9d0211944172da35f9f29134af990` |
| `OHF-Voice/piper1-gpl` | `39ac0a04ff01120b1cb7867d21723fa320305b03` |
| `rhasspy/piper` (archived) | `73c04d81d5590ecc46e522de3601ce7fb29fc2be` |
| `k2-fsa/sherpa-onnx` | `cdbcacf732271ca373c9f323cff3c6083ab50dbe` |
| `moonshine-ai/moonshine` | `d8d8fa29d05ca67441a7719e70099f9f48917a93` |
| `hexgrad/kokoro` | `dfb907a02bba8152ca444717ca5d78747ccb4bec` |
| `espeak-ng/espeak-ng` | `ee9fa7cc4704d683020526f47a9655f14c8d5f4c` |
| `resemble-ai/chatterbox` | `ab55923c27757c90cfcc407baa57dc25a7d25217` |
| `snakers4/silero-vad` | `63d4a2a7bfce8cd9bb39ed8fa0afcb5236d73e01` |
| `QwenLM/Qwen3-TTS` | `022e286b98fbec7e1e916cb940cdf532cd9f488e` |
| HF `rhasspy/piper-voices` | `e9c8a39444359388adaa94255becb37a5a2b8e51` |
| HF `ggerganov/whisper.cpp` (ggml models) | `5359861c739e955e79d9a303bcbc70fb988958b1` |
| HF `hexgrad/Kokoro-82M` | `f3ff3571791e39611d31c381e3a41a3af07b4987` |
| HF `nvidia/parakeet-tdt-0.6b-v3` | `541d1f99c6b0c3cd0b11a95167540bb8edefd82b` |
| HF `openai/whisper-large-v3-turbo` | `41f01f3fe87f28c78e2fbf8b568835947dd65ed9` |

## 1. Which licence rules decide the architecture

Three facts, all from files in this repository or from the licence texts, fix
the shape of any answer before a single model is compared.

1. **ADR 0005 allows only GPL-2.0-compatible code to be linked.** "Code that is
   GPL-3.0, AGPL-3.0 or Apache-2.0 only cannot be linked in" (ADR 0005,
   Consequences). `ACCEPTED` in `scripts/check-licences.py` lists MIT, BSD-2/3,
   ISC, Zlib, Unicode-3.0, Unlicense, BSL-1.0 and the two GPL-2.0 ids, and
   MPL-2.0 under ADR 0017. Apache-2.0 is not in it, so a crate whose manifest
   says `Apache-2.0` alone fails the gate.
2. **ADR 0008 already reads a GPL-3.0 program as acceptable "only as a
   separate program"** (the GRUB row). The same reading is used below for GPL-3.0
   code that is run as its own process and never linked into `intent-shell`.
3. **Model weights carry their own licence** (ADR 0005, last consequence), and
   for speech voices that licence is the licence of the recordings the voice
   was trained on, not of the engine (section 4).

Consequences, from the licences read in this session:

| Component | Declared licence | Linked into `intent-shell`? | As a separate process? |
|---|---|---|---|
| whisper.cpp | MIT ([LICENSE](https://github.com/ggml-org/whisper.cpp/blob/96ec4d9823e9d0211944172da35f9f29134af990/LICENSE), source) | yes, allowed | yes |
| `whisper-rs` 0.16.0 (crate that wraps it) | `Unlicense` (crates.io API, source) | yes, `Unlicense` is in `ACCEPTED`; builds whisper.cpp from source, so needs `cmake` (**unverified**) | n/a |
| Piper engine (`piper1-gpl`) | GPL-3.0 (`COPYING`, source) | **no**: GPL-3.0 | yes, as ADR 0008 reads GRUB |
| espeak-ng (phonemizer inside Piper; named as the pt-BR G2P of Kokoro) | "GPL version 3 or later" ([README](https://github.com/espeak-ng/espeak-ng/blob/ee9fa7cc4704d683020526f47a9655f14c8d5f4c/README.md), source) | **no** | yes |
| sherpa-onnx (and its Rust crate `sherpa-onnx` 2.0.3) | Apache-2.0 ([LICENSE](https://github.com/k2-fsa/sherpa-onnx/blob/cdbcacf732271ca373c9f323cff3c6083ab50dbe/LICENSE); crates.io says `Apache-2.0`) | **no**: Apache-2.0 alone | yes |
| Kokoro code (`kokoro`, `misaki`) | Apache-2.0 (GitHub API) | **no** | yes (Python) |
| NVIDIA NeMo-Speech.cpp (runtime for Parakeet) | Apache-2.0 (GitHub API) | **no** | yes |
| Qwen3-TTS code | Apache-2.0 (GitHub API) | **no** | yes (Python) |
| Silero VAD | MIT ([LICENSE](https://github.com/snakers4/silero-vad/blob/63d4a2a7bfce8cd9bb39ed8fa0afcb5236d73e01/LICENSE), source) | allowed | n/a |

Audio crates (crates.io API, **source**), each checked for the same gate:

| Crate | Version | Declared licence | Passes the gate? |
|---|---|---|---|
| `cpal` | 0.18.2 | `Apache-2.0` | **no** |
| `hound` (WAV) | 3.5.1 | `Apache-2.0` | **no** |
| `rodio` | 0.23.0 | `MIT OR Apache-2.0` | yes by itself, but it depends on `cpal` (**unverified**: dependency list not read) |
| `coreaudio-sys` | 0.2.18 | `MIT` | yes |
| `coreaudio-rs` | 0.14.2 | `MIT/Apache-2.0` | yes (MIT) |
| `alsa-sys` | 0.6.1 | `MIT` | yes |
| `alsa` | 0.12.1 | `Apache-2.0/MIT` | yes (MIT) |
| `pipewire` | 0.10.1 | `MIT` | yes |

So the obvious Rust route for audio, `cpal` plus `hound`, is closed by ADR 0005,
and every pt-BR synthesis engine found needs GPL-3.0 code (espeak-ng), which
must stay a separate process. The sections below are written under these
two limits.

## 2. Speech recognition (ASR)

The task's constraints: Brazilian Portuguese and English, local and offline, on
an M1 with 8 GB next to Bonsai 8B, and on Linux aarch64 and x86_64.

| Candidate | Code / weights licence | Languages | Streaming | Size on disk | Memory | Verdict |
|---|---|---|---|---|---|---|
| **whisper.cpp + Whisper `small` (ggml q5_1)** | MIT / MIT ([model card](https://huggingface.co/openai/whisper-large-v3-turbo/raw/main/README.md) declares `license: mit` for the Whisper family; the `ggerganov/whisper.cpp` HF repo metadata says `mit`) | multilingual, includes Portuguese (the CLI took `language="pt"` below) | no: utterance at a time. A `whisper-stream` example exists that re-runs a window every 0.5 s ([README](https://github.com/ggml-org/whisper.cpp/blob/96ec4d9823e9d0211944172da35f9f29134af990/README.md#real-time-audio-input-example)) | 190,085,487 bytes (HF API; measured file) | peak RSS 356 MiB (**measured**) | **candidate** |
| whisper.cpp + Whisper `base` q5_1 | same | same | no | 59,707,625 bytes | peak RSS 181 MiB (**measured**) | fallback; two errors on 2 sentences (below) |
| whisper.cpp + `large-v3-turbo` q5_0 | same | same | no | 574,041,195 bytes (HF API) | vendor table: ~3.9 GB for `large` ([README](https://github.com/ggml-org/whisper.cpp/blob/96ec4d9823e9d0211944172da35f9f29134af990/README.md#memory-usage)); no figure for turbo | **not measured** (over 500 MB; and its memory next to Bonsai is the open question) |
| Parakeet-TDT 0.6B v3 | runtime NeMo-Speech.cpp Apache-2.0; weights **CC-BY-4.0** (HF metadata, card) | 25 European languages, includes `pt`; the card warns that its Portuguese training data is European Portuguese while most benchmarks use Brazilian | no (long-audio batch model) | q8_0 gguf size not read | not read | **not measured**; weights usable as data with attribution; runtime separate process only; the card says it is "optimized to run on NVIDIA GPU-accelerated systems" |
| Moonshine | code MIT; most models MIT | the ASR list in the README has no Portuguese (English, Spanish, German, Mandarin, Japanese, Korean, Vietnamese, Tagalog, Ukrainian, Arabic) | yes | not read | not read | **unusable for pt-BR ASR**. Its non-English legacy non-streaming models are under a non-commercial licence ([LICENSE](https://github.com/moonshine-ai/moonshine/blob/d8d8fa29d05ca67441a7719e70099f9f48917a93/LICENSE)) |
| sherpa-onnx as the runtime for Whisper, Parakeet or others | Apache-2.0 | depends on model | yes for its streaming zipformers (none for pt-BR in the README list read) | n/a | n/a | an alternative runtime, separate process only; not needed if whisper.cpp serves |

Evidence for the claims in the table:

- **Whisper licence.** The `openai/whisper-large-v3-turbo` card declares
  `license: mit` and calls the model "the exact same model" as large-v3 with the
  decoder reduced from 32 to 4 layers (source). The licence of the smaller
  Whisper models (`small`, `base`) was **not read in their own cards**; the
  `ggerganov/whisper.cpp` repository holding the ggml conversions declares `mit`
  in its metadata (source). **Unverified** for the original `openai/whisper-small`
  card.
- **Sizes.** The ggml file sizes are the byte counts the Hugging Face tree API
  returned for `ggerganov/whisper.cpp`; for the two downloaded files they match
  the files on disk (59,707,625 and 190,085,487).
- **Metal.** The whisper.cpp README says "On Apple Silicon, the inference runs
  fully on the GPU via Metal" and lists Core ML for the encoder (source).
  `otool -L` on the `libwhisper.1.8.5.dylib` shipped in the `pywhispercpp`
  1.5.1 wheel lists `libggml-metal.0.13.1.dylib` (**measured**), so the Metal
  backend was linked in the run below.
- **Server.** whisper.cpp ships `examples/server` ("Simple http server. WAV
  Files are passed to the inference model via http requests", source:
  [README](https://github.com/ggml-org/whisper.cpp/blob/96ec4d9823e9d0211944172da35f9f29134af990/examples/server/README.md);
  directory listing from the GitHub contents API). It was **not run**.
- **VAD.** whisper.cpp supports Silero-VAD with `--vad` (source). Push-to-talk
  makes it optional.
- **Parakeet figures are the vendor's:** FLEURS Portuguese WER 4.76 and
  Multilingual LibriSpeech Portuguese WER 7.5, from the card's metadata block.
  The card also says audio up to 24 minutes on an A100 80 GB. Not checked here.

### What was measured (Whisper through `pywhispercpp`)

Two Brazilian Portuguese sentences were synthesised with the macOS voice
Luciana (`say -v Luciana`), converted to 16 kHz mono 16-bit WAV, and
transcribed with `language="pt"` and 4 threads.

| Model | Audio | Steady time (second run) | Real-time factor (time / audio) | Peak RSS | Transcript |
|---|---|---|---|---|---|
| base q5_1 | 5.43 s, 4.73 s | 0.22 s, 0.24 s | 0.041, 0.050 | 181 MiB | "…e mostra o tamanho…" (said: "mostre"); "…pasta de Daum Loudes." (said: "downloads") |
| small q5_1 | 5.43 s, 4.73 s | 0.63 s, 0.62 s | 0.116, 0.130 | 356 MiB | both sentences exact |

Limits of these numbers, all of which matter:

- The audio is **synthetic and clean**, two sentences, one voice. It says
  nothing about accuracy on a real microphone, accents, noise or other voices.
  It does show that `small` got both sentences right where `base` made two
  errors, which is a reason to start from `small`, not a result.
- Rows are the second run of each file. The first run of `base` took 0.92 s and
  a one-off model load of 13.24 s on a cold start (probably the Metal backend
  compiling; **unverified**). A later run of `small` took a load of 288.77 s
  while a Bonsai 8B server was resident and the machine was under memory
  pressure (system-wide free memory 27 % afterwards); its steady times were
  0.79 s and 0.66 s, RTF 0.146 and 0.139. That load time is a warning about
  starting the recogniser next to the 8B model on 8 GB, not a typical figure.
- `ru_maxrss` does not count GPU buffers (the same caveat as S1,
  `docs/s1-bonsai-8b-local-run.md`).
- The Bonsai 8B server that was resident during that run was **not started by
  this session** (this session's own `llama-server` failed to bind port 8089
  because another one already held it, `llama.log`). Its footprint was not
  measured. So "next to Bonsai 8B" is shown to work for `small` on one short
  run and no more: S1 gives 1303 MB footprint and 2256 MB RSS for Bonsai at 8k,
  and the Metal budget on this machine is 5461 MiB (S1, measured there).

Memory figures for the larger models in the whisper.cpp README (tiny ~273 MB,
base ~388 MB, small ~852 MB, medium ~2.1 GB, large ~3.9 GB) are the
**vendor's**. The measured RSS above is lower than the vendor's `small` figure
(852 MB); the files used are q5_1 quantised and the RSS excludes GPU buffers
(probable causes, not checked), so do not compare the two as like for like.

## 3. Speech synthesis (TTS) and how each gives more than one voice

The second requirement is that two users can hear two different voices, so how
a candidate provides distinct voices is part of the comparison.

| Candidate | Engine / weights licence | pt-BR | How it gives several voices | Cloning | Verdict |
|---|---|---|---|---|---|
| **Piper** (`piper1-gpl`) | engine GPL-3.0 (`COPYING`); the project "makes no license grant of its own over voice models" ([LICENSING.md](https://github.com/OHF-Voice/piper1-gpl/blob/39ac0a04ff01120b1cb7867d21723fa320305b03/docs/LICENSING.md)); each voice has its own `MODEL_CARD` | yes: `pt_BR` is in the supported languages; voices `cadu`, `edresson`, `faber`, `jeff` in `rhasspy/piper-voices` | **one voice per model file** (single speaker, 63,221,984 bytes each for `cadu`, `jeff`, `faber` medium; a few voices are multi-speaker). A different voice is a different file | no | **candidate**, separate process |
| Kokoro-82M | code Apache-2.0; weights Apache-2.0 (card) | yes: 3 voices (`pf_dora`, `pm_alex`, `pm_santa`), pt-BR G2P is espeak-ng `pt-br` (VOICES.md) | a fixed table of 54 voices in 8 languages inside one model (`voices.bin` in the sherpa packaging); chosen by speaker id | no (no encoder released: "Decoder only") | alternative; needs GPL-3.0 espeak-ng for pt-BR, so also a separate process; **not measured** (see below) |
| Chatterbox Multilingual V3 | code MIT; HF metadata `mit` | yes (`pt`; separate `pt-br` fine-tune linked in the README) | by reference audio, 0.5B parameters, about 2 GB per `t3` file (HF tree) | **yes, zero-shot** | **excluded by policy** (cloning, section 6). Also: the README says that as of August 2026 it is retired research, no longer maintained |
| Qwen3-TTS (0.6B / 1.7B) | code Apache-2.0; HF metadata `apache-2.0` | yes (Portuguese is in the 10 languages) | 9 premium timbres in `CustomVoice`; a natural-language voice description in `VoiceDesign`; reference audio in `Base` | **yes, 3-second** | **excluded by policy** for the cloning models. The 0.6B `Base` checkpoint is 1,829,344,272 bytes; the README's install uses PyTorch and GPU libraries (**not run**). Timbres are labelled with native languages, none read as pt-BR |
| Kyutai TTS 1.6B | weights CC-BY-4.0 | English and French only (HF metadata) | n/a | n/a | unusable for pt-BR |
| XTTS-v2 (Coqui) | HF metadata: `license: other`, `coqui-public-model-license` | — | — | yes | **unusable**: the Coqui Public Model Licence is non-commercial (the licence text itself was **not read**; the name and field are from HF metadata) |
| F5-TTS | HF metadata: `cc-by-nc-4.0` | — | — | yes | **unusable**: non-commercial weights |
| Supertonic 3 (`Supertone/supertonic-3`, in the sherpa release list) | HF metadata: `openrail` | multilingual list in the metadata (pt not confirmed; truncated) | not read | not read | **not accepted**: OpenRAIL carries use restrictions that were not read; would need a reading before use |
| macOS `say` | Apple system voices | yes: Luciana, Eddy, Flo, Grandma, Grandpa, Reed, Rocko, Sandy, Shelley (`say -v '?'`, **measured**) | system voices | no | **host-only convenience**: not redistributable, absent on Linux, absent in the OS. Used in this research only to make test audio |
| Moonshine TTS | "licensed under the terms listed in their own readmes" ([LICENSE](https://github.com/moonshine-ai/moonshine/blob/d8d8fa29d05ca67441a7719e70099f9f48917a93/LICENSE)) | Portuguese listed in the README | not read | not read | **unverified**: those readmes were not read |

### Piper's voices: the licence chain is the licence

This is the finding that decides which voices can ship.

- Piper's `LICENSING.md` says the `License` line of a `MODEL_CARD` names the
  licence of "that voice's own dataset only — not the datasets of any voice it
  was fine-tuned from", and that most voices are fine-tuned from another voice,
  so the chain must be walked down to a voice trained from scratch.
- It says the very common base voice `en_US-lessac-medium` was trained on a
  corpus whose licence "limits use to 'research and exploration of speech
  technologies' and excludes commercial use of the resulting synthesis", and that
  a large share of the voices descend from it (source). **Treated here as
  unusable**, in line with the task's rule about research-only weights.
- It says many voices were retrained "in late 2026" onto two permissive bases
  (`nl_BE-rdh-medium`, CC0; `en_US-hifi_tts_92-medium`, CC BY 4.0) **keeping the
  same names**, so "a copy downloaded before a voice was retrained is still the
  lessac-derived version" (source, `COMMERCIAL_USE.md`).
- **That is exactly what was found.** The three pt-BR medium voices in the
  current `rhasspy/piper-voices` `main` (commit `e9c8a394…`) have these
  `Training` lines (source, `MODEL_CARD` files):

  | Voice | Dataset licence | Training line |
  |---|---|---|
  | `pt_BR-cadu-medium` | CC0 | fine-tuned from the `rdh` base (CC0) |
  | `pt_BR-jeff-medium` | CC0 | fine-tuned from the `rdh` base (CC0) |
  | `pt_BR-faber-medium` | CC0 | fine-tuned from `hifi_tts_92` (CC BY 4.0: **attribution is owed**) |

  `COMMERCIAL_USE.md` lists all three, `cadu` and `jeff` in the `rdh` table and
  `faber` in the `hifi_tts_92` table. The same page lists `en_US-joe-medium`
  (`MODEL_CARD` read: CC0, from `rdh`) and `en_US-mike-medium` (listed; card
  not read).
- The **sherpa-onnx `tts-models` tarballs of the same voices carry the old
  chain.** `vits-piper-pt_BR-cadu-medium-int8`, `…-jeff-medium-int8` and
  `…-faber-medium-int8`, downloaded from the `k2-fsa/sherpa-onnx` release
  `tts-models`, each contain a `MODEL_CARD` whose Training line reads "Finetuned
  from U.S. English lessac voice (medium quality)" (**measured**: tarball
  SHA-256 prefixes `78f1caf0a74cc6cb`, `21a8883f9662c784`, `dbc8b1d7d729fd41`).
  The file names are the same as the retrained voices. Those copies are
  unusable for this project; only the files from the pinned `piper-voices`
  commit, with the card read, qualify.
- SHA-256 of the three `.onnx` files from `piper-voices` `main` at the time of
  reading (**measured**): `cadu` `d5a662e3f606d5a49dd29ba9ce90c304ae45dd9543f8b0c773bcb0de282499de`,
  `jeff` `82ddcdb01a0342f078bd89ce2815eda27185818d15e15f06fb9a72ac3bce94e1`,
  `faber` `75e913eb463db0b6ac7ddf68d3e4c9ac19ace36dc3b710a81039b448352b8ffc`.
- Piper itself says the engine "is intended for personal use and text to speech
  research only; we do not impose any additional restrictions on voice models"
  (VOICES.md, source). Read with LICENSING.md, that sentence says what the
  project intends for its own models, not a grant over somebody else's
  recordings; the licence of the engine is GPL-3.0.
- Two voices need a reading each before use, not decided here: the attribution
  text for the CC BY 4.0 Hi-Fi TTS base under `faber`, and the "unsettled legal
  question" Piper itself declines to answer, whether a trained model is a
  derivative work of its recordings (LICENSING.md). Both go into the legal
  review already open in the roadmap.

### Kokoro

- Weights: Apache-2.0, "trained exclusively on permissive/non-copyrighted audio
  data" including public-domain audio, Apache/MIT audio, and synthetic audio
  from closed TTS models, with some CC BY datasets (card, source). The licence is
  the model's own, so it is simpler than Piper's chain.
- It has three pt-BR voices, one female and two male (`pf_dora`, `pm_alex`,
  `pm_santa`). The card says support for non-English languages "may be absent or
  thin due to weak G2P and/or lack of training data", and that quality is poor on
  utterances shorter than 10 to 20 tokens (VOICES.md), which is the typical
  length of a short shell reply.
- The pt-BR front end is espeak-ng (VOICES.md: "espeak-ng `pt-br`"), so it
  carries GPL-3.0-or-later code too.
- **Not measured.** Section "What failed" explains why.

### What failed (measured, kept as findings)

TTS could **not** be measured on this machine, after a bounded effort:

1. `piper-tts` 1.8.0 (the macOS arm64 wheel) bundles `piper/espeak-ng-data` with
   a `phontab` file present, yet `espeakbridge.initialize()` ends the process
   with `Error processing file '/Users/runner/work/piper1-gpl/…/phontab': No
   such file or directory`, a CI build path, with the data directory passed
   explicitly (the data folder and its parent were both tried). **Unresolved**;
   it looks like a path baked into the wheel, but that is not confirmed.
2. `sherpa-onnx` 2.0.3 could not read the retrained Piper voices: with the
   voice's own `onnx`, tokens made from its `phoneme_id_map`, it stops with
   `Error when reading tokens at Line aɪ 161. size: 2`, because the retrained
   voices use multi-character phoneme tokens (Piper 1.5 merges vowel clusters;
   the model cards say "Requires Piper 1.5 or higher"). With the sherpa-packaged
   copy (old chain) it reports `phoneme_codepoints is empty, no lexicon, and no
   frontend available`. Whether sherpa-onnx 2.0 dropped the built-in espeak-ng
   frontend was **not checked** (its change log was not read).
3. Kokoro through sherpa-onnx 2.0.3: with the English lexicon every Portuguese
   word was skipped as out of vocabulary (the run produced about 2 s of audio
   for a 5 s sentence, so its real-time factor of 1.45 to 1.54 is **not a
   result**); without a lexicon it answered "No lexicon available". No valid
   Portuguese audio came out.

The consequence for the decision: the retrained Piper voices need the
`piper1-gpl` runtime, which is the GPL-3.0 engine run as its own process. That
matches the licence conclusion of section 1 and means there is **no measured
synthesis speed in this document**. The vendor publishes samples and says
Piper is "fast"; the figures were not read, so none is quoted. S11-2 must start
by getting one voice to run and measuring it.

## 4. How the audio reaches the shell

What exists on this machine (**measured**): `/usr/bin/afplay` and
`/usr/bin/say` are present. `ffmpeg`, `sox`/`rec`, `arecord` and `aplay` are
not. macOS has no built-in command-line recorder.

| Direction | macOS | Linux | New dependency |
|---|---|---|---|
| Playback | `afplay FILE.wav` (present; plays a file, so the reply is written to a temporary file in a directory only the user can open) | `aplay` (alsa-utils) or `pw-play` (PipeWire): **not run here**, **unverified** that they exist on a given distribution | none new on macOS; one system package on Linux |
| Capture | no tool: needs code. `coreaudio-sys` (MIT) passes the gate; the AudioQueue or AudioUnit API is C | `arecord` or `pw-record` as a subprocess, or `alsa` (Apache-2.0/MIT) / `pipewire` (MIT) crates | a small capture module behind a port |
| WAV file | 44-byte header written by hand; `hound` is Apache-2.0 only and fails the gate | same | none |

- The microphone permission on macOS (the system asks the application that
  opens the microphone) was **not tested**, because the microphone was never
  opened. Whether a terminal launched by an agent gets that prompt is
  **unverified**.
- Capture, speech recognition and synthesis only exchange two things with the
  shell: text, and audio files or buffers. That is the contract that can later
  be served by something other than a host library.

### Later, inside the OS

The OS has no audio driver, and audio belongs to a driver, as the roadmap says
for S12 ("needs an installer, userspace, storage and audio drivers"). Nothing
about audio hardware in QEMU or on the boards the project targets was read, so
nothing is claimed here. What can be said from the design: if the shell talks to
speech only through "text in, text out; audio in, audio out" ports, the host
processes (`whisper-server`, Piper) can later run as userspace services and only
the capture and playback adapters change, as S3c changed only the terminal
adapter.

## 5. Interfaces between the shell and the speech components

- whisper.cpp has an HTTP server (`examples/server`) taking WAV files, and
  Piper has an HTTP server (`python3 -m piper.http_server`, port 5000 by
  default, `--host` and `--port`; [API_HTTP.md](https://github.com/OHF-Voice/piper1-gpl/blob/39ac0a04ff01120b1cb7867d21723fa320305b03/docs/API_HTTP.md),
  source). Neither was run here (the Piper wheel failed, section 3; whisper.cpp
  was used as a library through `pywhispercpp`).
- Piper's own CLI says it "can be slow, however, because it needs to load the
  voice model each time" and recommends the web server for repeated use (CLI.md,
  source). That is a reason to run it as a server.
- The shell already talks to its model over local HTTP (`llama-server`, S1, S3),
  so the same pattern fits: a process per engine, on `127.0.0.1`, started by the
  user or the shell, with no network beyond the loopback.

## 6. The safety boundary

None of this is new policy; it is ADR 0002, 0006, 0007, 0021 and 0028 applied
to a new channel. The risks are specific to audio.

1. **Recognised speech is untrusted input exactly like typed text.** The
   recogniser can mis-hear, and any sound in the room (another person, a video,
   the system's own loudspeaker) can become text. The transcript is therefore
   shown to the user and goes down exactly the typed path: planner, resolver,
   broker, dry run, confirmation. It never goes to `Confirm` as an answer.
2. **The spoken answer is the ADR 0021 reply and nothing else.** The table, the
   verdicts, the root panel and every confirmation stay deterministic text
   (ADR 0021). A root action's exact-action display, which ADR 0007 requires
   before confirming, is never read aloud *in place of* the typed one.
   Voice confirmation of root actions is S11b.
3. **The system must not hear itself.** ADR 0007 says "this system has a
   loudspeaker and its own speech synthesis, so audio it plays could try to
   confirm an action". For S11 the rule is the simple one: no capture while
   the system is speaking, and push-to-talk rather than always listening,
   since ambient audio from other people is an injection source that typed
   input does not have.
4. **Cloning voices are excluded on purpose**, even where the licence allows
   them (Chatterbox is MIT, Qwen3-TTS is Apache-2.0). ADR 0007 names a cloned
   voice as the way to defeat a voiceprint; shipping a cloning model in the OS
   hands that tool to every user. A fixed list of voices is also what makes
   a personality's `voice` a closed vocabulary (ADR 0028).
5. **A voice, like the rest of a personality, is presentation.** It cannot grant
   a capability and one user's voice never applies to another user (ADR 0006).
   The same request must lead to the same validated actions under any voice;
   this is tested, as for presets (ADR 0021 section 4).
6. **Speech off, no model, recovery mode:** with speech off nothing of it is
   loaded; recovery mode runs the deterministic shell only (ADR 0007).

## 7. What could not be verified

- **Any synthesis speed or memory**, for Piper or Kokoro (section 3, "What
  failed").
- **The legal reading of voice licences**: whether a model is a derivative of
  its recordings (Piper declines to answer), and what exactly the CC BY 4.0
  attribution for Hi-Fi TTS requires in a shipped OS.
- **Whisper accuracy on Brazilian Portuguese** from anything but two synthetic
  sentences; no word-error-rate was measured and the Whisper paper's Portuguese
  figures were not read.
- **The licence text of the smaller Whisper models' own cards**, of the Coqui
  Public Model Licence, of OpenRAIL (Supertonic) and of the Moonshine TTS
  readmes: only metadata fields or summaries were read.
- **Parakeet** (CC-BY-4.0) speed and size on this machine: over the download
  threshold or not run; the runtime's licence is Apache-2.0 (separate process).
- **`whisper-server`** and **Piper's HTTP server**: not run.
- **Linux**: nothing was run on Linux aarch64 or x86_64. Platform support rests
  on the READMEs and on the sherpa-onnx README's architecture tables, which
  list aarch64 and x86_64 for Linux (line 32 lists `riscv64`; the other rows
  were not individually read).
- **Capture on macOS and Linux**, the microphone permission, and `aplay` /
  `pw-play` / `pw-record` availability.
- **Whisper `large-v3-turbo` next to Bonsai** on the 8 GB machine: not
  downloaded (574 MB q5_0 is over the threshold).
- **Which process owned port 8089** during the Bonsai test, and its footprint.
- **Not examined at all:** Vosk, Coqui STT, Mimic3, Coqui TTS other than XTTS-v2,
  Sesame CSM and Orpheus (English only per HF metadata), MOSS-TTS, and the
  Apple speech recognition frameworks.

## 8. Recommendation

For ADR 0031 to settle:

- **Recognition:** whisper.cpp, MIT, with the multilingual Whisper `small`
  (q5_1, about 190 MB), as a separate local HTTP process; `base` as a fallback
  when memory is tight. It was the only candidate run here, it runs on the M1
  GPU through Metal, and it is the only recogniser read that covers Portuguese
  and English with a licence that also allows linking later.
- **Synthesis:** Piper as a separate process (GPL-3.0 engine plus GPL-3.0+
  espeak-ng), with voices **only** from a pinned `piper-voices` commit whose
  `MODEL_CARD` chain was read: for pt-BR `cadu`, `jeff` (CC0) and `faber`
  (CC0 plus the CC BY 4.0 attribution); for English `joe`. Distinct voices
  come from distinct files.
- **Not chosen, and why:** Kokoro (a good second option licence-wise, but
  espeak-ng again, three pt-BR voices, a warning about short utterances and no
  verified runtime here), any cloning model (policy), Moonshine (no pt-BR ASR),
  XTTS-v2 and F5-TTS (non-commercial), Parakeet (not measured, European
  Portuguese training data, GPU-oriented).
- **Audio:** playback by a system tool, capture by a small module over
  platform sys crates that pass the gate; the first S11-2 task is to get one
  Piper voice and one `whisper-server` request running, and to measure both.

Commands used are in the next section.

## 9. Commands

Everything ran in `$SCRATCH` (the session scratch directory); nothing
system-wide.

```bash
uv venv venv
uv pip install --python venv/bin/python piper-tts pywhispercpp sherpa-onnx soundfile numpy onnx
# versions: piper-tts 1.8.0, pywhispercpp 1.5.1 (libwhisper 1.8.5, ggml 0.13.1), sherpa-onnx 2.0.3, onnxruntime 1.32.0

# whisper models (MIT), from the ggerganov/whisper.cpp HF repo
curl -sSL -o ggml-base-q5_1.bin  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base-q5_1.bin
curl -sSL -o ggml-small-q5_1.bin https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin
shasum -a 256 ggml-base-q5_1.bin ggml-small-q5_1.bin     # printed prefixes 422f1ae452ade6f3, ae85e4a935d7a567

# test audio: macOS voice, 16 kHz mono
say -v Luciana -o t1.aiff "Liste os cinco maiores arquivos da pasta Documentos e mostre o tamanho de cada um."
say -v Luciana -o t2.aiff "Apague os arquivos temporários antigos da minha pasta de downloads."
afconvert -f WAVE -d LEI16@16000 -c 1 t1.aiff t1.wav     # and t2

# the measurement (pywhispercpp.model.Model, n_threads=4, language="pt", wall time per transcribe call)
venv/bin/python -I asr_bench.py ggml-small-q5_1

otool -L venv/lib/python3.12/site-packages/pywhispercpp/.dylibs/libwhisper.1.8.5.dylib   # lists libggml-metal

# Piper voices: current main of rhasspy/piper-voices
curl -sSL -O https://huggingface.co/rhasspy/piper-voices/resolve/main/pt/pt_BR/cadu/medium/pt_BR-cadu-medium.onnx
curl -sSL https://huggingface.co/rhasspy/piper-voices/raw/main/pt/pt_BR/cadu/medium/MODEL_CARD
# the failing runs: PiperVoice.load(...) and piper.espeakbridge.initialize(<abs path to piper/espeak-ng-data>)
```

The throw-away environment and the bench scripts are in the session scratch
directory, not in the repository.

## Sources

Read in this session (the links are the pinned ones where a commit is named):

- [whisper.cpp README](https://github.com/ggml-org/whisper.cpp/blob/96ec4d9823e9d0211944172da35f9f29134af990/README.md), [LICENSE](https://github.com/ggml-org/whisper.cpp/blob/96ec4d9823e9d0211944172da35f9f29134af990/LICENSE), [server example](https://github.com/ggml-org/whisper.cpp/blob/96ec4d9823e9d0211944172da35f9f29134af990/examples/server/README.md)
- [piper1-gpl README](https://github.com/OHF-Voice/piper1-gpl/blob/39ac0a04ff01120b1cb7867d21723fa320305b03/README.md), [COPYING](https://github.com/OHF-Voice/piper1-gpl/blob/39ac0a04ff01120b1cb7867d21723fa320305b03/COPYING), [VOICES](https://github.com/OHF-Voice/piper1-gpl/blob/39ac0a04ff01120b1cb7867d21723fa320305b03/docs/VOICES.md), [LICENSING](https://github.com/OHF-Voice/piper1-gpl/blob/39ac0a04ff01120b1cb7867d21723fa320305b03/docs/LICENSING.md), [COMMERCIAL_USE](https://github.com/OHF-Voice/piper1-gpl/blob/39ac0a04ff01120b1cb7867d21723fa320305b03/docs/COMMERCIAL_USE.md), [CLI](https://github.com/OHF-Voice/piper1-gpl/blob/39ac0a04ff01120b1cb7867d21723fa320305b03/docs/CLI.md), [API_HTTP](https://github.com/OHF-Voice/piper1-gpl/blob/39ac0a04ff01120b1cb7867d21723fa320305b03/docs/API_HTTP.md)
- [piper-voices (Hugging Face)](https://huggingface.co/rhasspy/piper-voices/tree/e9c8a39444359388adaa94255becb37a5a2b8e51): `MODEL_CARD` of `pt_BR` `cadu`, `jeff`, `faber` and `en_US` `joe`
- [Kokoro-82M model card](https://huggingface.co/hexgrad/Kokoro-82M/blob/f3ff3571791e39611d31c381e3a41a3af07b4987/README.md) and [VOICES.md](https://huggingface.co/hexgrad/Kokoro-82M/blob/f3ff3571791e39611d31c381e3a41a3af07b4987/VOICES.md)
- [sherpa-onnx README](https://github.com/k2-fsa/sherpa-onnx/blob/cdbcacf732271ca373c9f323cff3c6083ab50dbe/README.md), [LICENSE](https://github.com/k2-fsa/sherpa-onnx/blob/cdbcacf732271ca373c9f323cff3c6083ab50dbe/LICENSE), and the `tts-models` release asset list
- [Moonshine README](https://github.com/moonshine-ai/moonshine/blob/d8d8fa29d05ca67441a7719e70099f9f48917a93/README.md) and [LICENSE](https://github.com/moonshine-ai/moonshine/blob/d8d8fa29d05ca67441a7719e70099f9f48917a93/LICENSE)
- [Parakeet-TDT 0.6B v3 card](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3/blob/541d1f99c6b0c3cd0b11a95167540bb8edefd82b/README.md)
- [Whisper large-v3-turbo card](https://huggingface.co/openai/whisper-large-v3-turbo/blob/41f01f3fe87f28c78e2fbf8b568835947dd65ed9/README.md)
- [Chatterbox README](https://github.com/resemble-ai/chatterbox/blob/ab55923c27757c90cfcc407baa57dc25a7d25217/README.md), [Qwen3-TTS README](https://github.com/QwenLM/Qwen3-TTS/blob/022e286b98fbec7e1e916cb940cdf532cd9f488e/README.md)
- [espeak-ng README](https://github.com/espeak-ng/espeak-ng/blob/ee9fa7cc4704d683020526f47a9655f14c8d5f4c/README.md), [Silero VAD LICENSE](https://github.com/snakers4/silero-vad/blob/63d4a2a7bfce8cd9bb39ed8fa0afcb5236d73e01/LICENSE)
- Hugging Face model metadata (`api/models/<id>`) for licence fields of XTTS-v2, F5-TTS, Chatterbox, Qwen3-TTS, Kyutai TTS, Supertonic, Kokoro, Parakeet and Whisper; crates.io API for the crate licences in section 1
- ADRs [0002](adr/0002-models-as-control-plane.md), [0005](adr/0005-gpl2-licence.md), [0006](adr/0006-system-persona-soul-and-voice.md), [0007](adr/0007-linux-style-user-permissions.md), [0008](adr/0008-own-uefi-bootloader.md), [0017](adr/0017-mpl2-dependencies.md), [0021](adr/0021-answer-voice-from-the-soul.md), [0028](adr/0028-soul-and-personality-file-format.md); [S1](s1-bonsai-8b-local-run.md)
