# Documentation map

Documents are grouped by responsibility. Architecture documents define module
ownership, dependency direction, shared contracts, and resource lifecycle.
Keep them short and current: update the relevant rule when a boundary changes.
Algorithms, default parameters, schema fields, UI walkthroughs, migration
history, and verification logs do not belong in these documents. Code and
configuration own implementation details; operational guides retain procedures
needed to build, integrate, or use the system. Avoid repeating the same rule
across documents; link to its owner.

## Contracts and architecture

- [Refactoring contract](refactoring-contract.md): compatibility invariants,
  ownership, extraction, and proportionate verification.
- [Platform architecture](platform-architecture.md): host/platform boundaries,
  capability composition, and resource ownership.
- [Plugin architecture](plugin-architecture.md): extension contracts and
  plugin lifecycle.
- [Prompt architecture](prompt-architecture.md): prompt, context, execution,
  and delivery ownership.
- [UI architecture](ui-architecture.md): shared presentation, adaptive layout,
  and interaction ownership.

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
- [Recorded speech check](speech-check.md)
- [Text/speech roundtrip check](roundtrip-check.md)
- [Contributors](contributors.md)
