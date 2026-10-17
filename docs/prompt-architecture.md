# Prompt architecture

Prompt composition is a shared domain. Model delivery, runtime context, and UI
editing use its contracts without maintaining separate prompt implementations.

Translation models declare a shared prompt profile in their local model card or
remote capabilities (`translation_prompt_profile`). Prompt Studio and inference
resolve that same declaration. Connector identity only selects the wire format;
service and model names do not select graph pages.

The graph exposes chat messages and translation-instruction delivery paths, plus
the existing recognition-mode branches. Both translation paths share the style
and context nodes. Fixed bilingual models retain their training template and do
not advertise style support. Message wording, context blocks, roles, and ordering remain owned
by the graph, with transport-only conversions owned by the connector.

## Ownership

| Owner | Responsibility |
|---|---|
| Task owner | Context selection, conversation scope, and task lifetime |
| Backend context | Bounded conversation memory, ordering, and request snapshots |
| XR Corpus | Terminology, lexical retrieval, and recognition vocabulary |
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

Each translation task supplies a `TaskContext`: a complete snapshot of reference
facts, or a conversation scope with initial facts. Owners select relevant
completed translations and surrounding source text. A snapshot has no implicit
history; conversation memory is held by the backend for the owner's scope.
Starting a new document, media job, or independent capture gives it fresh context.

Microphone, system audio, and text may explicitly share a conversation. Input
sources label current and historical speakers. Existing turn identifiers and
observation order distinguish asynchronous results and streaming revisions;
only accepted translations enter completed history. Previous revisions of the
current input remain separate from completed turns. Additional target languages
retain their own translated history.

Context facts enter the graph through shared runtime values, independently of
style and model delivery. The backend applies character and model token budgets,
selects supported reference capabilities, and combines task facts with retrieved
terminology. Plugins do not maintain model-specific prompt variants. Optional
context removal during recovery affects the request, not the owner's memory.

## Execution and validation

Graphs are validated before activation. Typed connections, required inputs,
delivery targets, and message ordering have one definition shared by editor and
backend. Execution produces the messages sent to the selected provider.

Translation execution publishes its trace after graph evaluation and before the
model request. Prompt Studio follows the latest request automatically, including
recovery and concurrent tasks, and displays its matching graph values and assembled
request read-only. Plugin-owned facts are not editable in Studio. Recognition
traces accompany their corresponding results; neither view runs a second evaluator
or supplies translation memory.

Inference validates normalized output before the pipeline accepts it as a final
result. The backend coordinates bounded recovery using the same graph and
provider contract. Rejected or incomplete output must not become a successful
translation or an irreversible downstream action.

## Persistence and evolution

The shared prompt domain owns schema evolution and built-in defaults. User
profiles and styles retain authored content through supported normalization.
Editor layout data does not define prompt behavior, and delivery views do not
create independent copies of the same graph.

Delivery paths use capability names. Import accepts legacy provider-based names
and preserves node IDs, authored instructions, styles, and connections. Graph
creation, import/export, and deletion use the same library.

Concrete prompt wording, node definitions, schema versions, and recovery
algorithms are maintained with their implementation. See
[provider integration](providers/README.md) for delivery contracts and
[UI architecture](ui-architecture.md) for shared presentation ownership.
