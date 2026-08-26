# OpenVoice TTS provider

XRTranslate's `openvoice` provider is a pure-Rust, in-process ONNX TTS and
voice-cloning path. It does not start Python, an external model server, or a
provider-specific downloader. The provider combines a language-specific
MeloTTS base voice with the OpenVoice V2 tone-color converter.

The provider currently has three model assets:

| Asset | Synthesis language | Choice inside the package |
| --- | --- | --- |
| `openvoice-v3-onnx-fp16` | English | one `EN-Newest` base speaker |
| `openvoice-v2-onnx-fp16` | English | American, British, Indian, Australian, and default English base speakers |
| `openvoice-v2-zh-onnx-fp16` | Chinese with mixed-English input | one Chinese base speaker |

The two English assets both claim `en`, so they are replacement model
variants and cannot be active together. The Chinese asset claims `zh`, so it
is an independently installable language pack and may be active alongside one
English asset.

## Package, speaker, and cloned voice are different choices

These choices must not be represented by the same configuration or download
control:

- A **language pack** contains a distinct MeloTTS checkpoint, text frontend,
  contextual-embedding graph, source embedding, and immutable file manifest.
  It belongs in `model_assets` and creates one download/install task.
- A **packaged base speaker or accent** selects a speaker ID and its matching
  source embedding already present in one installed package. It belongs in
  `voice_presets` and never creates another download.
- A **registered cloned voice** is a target tone-color embedding derived from
  the user's reference audio. It is neither a model package nor an English
  accent. Registration is applied to every active language pack so the same
  named target voice can be used by language routing.

Consequently, US and UK English are not separate models in the OpenVoice v2
asset. They are two base-speaker/accent presets in the same download. The v3
`EN-Newest` graph is a separate package because NVIDIA distributes it as a
different immutable model variant. Chinese is also a separate package because
it uses a different official MeloTTS checkpoint, multilingual BERT, language
ID, pronunciation resources, and source embedding.

OpenVoice voice cloning transfers tone color. The selected MeloTTS base still
controls language, pronunciation, accent, and much of the prosody. XRTranslate
does not describe arbitrary emotion, rhythm, or style controls as supported
unless a future graph and typed provider setting implement them explicitly.

## Capability contract

| Capability | Contract |
| --- | --- |
| Provider ID | `openvoice` |
| Transport | in-process `onnx` |
| Model selection | one English variant and any complementary installed language packs |
| English routing | `en` plus locale forms such as `en-US` and `en-GB` |
| Chinese routing | `zh` plus locale forms such as `zh-CN` and `zh-TW` |
| Output | mono PCM16 at 22,050 Hz |
| Voice reference | mono PCM16 WAV; decoded and resampled to 22,050 Hz |
| Reference transcript | accepted by the shared clone API but not consumed by OpenVoice |
| User synthesis options | `speed`, validated in `0.5..=2.0`; language-to-preset entries in `voices` |
| Execution device | managed NVIDIA CUDA with at least 7 GiB VRAM; no managed-model CPU fallback |

The 22,050 Hz value is the final provider wire contract. MeloTTS Chinese and
English base graphs may emit 44,100 Hz internally, but the base waveform is
resampled before OpenVoice conversion. The backend sends raw PCM bytes without
an inline header, so every active OpenVoice manifest declares the final
22,050 Hz rate. The editable legacy `sample_rate` setting is not a conversion
mechanism and is not trusted for native model packages.

## Model and signal path

```text
text + routed target language
  -> frontend owned by the selected language pack
  -> BERT WordPiece tokens and contextual embeddings
  -> matching MeloTTS base speaker at 44,100 Hz
  -> resample to 22,050 Hz
  -> OpenVoice V2 tone-color converter
       source embedding: packaged embedding for the selected base speaker
       target embedding: registered reference voice
  -> mono PCM16 at 22,050 Hz

reference PCM16 WAV
  -> decode + resample to 22,050 Hz
  -> 1024-point Hann STFT, hop 256, reflect padding 384
  -> FP32 reference encoder
  -> 256-value target speaker embedding
```

The English frontend owns English normalization, WordPiece-to-phone
alignment, CMU pronunciation lookup, tone offsets, and OOV spelling behavior.
The Chinese frontend owns `ZH_MIX_EN` normalization, pinyin and phrase
resolution, tone sandhi, OpenCPOP phone mapping, multilingual-BERT alignment,
and Chinese/English code switching. A language may be catalogued only after
its frontend behavior is versioned and tested with fixtures from the matching
official MeloTTS source. See
[OpenVoice language-pack recipe](openvoice-language-packs.md).

The BERT, MeloTTS, converter, and reference-encoder sessions form one CUDA
execution group. Every active language pack is prepared before the provider
is reported ready. If any graph cannot construct on CUDA, provider startup
fails; it is not rebuilt on CPU.

