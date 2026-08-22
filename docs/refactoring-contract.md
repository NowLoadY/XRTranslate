# Refactoring contract

Use this contract for architecture cleanup. A refactor is successful only when
it makes ownership and reuse clearer without changing observable behavior.

## Objective

Reduce duplicated implementations and oversized mixed-responsibility modules.
Keep shared recognition, translation, audio, configuration, lifecycle, and UI
capabilities reusable. Keep plugins independently understandable and removable.

Prefer the smallest design that expresses the real domain boundary. Fewer lines
are useful only when the result is easier to read, test, and extend.

## Non-negotiable invariants

- Preserve runtime behavior, visible text, layout, spacing, colors, interaction,
  timing, error handling, defaults, and supported inputs unless a separate
  change explicitly requests otherwise.
- Preserve serialized configuration, database schemas, stable plugin and page
  IDs, file locations, public interfaces, and migration behavior.
- Preserve cancellation, shutdown, worker ownership, channel capacity, event
  ordering, and other concurrency semantics.
- Treat the current working tree as the behavioral baseline. Do not discard,
  overwrite, or silently redesign pre-existing uncommitted work.
- Keep every intermediate batch compiling and independently reviewable.

## Ownership and dependency direction

The intended dependency direction is:

```text
shared domain/runtime/UI capability
                  ^
                  |
           host coordination
                  ^
                  |
        plugin controller + UI
```

- Shared infrastructure must not import a concrete plugin.
- Shared session request/event types must not contain variants, fields, or
  lifecycle branches named after a concrete plugin. Plugins configure sessions
  through `TranslationSessionPlugin` and consume results through the neutral
  subscriber contracts.
- A plugin owns its domain state, persistence, workers, UI, and assets below its
  named module. It requests host capabilities through small typed inputs and
  actions rather than receiving the root application object.
- Cross-plugin reuse belongs in a clearly named shared capability only when at
  least two real consumers need the same semantics. A plugin must not reach into
  another plugin's private module.
- Host code coordinates exclusive resources and routes typed events; it must not
  duplicate a plugin's domain policy.
- Generic event pumps broadcast to registered subscribers. Conversion from a
  generic session event into plugin-domain data belongs in that plugin's event
  adapter, not in the pump or networking layer.
- All immutable remote-file transfers use `xrtranslate-download`. Feature
  modules own artifact selection, extraction, installation, and UI state, but
  must not implement their own HTTP chunk loop, range-resume, retry, proxy, or
  checksum policy. SHA-256 verification is the default; size-only verification
  is an explicit fallback only when a trusted source publishes no digest.
  Source switching must use cooperative cancellation. Staging cleanup remains
  with the artifact owner and happens only after the shared transfer releases
  its file handle; UI code must not delete `.part` files directly.
- Provider/model selection is data-driven through the shared configuration and
  asset manifest catalogue. Main application and onboarding code may branch on
  neutral capabilities, but must not name a concrete provider, model, revision,
  or resource path. Delete controls pass an asset identity to its owner and
  never construct or recursively remove resource paths in UI code. Every
  user-initiated resource deletion requires a shared confirmation dialog that
  names the target; cancellation clears the pending identity without invoking
  the owner.
- UI rendering should consume a snapshot/controller and emit typed actions.
  Rendering code must not acquire unrelated runtime or persistence ownership.
- Treat model package metadata, prompt composition, and provider delivery as
  different domains. Asset variants belong in the manifest catalogue. All
  semantic prompt text, variables, conditions, message roles, and provider
  output paths belong in `xrtranslate-prompt`; provider profiles select a
  graph output and own only sampling and output-cleanup rules. Process/adaptor
  creation belongs in the backend runtime plan. Pipelines and plugins consume
  these contracts without matching provider or model names.
- Query selectable models by provider and capability, including level only
  when the operation requires it. A new size for an existing model family must
  not require another fixed field or branch in every consumer.
- Model selection cardinality, synthesis languages, and hardware requirements
  are manifest/domain metadata. ASR and translation variants are singular;
  complementary TTS language packs may be plural. UI, installer, backend, and
  task routing must consume the same metadata instead of maintaining parallel
  provider-specific language or hardware lists. Preserve `model_asset` as a
  serialized compatibility alias while accepting `model_assets` where plural
  selection is meaningful.
- Downloadable managed models must not silently change execution class. They
  require an eligible NVIDIA CUDA host with at least 8 GiB VRAM; only explicitly
  classified small bundled ONNX components may execute on CPU.

## When to extract or split

Extract code only when at least one condition is demonstrated:

1. two or more callers implement the same domain rule;
2. a capability is incorrectly owned by one consumer but is already needed by
   another;
3. a file contains independently testable responsibilities with a stable seam;
4. dependency direction or lifecycle ownership becomes clearer after the move;
5. a new plugin would otherwise need to copy an existing implementation.

Do not extract merely to satisfy a line-count target. Do not replace direct code
with speculative traits, deep generic layers, broad utility modules, or macros
that hide control flow. Small duplication is preferable when the semantics are
different or likely to evolve independently.

## Module organization and naming

- Organize by domain and responsibility, not by incidental type names.
- Use a directory when a domain has multiple substantial responsibilities; keep
  its entry module as the readable public surface and dependency map.
- Avoid a flat collection of ambiguous names such as `utils`, `helpers`,
  `common`, `manager`, or `misc`. Name the capability or policy precisely.
- Keep implementation details private. Use `pub(crate)` only for intentional
  crate-level seams and `pub` only for actual package contracts.
- Keep tests next to the rule they protect. A moved rule takes its tests with it.

## Refactoring workflow

1. Inspect `git status`, the complete diff, callers, tests, and module ownership.
2. State the concrete smell, consumers, proposed owner, invariants, and risk.
3. Establish a green baseline with formatting, compile, and relevant tests.
4. Make one coherent move or extraction at a time. Avoid mixing feature work,
   visual redesign, renaming, and architecture cleanup in one batch.
5. Search for stale paths, compatibility aliases, duplicate implementations,
   widened visibility, and reversed dependencies.
6. Run formatting, workspace compilation, and focused tests after each batch;
   run the full workspace test suite before handoff when practical.
7. Report files moved, APIs introduced or removed, preserved compatibility, and
   any risk that could not be verified automatically.

## Completion criteria

A batch is complete when:

- all call sites use the intended owner and no accidental duplicate remains;
- module names and placement make the dependency direction apparent;
- the public surface is no broader than before unless justified;
- formatting, compilation, and relevant tests pass without new warnings;
- observable behavior and visuals remain unchanged;
- the diff contains no unrelated cleanup and preserves prior user changes.
