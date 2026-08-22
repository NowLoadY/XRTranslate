# OpenVoice multilingual ONNX export

This directory contains XRTranslate's reproducible conversion workflow for the
official MyShell MeloTTS base voices used by OpenVoice V2. It intentionally
runs inside the existing `torch211cu128` Conda environment and never installs,
upgrades, or downgrades PyTorch.

The workflow exports only provider-specific model artifacts. Application model
selection, immutable downloads, and installation remain owned by
`xrtranslate-assets`; inference remains owned by `xrtranslate-inference`.

## Pinned upstream sources

- MeloTTS: `myshell-ai/MeloTTS` at
  `209145371cff8fc3bd60d7be902ea69cbdb7965a`
- OpenVoice: `myshell-ai/OpenVoice` at
  `74a1d147b17a8c3092dd5430504bd83ef6c7eb23`
- OpenVoice V2 embeddings: `myshell-ai/OpenVoiceV2` at
  `f36e7edfe1684461a8343844af60babc2efbb727`

`languages.py` records immutable model, BERT, vocabulary and source-embedding
digests for the official Chinese, Spanish, French, Japanese and Korean V2 base
voices. This is an upstream candidate matrix, not an availability list:

| Key | Frontend | Workflow state |
| --- | --- | --- |
| `zh` | Chinese mixed-English | Buildable and parity-tested |
| `es` | gruut Spanish | Blocked: BERT license absent; frontend closure incomplete |
| `fr` | gruut French | Blocked: frontend data/license closure incomplete |
| `jp` | MeCab/UniDic Japanese | Blocked: frontend data/license closure incomplete |
| `kr` | g2pK/MeCab-ko Korean | Blocked: BERT license absent; frontend closure incomplete |

`frontend_recipes.py` owns those readiness gates and lists the runtime data and
license/NOTICE closure each language actually needs. `build.py --language`
offers only recipes that have both redistribution approval and an implemented,
parity-tested frontend. A discovered upstream checkpoint can therefore never
silently become a shippable package.

The checked-out upstream source trees live beside this directory as
`third_party/MeloTTS` and `third_party/OpenVoice`. Their original licenses are
preserved in those trees.

## Build

From the repository root:

```powershell
conda run -n torch211cu128 python -m pip install --no-deps `
  -r third_party/openvoice_onnx_export/requirements-export.txt
conda run -n torch211cu128 python third_party/openvoice_onnx_export/build.py `
  --language zh `
  --output runtime/.temp/openvoice-onnx-export
```

The script refuses to run unless the environment still has the recorded
PyTorch build (`2.11.0+cu128`). It downloads immutable upstream revisions via
the Hugging Face cache, exports the FP16 MeloTTS graph, copies the matching
OpenVoice source embedding, and writes a machine-readable build manifest with
every source and output SHA-256.

The export wrapper preserves control operators such as `Range` in FP32 while
keeping the acoustic graph and its public tensor ABI in FP16. This avoids an
invalid FP16 `Range` emitted by PyTorch 2.11's legacy ONNX exporter without
changing model weights or the PyTorch installation.

The packaged graph uses MeloTTS's supported deterministic duration predictor
(`sdp_ratio=0`) and the acoustic latent distribution mean. The production ABI
uses a fixed 512-phone execution tensor plus the real unpadded length; the Rust
provider performs its CUDA-validated segmentation before inference.

Each exported MeloTTS and BERT graph is checked before its smoke test. The
check rejects unexpected input/output names, tensor dtypes, fixed or dynamic
dimensions, ONNX opsets, and BERT feature widths other than 768. The contract
tests do not require a GPU or load PyTorch:

```powershell
conda run -n torch211cu128 python -m unittest discover `
  -s third_party/openvoice_onnx_export/tests -p "test_*.py" -v
```

The workflow never reads Hugging Face credentials. Authentication is required
only by the separate publishing step, and credentials must be supplied through
Hugging Face's interactive login/token store rather than committed files or
command-line arguments.

## Publish

`publish.py` stages a deterministic release, takes a repository-wide local
lock, and uploads one file at a time with an exact `parent_commit`. It records
atomic recovery state only after the Hub confirms the uploaded size and
SHA-256. The first publication requires a private repository.

After that release passes anonymous fixed-revision verification and the
repository becomes public, a later language addition must pass
`--allow-public-update` explicitly. Existing application catalogues continue
resolving their old immutable revisions while the new moving HEAD is assembled;
the new revision is not copied into the catalogue until the complete release
manifest passes the same verification gates.