## Asset provenance

The asset manifest is the only installation contract. It fixes every installed
file by immutable revision, byte size, and SHA-256 and activates the package
only after complete preflight.

- The English archives are NVIDIA's signed
  [NVIGI OpenVoice v2/v3 packages](https://catalog.ngc.nvidia.com/orgs/nvidia/nvigisdk/models/openvoice).
  The v2 `speaker_ids.json` maps `EN-US=0`, `EN-BR=1` (British),
  `EN_INDIA=2`, `EN-AU=3`, and `EN-Default=4`.
- The Chinese base comes from the official
  [`myshell-ai/MeloTTS-Chinese`](https://huggingface.co/myshell-ai/MeloTTS-Chinese)
  checkpoint and the Chinese source embedding from
  [`myshell-ai/OpenVoiceV2`](https://huggingface.co/myshell-ai/OpenVoiceV2).
  XRTranslate's ONNX conversion records the exact upstream commits,
  checkpoint revisions, graph ABI, toolchain versions, and output hashes in
  its package manifest. The verified package is published at immutable
  revision
  [`NowLoadY/XRTranslate-OpenVoice-ONNX@961ef7e`](https://huggingface.co/NowLoadY/XRTranslate-OpenVoice-ONNX/tree/961ef7e65b63b7793dda61c7fe159a6e5a4b2f04).
- The shared FP32 reference encoder is the immutable
  [`tone_ref_encoder.onnx` export at revision `34d010c`](https://huggingface.co/TigreGotico/voiceclonnx-openvoice-v2/commit/34d010c192c97f763207f488f6057fd07fee42ad).
- The source projects are
  [MyShell MeloTTS](https://github.com/myshell-ai/MeloTTS) and
  [MyShell OpenVoice](https://github.com/myshell-ai/OpenVoice). OpenVoice V1
  and V2 and MeloTTS are published under MIT terms. Each redistributed model,
  frontend resource, and generated dataset retains its own required notice.

The application never downloads a moving branch. A community conversion is
eligible for the catalogue only after it is available without authentication
at a public immutable commit. The catalogue's required-file sizes and hashes,
not a repository README, drive onboarding size display and installation.

Current English package totals are:

| Variant | Network transfer | Installed required files |
| --- | ---: | ---: |
| v3 `EN-Newest` | 207,772,473 bytes | 255,830,497 bytes |
| v2 five accents | 207,838,325 bytes | 255,966,606 bytes |

The Chinese package transfers and installs 468,011,765 bytes (about 446.3
MiB). This total is read directly from its direct-file manifest so it remains
correct when a license or frontend resource changes. No model or Python
environment is bundled into the default XRTranslate release.

## Integration boundaries

Concrete OpenVoice knowledge is limited to these owners:

- `crates/xrtranslate-assets/src/catalog/tts/openvoice.rs`: immutable files,
  source revisions, hashes, licenses, provider identity, synthesis languages,
  package labels, and base-speaker presets;
- `crates/xrtranslate-inference/src/tts/providers/openvoice/`: language
  frontends, tensor contracts, source embeddings, reference encoding,
  synthesis, and provider validation;
- `apps/xrtranslate-backend/src/model_runtime/tts/`: provider registration,
  configuration-to-adapter mapping, asset ownership validation, multi-pack
  composition, and provider-erased language routing;
- `config.json`: user-selectable provider settings and declarative runtime
  archives. `voices` stores language-to-preset stable keys; speaker numbers and
  embedding filenames remain implementation details.

`main.rs`, the pipeline, TTS worker, onboarding, settings, download manager,
and playback consume neutral adapters, asset manifests, task snapshots, or
typed capabilities. They must not branch on `openvoice`, a model revision, a
language-specific filename, or a provider tensor name.

OpenVoice does not consume ASR instructions, lexical context, or weighted
vocabulary. Prompt Studio and ASR bias semantics must not be routed into this
TTS adapter merely because all of them contain text.

## Verification boundary

Fast tests protect catalogue composition, provider/asset ownership,
language-to-preset validation, language routing, frontend alignment, graph
shapes, sample-rate metadata, and the strict managed-CUDA policy.

A release candidate additionally requires:

1. an unauthenticated download from the exact catalogue revision followed by
   size/SHA preflight and atomic activation;
2. CUDA-only construction of every graph for every active language pack;
3. official-frontend fixtures for normalized text, phones, tones, language
   IDs, and `word2phone` alignment;
4. a clean real-speaker reference, intelligible text in every declared
   language, ASR read-back, and audible voice-similarity comparison;
5. confirmation that output is mono PCM16 at 22,050 Hz and that English and
   Chinese packs route independently when activated together.

A non-silent waveform or a session that silently used `CPUExecutionProvider`
is not release evidence.
