# UI architecture

Shared UI infrastructure owns visual language and common interaction behavior.
Pages and plugins compose these capabilities around their domain state.

## Presentation ownership

`ui::theme` owns persisted theme choices, visual tokens, and semantic animation
timings. Shared components resolve those choices into painting and interaction.
Pages do not maintain parallel theme rules or animation systems.

Reusable icons, controls, cards, menus, and feedback belong in shared components.
Common actions reuse the same SVG and control behavior across pages. Styling
and accessibility information stay with the component rather than being copied
into each caller.

Interactive components also own their Director declaration. Shared buttons,
choices, inputs and sliders register their rendered ID, value and supported
actions through `ui_director::egui::Control`; Agent and CLI actions then reach the
normal component handler. Use `components::button/selectable_label/selectable_value`
for native-looking controls, or add a `Control` declaration to a custom response.
Shared disclosures and numeric inputs follow the same path; `text_response` and
`number_response` adapt custom inputs while retaining their normal change handlers.
Pages do not maintain a separate button list or duplicate action handlers.
Discovery is paged, respects disabled state and excludes password fields.
Page discovery reuses the sidebar navigation catalogue. Rendered text is read in
bounded ranges; document-wide reading remains with the owning plugin.
The standalone [UI Director package](../crates/ui-director/README.md) owns this
adapter and its feedback. It observes real control values after handlers run;
application components retain their layout, drawing and animation. Document
adapters provide only affected items to the same feedback core. The optional
overlay and `feedback::current` consume execution facts and current geometry,
without adding messages to the agent's context.
The default overlay uses solid borders and directional pulses without text or
translucent trails; custom renderers can still read the full feedback facts.
See [Director CLI and Agent design](design/director-agent.md) for the shared tools.

Rendering consumes focused state and emits domain actions. Persistence, network
requests, and resource lifecycle remain with controllers or host capabilities.

Embedded plugin pages use that same GPU renderer on every platform. Plugins send
lossless drawings and texture deltas, retaining unchanged frames. Shared shader
callbacks transmit instance parameters, not rendered images or duplicate geometry.
The host owns pipelines and GPU textures; plugins own layout and interaction.
Independent games or custom renderers keep their own windows and GPU resources.

## Layout and interaction

`ui::layout` owns adaptive flow, control sizing, column stacking, and window-size
policy. Components measure their content within the available space; pages
compose them without duplicating viewport heuristics. Wrapping and stacking take
precedence over window growth, while user window sizing remains respected.

`ui::virtual_list` owns variable-height measurements, clipping, stable reading
positions, and smooth scrolling. Media subtitles and meeting records use its
single set of exact row heights per view for the current document. Revisions
reconcile rows by stable identity and content; only changed rows are measured
again. Width, scale, language, or style changes require fresh measurements.
Only visible rows prepare text for painting; the component retains no
font-atlas-bound text layouts. Plugins supply row content and editing behavior.
Shared graph-editor infrastructure owns selection, wiring, navigation, and edit
history; each studio supplies its domain rendering and validation.

Transient windows use shared rendering and size themselves to their content.
Page-specific behavior stays local only when it expresses a distinct user task.

Plugin landing pages foreground the next useful action and recent work through
shared entry components. Full lists remain available for search and organization.
The terminology library starts with domain cards; vocabulary inherits domain
rules without requiring a growing canvas of word-to-word connections.

## State and resources

Theme preferences are persisted. Animation state, layout measurements, and GPU
resources are transient and owned by the shared UI layer. Border renderers own
their visual margins and clipping needs, so pages do not compensate with local
offsets or duplicate fallback painting.

A new visual or interaction treatment should extend the existing component or
theme contract when it has the same meaning. Concrete colors, durations, sizing
formulas, shaders, and widget implementation belong in code.
