# OpenVoice language-pack recipe

This recipe defines how a new MeloTTS language enters XRTranslate's existing
OpenVoice provider. It separates reproducible model conversion from shared
asset installation, runtime selection, and UI presentation.

## Classification before implementation

Classify the upstream choice by what changes:

| Upstream difference | XRTranslate representation | Download effect |
| --- | --- | --- |
| Different language checkpoint, BERT, G2P/frontend, or graph files | one `ModelAssetManifest` language pack in `model_assets` | one independently installable package |
| Different quality/model export claiming a language already provided by another package | replacement package variant | selecting it replaces the same-language package |
| Speaker, accent, or base style already stored in one package | `voice_presets` entry | none |
| User reference voice | registered target speaker embedding | none; local runtime state only |
| CUDA, cuDNN, or ONNX Runtime version | host runtime plan | shared once across eligible local providers |

Do not create separate US and UK downloads when both speaker IDs and source
embeddings are present in one English model package. Do not put Chinese,
Japanese, or another distinct MeloTTS checkpoint into `voice_presets` merely
because all packs share the OpenVoice converter.

## Pinned upstream recipe

Every conversion recipe records all inputs needed to reproduce its bytes:

- MeloTTS source repository and full commit SHA;
- OpenVoice source repository and full commit SHA;
- language checkpoint repository, immutable revision, configuration SHA-256,
  checkpoint SHA-256, upstream speaker key, and numeric speaker ID;
- matching OpenVoice V2 source-embedding path, immutable revision, shape,
  dtype, and SHA-256;
- BERT repository, immutable revision, vocabulary and weight hashes, selected
  hidden state, output width, and dtype;
- frontend source data and versions, including tokenizer, normalization,
  segmentation, pronunciation, tone, and license inputs;
- exporter versions, ONNX opset, graph precision, input/output ABI, and
  validation results.

The first XRTranslate Chinese recipe is fixed to:

| Input | Immutable source |
| --- | --- |
| MeloTTS source | `myshell-ai/MeloTTS@209145371cff8fc3bd60d7be902ea69cbdb7965a` |
| OpenVoice source | `myshell-ai/OpenVoice@74a1d147b17a8c3092dd5430504bd83ef6c7eb23` |
| Chinese checkpoint | `myshell-ai/MeloTTS-Chinese@af5d207a364ea4208c6f589c89f57f88414bdd16` |
| OpenVoice V2 source embedding | `myshell-ai/OpenVoiceV2@f36e7edfe1684461a8343844af60babc2efbb727`, `base_speakers/ses/zh.pth` |
| Multilingual BERT | `bert-base-multilingual-uncased@7cbf9a625e29989f6b9c6c2fa68234c304f7e38f` |
| Reference encoder | `TigreGotico/voiceclonnx-openvoice-v2@34d010c192c97f763207f488f6057fd07fee42ad` |

