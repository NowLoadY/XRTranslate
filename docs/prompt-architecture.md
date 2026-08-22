# Prompt architecture

Prompt Studio owns the complete translation prompt and semantic ASR instruction
sent to a model. It also owns rendering provider text fields that are explicitly
declared as lexical ASR context. There is no hidden instruction,
reference-context wrapper, current-input label, or message-role split in a
provider profile.

ASR instruction prompts, lexical context bias, and weighted vocabulary bias are
three separate capabilities. An instruction tells a capable recognition model
how to transcribe. Lexical context supplies likely terms but has no instruction
semantics. Weighted vocabulary is structured provider data (`term -> weight`)
and never passes through a Prompt Studio text graph. A provider profile declares
which delivery modes its transport supports; it must not reinterpret one mode
as another.

## Ownership

`xrtranslate-prompt` is the shared prompt domain used by the desktop editor,
wire protocol, backend, and inference adapters. Its responsibilities are:

- the serialized graph schema and validation rules;
- neutral translation context facts;
- deterministic graph execution into ordered, role-bearing messages;
- canonical built-in graphs and saved-profile normalization;
- Compose text parsing and graph activation validation.

Provider profiles in `xrtranslate-inference` select a graph provider target and
own sampling parameters and output cleanup. The OpenAI adapter converts the
already-rendered messages to JSON without changing their content.

## Output validation and regeneration

Translation output is validated in `xrtranslate-inference` after provider
cleanup and before it can become a successful `TranslationResult`. The quality
gate consumes the exact ordered `PromptMessage` values rendered for that
request; it does not inspect a built-in template, provider name, graph node ID,
or hard-coded instruction. Custom Prompt Studio graphs therefore receive the
same protection as the canonical graph.

The prompt-echo detector normalizes Unicode letters and numbers, removes the
current input and rendered runtime reference facts from the comparison corpus,
then combines three independent forms of evidence: a substantial exact prompt
fragment, high character-shingle coverage, or multiple copied prompt lines.
Removing runtime values prevents an unchanged proper name or a legitimate
translation reused from dialogue history from being classified as an
instruction leak. Reference-context structural checks remain responsible for
detecting dumped terminology and history blocks.

Rejected text never enters session state, translation history, terminology
rewriting, or the wire protocol. The backend owns retry coordination: it clears
optional reference facts and regenerates the same source segment exactly once
through the same graph and provider target. The regenerated request retains all
main translation instructions, message roles, current input, and sampling
policy. If that output is also rejected, the segment fails and no translation
is published. As with a context-window retry, a successful regenerated result
carries the execution trace of the final request.

XR Corpus and the backend provide structured facts. XR Corpus retains its legacy
pre-rendered ASR field for older consumers, but the native backend consumes its
structured recognition vocabulary. The desktop client owns template selection
and persistence but does not execute provider policy.

## Graph pages and requests

A profile is one complete graph. Prompt Studio presents that graph through four
fixed delivery pages, `OPENAI`, `HUNYUAN`, `ASR PROMPT`, and `ASR CONTEXT`;
these pages are views, not separate profiles or independently creatable graphs.
Shared nodes appear on compatible pages, while delivery-specific composition
and Request nodes appear only on their matching page. `NEW GRAPH` creates the
complete graph with all four pages and the shared data flow.

Prompt Studio initially opens the page matching the active translation
provider. A local Hunyuan provider selects `HUNYUAN`; providers delivered over
the OpenAI transport select `OPENAI`. Applying a provider configuration updates
that initial page selection, while users may still switch pages manually to
inspect or edit the other provider path.

A graph renders `PromptMessage` values. Each translation provider page
terminates in exactly one Request node. ASR pages have at most one Request each
so old translation-only graphs remain wire-compatible; saved profiles are
normalized by adding any missing canonical ASR paths. A Request declares:

- a delivery target (`hunyuan`, `openai_compatible`, `asr_instruction`, or
  `asr_context_bias`);
- an ordered list of message roles (`system` or `user`);
- one connected content input for each role.

The built-in OpenAI Request has ordered `SYSTEM` and `USER` inputs. It is one
HTTP/API request containing two role-bearing messages, not one flattened string.
The built-in Hunyuan Request has one `USER` input. Provider adapters preserve
this role order exactly and only convert the rendered messages to transport JSON.

Compose nodes own verbatim static text, spaces, newlines, headings, and
boundaries. They interpolate connected inputs through `{0}` to `{9}`; literal
braces use `{{` and `}}`. A Compose text made only of input slots separated by
one consistent whitespace separator joins its non-empty inputs with that
separator. This makes `{0}\n\n{1}\n\n{2}` the general form of an optional block
join without a separate Concat node. Variable nodes expose source language,
target language, current input, and recognition context. Context input nodes render the terminology,
history, previous revision, and surrounding-source sections. Condition nodes
select the explicit/automatic language branch and the with/without-reference
branch. Provider adapters must not add separators around these values.

The built-in graph uses no fragmented Text nodes. Static prompt structure is
kept inside semantic Compose nodes so graph wires represent data flow rather
than punctuation. Links carry only endpoints and socket indexes; separators and
line breaks always remain visible in Compose text.

The fixed `REFERENCE HANDLING RULES` Compose node tells the model how terminology,
history, revisions, and surrounding speech must be interpreted. It is separate
from `TRANSLATION CONTEXT`, which contains the rendered runtime data. Both feed
the provider-specific with-context composition nodes, making policy and data
visible as distinct graph inputs without changing the assembled prompt text.
The rules require the model to use only relevant context to resolve ambiguity,
references, tone, and discourse continuity, producing coherent, natural,
idiomatic target-language expression. Predicates, arguments, referents, or
other meaning omitted from `Current input` must be recovered when the relevant
dialogue entails exactly one interpretation. Making that implicit meaning
explicit in the target language is semantic recovery, not expansion. When
multiple interpretations remain possible, the model must preserve the
ambiguity or fragment instead of guessing. Irrelevant context is ignored;
surrounding text is never translated, repeated, or summarized, and no event,
detail, intent, or meaning may be invented beyond what the current input and
relevant context entail.

