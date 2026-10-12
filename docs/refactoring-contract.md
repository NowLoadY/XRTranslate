# Refactoring contract

Architecture cleanup should clarify ownership and reuse while preserving
observable behavior. Prefer the smallest design that expresses the actual
domain boundary.

## Preserve the baseline

Preserve behavior, presentation, defaults, stored data, stable identities,
public contracts, and concurrency semantics unless the requested change alters
them. Treat the current working tree as the baseline and retain pre-existing
user changes.

## Ownership and dependency direction

- Shared domain and runtime capabilities remain independent of concrete plugins.
- The host composes capabilities, arbitrates shared resources, and routes typed
  actions and events. Domain policy stays with its owning module.
- Plugins own their feature state and lifecycle. They consume focused host
  contracts rather than the root application object or another plugin's internals.
- UI consumes snapshots and emits actions. It does not acquire unrelated
  persistence, transport, or resource ownership.
- Shared event delivery remains generic; subscriber adapters perform domain
  interpretation and presentation.

Each rule has one owner. Provider/model metadata belongs in catalogues, prompt
semantics in the prompt domain, transport in connectors, and installation in
resource owners using shared download infrastructure. Consumers query neutral
capabilities instead of repeating provider, model, or platform branches.

See [plugin](plugin-architecture.md), [platform](platform-architecture.md),
[prompt](prompt-architecture.md), and [UI](ui-architecture.md) contracts for the
corresponding boundaries.

## Extraction and module shape

Extract when real consumers share a domain rule, a responsibility has the wrong
owner, or a stable separation makes dependencies and lifecycle clearer. Do not
split solely to meet line counts or introduce abstractions for hypothetical
consumers. Similar-looking code may remain separate when its meaning differs.

Organize modules by domain. Keep entry points small and implementation details
private; widen visibility only for an intentional contract. Avoid generic
utility collections, speculative traits, or indirection that hides control flow.
Remove superseded paths and redundant code when the new owner is established.

## Verification and completion

Read the affected callers and existing evidence before choosing verification.
After a coherent change, check for stale references, duplicate ownership, and
reversed dependencies. Add tests only for an important, concrete need; do not
introduce test frameworks or loops as routine refactoring work.

Build or compile only when necessary to resolve a specific uncertainty, and
consolidate that verification after related changes. Completion means callers
use the intended owner, compatibility is preserved, and any required checks
support the result. Report material unresolved issues briefly.

Architecture documents follow the [documentation scope](README.md): maintain
current ownership and contracts, rather than appending implementation history.
