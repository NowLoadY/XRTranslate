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

Rendering consumes focused state and emits domain actions. Persistence, network
requests, and resource lifecycle remain with controllers or host capabilities.

## Layout and interaction

`ui::layout` owns adaptive flow, control sizing, column stacking, and window-size
policy. Components measure their content within the available space; pages
compose them without duplicating viewport heuristics. Wrapping and stacking take
precedence over window growth, while user window sizing remains respected.

Shared list geometry owns variable-height content, clipping, and scrolling.
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