## Runtime traces

`xrtranslate-prompt` owns `PromptExecutionTrace` and captures it during the
same cached graph execution that renders the provider messages. There is no
second preview evaluator for live data. Each visited node records its exact
output, Switch nodes also record the selected input, and the Request node
records the final ordered role-bearing messages. Nodes outside the executed
condition path have no trace entry.

Inference attaches the trace to the matching provider result. The backend and
wire protocol transport ASR traces with `SourceSegmentReady` and translation
traces with `TranslationReady`; the desktop stores the latest host-session
trace for each delivery family so a later translation does not erase the ASR
page's runtime data. If ASR retries without lexical context, or translation retries without
optional reference facts, the returned trace belongs to that final request
rather than the failed first attempt. Prompt Studio displays live data only for
the active graph and matching provider page. A deterministic graph fingerprint
also rejects late traces from requests that began before the active graph
changed, preventing a trace from being presented as another design's execution.

Every node reserves a right-side runtime pane. The pane displays the node output
or its not-executed state and owns vertical scrolling when its content exceeds
the node height. While the pointer is over that pane, the mouse wheel scrolls
the node output and does not zoom the graph canvas. UI code only renders the
typed trace; it does not reconstruct prompt values or provider behavior.

## Built-in compatibility

The read-only `builtin-default` profile is canonical code-owned data. Loading
or saving settings replaces any persisted profile with that ID, preventing an
old or modified local copy from changing the default.

Golden tests cover the complete canonical messages for:

- OpenAI-compatible explicit source with reference context;
- OpenAI-compatible automatic source with reference context;
- OpenAI-compatible without reference context;
- Hunyuan explicit source with reference context;
- Hunyuan automatic source with reference context;
- Hunyuan without reference context;
- semantic ASR instruction with and without recognition vocabulary;
- lexical ASR context containing only the recognition vocabulary.

These tests compare message roles and complete string contents, including every
space, newline, heading, boundary, and current-input label.

## Schema and validation

Graph schema version and canvas layout version are independent. Prompt Studio
has not shipped a public graph schema, so unsupported custom schema versions are
replaced with the canonical graph.

The WebSocket protocol carries `PromptNodeGraph` as a typed JSON object. A
backend validates a graph before activating it and reports malformed graphs as
client errors. Activation requires exactly one Request for each translation
provider target, at most one Request for each ASR delivery target, every Request
input to be connected, and the target's required variable to be reachable:
Current Input for translation and Recognition Context for lexical ASR context.
An ASR instruction may use source/expected languages without depending on
recognition vocabulary; the canonical instruction optionally incorporates it.
Requests must be on their matching provider page. Shared nodes may feed either
provider page; provider-specific nodes cannot cross into another provider page
or feed back into Shared. Malformed Compose placeholders, links to unused
Compose sockets, and referenced but unconnected sockets are rejected.

The editor exposes every referenced Compose socket plus one available input.
Connecting that input appends its `{n}` label to the Compose text and reveals
the next available input, up to ten total inputs. The label can then be moved
inside the text to control the exact composition order. An unconnected declared
input is itself the available input, so removing a wire does not create extra
sockets. The editor performs deterministic, height-aware, page-aware
auto-layout. Shared nodes keep one position across pages, while mutually
exclusive provider nodes reuse canvas space instead of reserving gaps for
hidden nodes. The canvas supports cursor-centered zoom, middle-button pan,
grid-snapped multi-node dragging, box selection, keyboard deletion, and wire
removal. These interactions change graph layout only; they do not alter prompt
execution semantics.

Each node has an execution-neutral purpose label. The canonical graph uses
semantic labels such as `EXPLICIT SOURCE INSTRUCTION` and `SELECT SYSTEM
PROMPT`, while newly created Compose labels follow the first line of their
text until named directly in the editable node header. At overview zoom the
editor keeps purpose titles visible and suppresses unreadable body previews.
Content nodes derive their height from wrapped text, and socket tooltips name
their connected source. Request nodes expose numbered role inputs such as `1
SYSTEM` and `2 USER`; their exact assembled messages appear in the runtime pane.

Node tones are derived from node kind and are not template configuration.
Inputs, variables, composition, conditions, and provider requests keep stable,
restrained grayscale roles; outgoing sockets and wires inherit the source kind
tone.
The toolbar and canvas context menu share the same categorized node catalogue.
Toolbar additions use the viewport center, while context-menu additions use the
pointer location. Neither operation rearranges existing nodes.

`supports_prompt_context` remains a serialized provider configuration field for
compatibility. Translation providers use it for optional reference facts.
ASR providers use the explicit `asr_prompt_mode` capability (`none`,
`instruction`, or `context_bias`); legacy ASR entries that only declared prompt
support migrate to `instruction`. `supports_vocabulary_bias` and
`vocabulary_weight` independently control structured vocabulary delivery. A
lexical provider may also declare `asr_context_max_chars`; runtime facts are
bounded before graph execution so the Request trace remains identical to the
text sent on the wire. A custom graph whose static composition still exceeds
that limit fails visibly instead of being silently truncated by an adapter.

On a context-window overflow or probable context leak, the backend re-executes
the same graph with empty optional reference facts. It must retain the provider
instruction, message roles, and current input.
