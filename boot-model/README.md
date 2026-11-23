# boot-model

Host-side tooling for the boot model of ADR 0027 (roadmap item B2): the corpus, the
training run, the `\WSOS.MDL` weights file and the evaluation harness. Nothing here is
built for the bare-metal or UEFI targets; the kernel and the bootloader do not depend on it.
The report, with the numbers and the commands, is
[`docs/b2-boot-model-corpus-and-training.md`](../docs/b2-boot-model-corpus-and-training.md).

| File | What it is | Needs |
|---|---|---|
| `window.py` | The 512-byte window of a candidate: the contract B3 implements | stdlib |
| `labels.py` | The closed labels and the order of the 34 scores | stdlib |
| `mdl.py` | `\WSOS.MDL` writer and reader, the integer forward pass, the budget check | stdlib |
| `metrics.py`, `evaluate.py` | The arithmetic of the evaluation harness | stdlib |
| `fetch.py` | Pins and downloads the corpus into a cache outside the repository | network |
| `build_ours.py` | Builds our kernel and bootloader at several commits and pins their first bytes | cargo |
| `catalogue.py`, `synthetic.py`, `layouts.py` | What each file is, generated files, the disk layouts | stdlib |
| `training.py`, `dataset.py`, `run_b2.py` | Training, quantisation and the full run | numpy, scikit-learn |
| `evaluate_model.py`, `render_report.py` | Evaluate a stored `\WSOS.MDL` on the generated sets; print the tables of the report from `results.json` | numpy / stdlib |
| `test_core.py`, `test_corpus.py` | Unit tests; run by `./scripts/check.sh` | stdlib |
| `test_training.py` | Tests of the numpy integer path against `mdl.forward`; skipped without numpy, so the gate skips it: `uv run --no-project --with numpy==2.5.5 python boot-model/test_training.py` | numpy |
| `mutation_check.py` | Confirms by mutation that the tests notice a change | stdlib |
| `corpus/pins.json`, `corpus/ours-pins.json` | The pinned corpus: URL, size, SHA-256 and licence of each file | |

The gate (`./scripts/check.sh`) runs the unit tests only. It does not download the corpus,
train, or need numpy. `python3 -I boot-model/mutation_check.py` runs each test against a set
of deliberate breakages and fails if one goes unnoticed.
