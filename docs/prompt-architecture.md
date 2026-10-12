# Prompt architecture

Prompt composition is a shared domain. Model delivery, runtime context, and UI
editing use its contracts without maintaining separate prompt implementations.

## Ownership

| Owner | Responsibility |
|---|---|
| XR Corpus and backend | Structured context, terminology, and recognition facts |
| `xrtranslate-prompt` | Graph schema, validation, built-in graphs, and deterministic rendering into ordered messages |
| Prompt Studio | Editing, selection, persistence, and display of execution traces |
| Inference profiles and adapters | Delivery target, sampling, transport conversion, and output normalization |
| Backend pipeline | Capability selection, scheduling, result acceptance, and bounded recovery |

Semantic instructions, composition, and message roles belong to the prompt
domain. Adapters preserve rendered content and role order; they do not append
hidden instructions or reconstruct context wrappers. Models requiring a fixed
prompt select it through the same domain boundary.

Style selection and graph editing use one prompt library and activation path.
Canvas layout and editor interaction remain separate from execution semantics.
Shared editor behavior belongs to the common UI infrastructure.

## Context and capabilities

Runtime context is structured data supplied to the graph. Instruction policy,
current input, and optional reference facts remain distinct so that reference
handling is explicit and optional context can be removed without changing the
task's meaning.

ASR instruction, lexical context bias, and weighted vocabulary are independent
capabilities. The prompt graph renders supported text inputs. Structured
vocabulary passes through its own typed contract. Providers declare support;
neither adapters nor consumers reinterpret one capability as another.

Translation context follows task and language ownership. Stateless ask requests
use the configured translation adapter without joining conversation history;
the calling feature supplies the request's purpose and necessary input facts.

## Execution and validation

Graphs are validated before activation. Typed connections, required inputs,
delivery targets, and message ordering have one definition shared by editor and
backend. Execution produces the messages sent to the selected provider.

Execution traces come from that same evaluation and accompany the corresponding
result. UI displays the matching graph and request trace rather than running a
second evaluator. Recovery traces describe the request that produced the result.

Inference validates normalized output before the pipeline accepts it as a final
result. The backend coordinates bounded recovery using the same graph and
provider contract. Rejected or incomplete output must not become a successful
translation or an irreversible downstream action.

## Persistence and evolution

The shared prompt domain owns schema evolution and built-in defaults. User
profiles and styles retain authored content through supported normalization.
Editor layout data does not define prompt behavior, and delivery views do not
create independent copies of the same graph.

Concrete prompt wording, node definitions, schema versions, and recovery
algorithms are maintained with their implementation. See
[provider integration](providers/README.md) for delivery contracts and
[UI architecture](ui-architecture.md) for shared presentation ownership.