The official Chinese configuration maps speaker key `ZH` to speaker ID `1`,
uses a 44,100 Hz MeloTTS base, and uses the `ZH_MIX_EN` frontend. The final
OpenVoice output remains 22,050 Hz. XRTranslate's verified v1 package is fixed
to public release revision
[`961ef7e65b63b7793dda61c7fe159a6e5a4b2f04`](https://huggingface.co/NowLoadY/XRTranslate-OpenVoice-ONNX/tree/961ef7e65b63b7793dda61c7fe159a6e5a4b2f04).

The official MyShell model collection also publishes distinct MeloTTS bases
for English, Spanish, French, Japanese, and Korean. Their presence upstream is
discovery information, not an XRTranslate capability claim. Each becomes
selectable only after this complete recipe is pinned, converted, licensed,
catalogued, and verified for that language.

## Frontend contract

An ONNX acoustic graph is not a complete language package. The runtime
frontend must reproduce the matching official MeloTTS input semantics without
Python:

```text
user text
  -> official-equivalent normalization
  -> tokenizer and language-specific segmentation
  -> pronunciation + contextual tone modification
  -> phones, tones, language IDs, and word2phone alignment
  -> contextual BERT feature expansion
  -> blank insertion and final graph tensors
```

For Chinese `ZH_MIX_EN`, the package contract includes:

- multilingual uncased BERT with 768-wide `ja_bert` features;
- a zero 1024-wide `bert` branch inside the exported wrapper;
- language ID `3` for the complete mixed-language sequence;
- English tone offset `7` for code-switched English phones;
- OpenCPOP pinyin-to-phone mapping;
- official-equivalent number conversion, punctuation normalization, phrase
  pronunciation, English WordPiece grouping, neutral tone, and tone sandhi;
- blank insertion and BERT repetition counts matching MeloTTS `word2ph`.

Generate golden fixtures with the pinned official Python frontend before
implementing or changing the Rust frontend. Fixtures cover normalized text,
phones, tones, language IDs, and `word2ph`; they are reviewed data, not a
runtime Python dependency. At minimum include integers and decimals,
polyphonic phrases, neutral tone, `一`/`不`, three-tone chains, punctuation,
newlines, English code switching, camel-case English, unknown characters, and
the 512-token boundary.

If a deliberate behavior differs from upstream, document the input, both
outputs, quality rationale, and a regression test. A non-silent synthesis
smoke does not establish frontend parity.

## Stable ONNX boundary

Language packages use the provider's shared base-graph ABI:

```text
x_tst:            int32[1,512]
x_tst_lenghts:    int32[1]
speakers:         int32[1]
tones:            int32[1,512]
lang_ids:         int32[1,512]
ja_bert:          float16[1,768,512]
length_scale:     float16[]
output:           float16[1,1,samples]
```

The historical `x_tst_lenghts` spelling is part of the graph ABI and must not
be silently corrected in only one consumer. The tensor width is a fixed graph
execution shape; `x_tst_lenghts` retains the real unpadded phone count. The
Chinese provider splits text at provider-private phoneme boundaries before a
segment exceeds the CUDA-validated 28-phone execution window, then joins the
base segments before tone conversion. This graph safety rule does not belong
in the shared model downloader or UI.

The packaged graph uses MeloTTS's supported deterministic duration-predictor
path (`sdp_ratio=0`) and the acoustic latent distribution mean. An unseeded
ONNX RNG is not part of XRTranslate's TTS contract and can produce a degenerate
duration before the host can recover. Language-specific BERT output
dtype or width belongs in the package/frontend recipe; sharing the provider
does not justify guessing a tensor contract.

Export tooling must preserve the project's required PyTorch build. Pin and
record all other byte-affecting dependencies rather than updating PyTorch to
make an exporter warning disappear. Validate the ONNX model, graph tensor
metadata, fixed execution width plus real length, multiple real frontend
fixtures, numerical finiteness, and output sample rate.

## Package and publication

The staged language package contains only deterministic runtime inputs:

```text
package-manifest.json
model_config.json
models/
frontend/
voices/
licenses/
```

`package-manifest.json` records provenance, conversion versions, graph ABI,
validation evidence, and every other packaged file's size and SHA-256. The
release manifest may hash package files and README content but must not hash
itself. Staging starts from an empty directory so results do not depend on an
earlier run.

Before catalogue registration:

1. preserve complete licenses and attribution for checkpoint weights, BERT,
   pronunciation data, generated lexicons, and third-party graphs;
2. publish to a public repository because the shared installer performs
   unauthenticated downloads;
3. obtain the resulting immutable commit SHA;
4. verify every file again through that commit's public resolve URL;
5. copy exact relative paths, sizes, and SHA-256 values into the asset
   catalogue.

Moving branches, private repositories, LFS pointer bytes, size-only checks,
and placeholder revisions are not valid installation contracts.

## Integration without duplicate infrastructure

1. Add the manifest and semantic file roles in
   `xrtranslate-assets/src/catalog/tts/openvoice.rs`; re-export it through the
   existing TTS and aggregate catalogues.
2. Add only the frontend/model behavior that differs inside
   `xrtranslate-inference/src/tts/providers/openvoice/`. Keep the provider
   entry module as the readable language-to-frontend composition map.
3. Map the asset's stable base-speaker preset in
   `backend/model_runtime/tts/providers/openvoice.rs`.
4. Let the provider-erased TTS adapter compose active packs and route by
   manifest language tags.
5. Let onboarding, settings, size display, deletion, and download state
   consume manifests and the shared model task manager. Do not add an
   OpenVoice download worker or language-specific UI branch.

## Acceptance gates

- catalogue roles, paths, public URLs, sizes, hashes, languages, voice
  presets, and package composition tests pass;
- provider configuration accepts complementary languages, rejects overlapping
  variants, and preserves/removes language-to-preset entries correctly;
- official frontend fixtures pass for the new language;
- every graph constructs and runs under the exact managed CUDA plan with CPU
  fallback disabled;
- English plus the new language can be prepared together, share a registered
  voice, and route synthesis independently;
- real-reference output is intelligible, uses the declared sample rate, and
  has documented voice-similarity evidence;
- a clean unauthenticated installation exercises queueing, progress, resume,
  verification, atomic activation, and deletion through shared infrastructure.
