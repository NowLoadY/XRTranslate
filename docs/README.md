# Documentation map

Documents are grouped by responsibility. Stable governance and cross-cutting
architecture documents remain at this level; implementation-specific provider
notes live under `providers/`.

## Contracts and architecture

- [Refactoring contract](refactoring-contract.md): compatibility invariants,
  dependency direction, and verification requirements.
- [Platform architecture](platform-architecture.md): host/platform boundaries,
  downloads, runtime selection, and resource ownership.
- [Plugin architecture](plugin-architecture.md): extension contracts and
  dependency rules.
- [Prompt architecture](prompt-architecture.md): Prompt Studio semantics,
  execution traces, and provider delivery boundaries.
- [UI architecture](ui-architecture.md): shared theme tokens, animation timing,
  and opt-in GPU border rendering.

## Providers and runtime

- [Provider integration](providers/README.md): where shared infrastructure ends
  and a concrete model/provider adapter begins.
- [OpenVoice TTS](providers/openvoice-tts.md): English and Chinese ONNX language
  packs, packaged accent choices, voice-cloning contract, and verification
  boundary.
- [OpenVoice language-pack recipe](providers/openvoice-language-packs.md):
  reproducible frontend/export requirements and the integration path for
  additional official MeloTTS languages.
- [Online API providers](online-api-providers.md): common remote-provider
  configuration and user override behavior.
- [Runtime resource matrix](runtime-resource-matrix.md): model/runtime packages
  and GPU selection.
- [Local ONNX TTS runtime design](tts-cuda-runtime-design.md): shared ONNX
  Runtime, CUDA, cuDNN, eligibility, and provider-composition policy.

## Features and operations

- [Speaker diarization](speaker-diarization.md)
- [Linux build](linux-build.md)
- [Contributors](contributors.md)
