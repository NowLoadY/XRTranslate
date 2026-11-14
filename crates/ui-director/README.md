# UI Director

UI Director connects desktop application controls to agents, native CLIs and
automation clients. Declare each control once, reuse its normal handler, and
show what an operation actually read or changed.

Use it in editors, design tools, dashboards and other interactive applications.
The Rust core is independent of any GUI toolkit or model provider. The included
UI adapter supports **egui**; other toolkits can build adapters on the same core.

## How it fits together

| Layer | Responsibility |
| --- | --- |
| `ToolProvider`, `catalog`, `dispatch` | Declare operations, discover them on demand and forward calls. |
| `feedback` | Describe reads, actions, pending work, failures and actual content changes using stable target IDs. |
| `egui` feature | Register controls, execute through UI frames, expose current geometry and serve local clients. |
| `egui::feedback` | Associate feedback with visible targets; provide an optional overlay or data for a custom renderer. |
| `ui-director` executable / `cli` | Call the running application from a native command line. |

The host provides pages and tool providers. Business logic, permissions, model
connections and visual design remain in the host. No scripting runtime or agent
SDK is required.

## Try it

Run the example application:

```sh
cargo run --example basic --features egui -- 18921
```

In another terminal, discover and operate its controls:

```sh
cargo run --bin ui-director -- --port 18921 tools ui
cargo run --bin ui-director -- --port 18921 call ui.read '{}'
cargo run --bin ui-director -- --port 18921 call ui.set '{"target":"Draft","value":"Hello"}'
```

## Integrate

Enable the `egui` feature and follow [the complete example](examples/basic.rs).
Initialize the adapter once, connect its input and frame hooks, and dispatch
requests to the host's providers. The current adapter uses one root UI context
per process.

```rust,ignore
use ui_director::egui::{self as director, Control};

let response = ui.button("Save");
if Control::button("Save").response(ui, response).clicked() {
    document.save()?;
}

let response = ui.text_edit_singleline(&mut title);
director::text_response(ui, "Title", &mut title, response);
```

Put these declarations inside shared components. Pages then gain discovery,
CLI control and feedback by using those components normally. Custom operations
implement `ToolProvider` and join the host's provider list. Page routes and target
IDs are supplied by the host; the framework has no application-specific routes.

An active document or detail view adds its existing identity once per frame:

```rust,ignore
director::record_location(ui, &document.id, &document.title);
```

`ui.read`, `ui.pages` and `driver().location()` expose the same `Location`:
a page and an optional path of `{id, label}` segments. Nested detail containers
can add segments; leaving a view clears them automatically. An agent host can
attach this small snapshot to each user instruction, including follow-ups,
without sending the document contents or introducing another operation.
Location is contextual data, not an execution scope or a navigation command.

## Observe operations

Controls report reads and actions automatically. Value changes are observed on
the following render, after the normal handler runs. Clicking a button reports
activation, not completion of any background work it starts.

Document providers pass only affected `feedback::Item`s before and after a
successful mutation. `feedback::changes` identifies edits, additions and deletions;
deletions use the surviving container as their visual target. Register the same
IDs with `egui::feedback::record` when rendering document rows.

Use `egui::feedback::current(ctx)` for facts and current clipped rectangles, or
call `paint` for the wordless overlay with an accent and optional connector anchor.
Solid outlines locate the operation; blue pulses flow toward writes, lighter white
pulses flow back from reads. Waiting gently breathes; failure gives a brief red recoil.
The overlay shows no tool names, status labels or document excerpts. Detailed facts
remain available through `current` for custom renderers. Hidden targets have no
rectangle; coordinates refresh every frame. Feedback is bounded and transient,
without changing component layout or adding messages to the agent's context.

Hosts can also paint read-only tutorials from `driver().rendered_snapshot()`
after page layout with `egui::feedback::paint_guidance`. It shares the same
clipped geometry and renderer without replacing execution feedback. The host
owns tutorial timing and navigation; guidance can stop its animation between
operations and let the UI sleep.

## Protocol

In-process callers and the local HTTP server use the same `ToolCall`:

```json
{"name":"ui.read","arguments":{"limit":20}}
```

POST a call to the loopback server. Responses contain `success`, `message` and
optional `data`. A pending result supplies its own polling and cancellation
calls; poll for completion before relying on the operation's result. The native
CLI prints one response per invocation, including pending receipts.

Native automation clients can reuse `client::send` with a port and timeout;
the CLI uses the same loopback transport. Callers own polling and cancellation.

`GET /frame` and the timed frame stream support capture and reproducible recording.
There are no alternate mutation protocols.

## Develop

```sh
cargo check --lib --no-default-features
cargo test --features egui
```

## License

Licensed under AGPL-3.0-only; see [LICENSE](LICENSE).
