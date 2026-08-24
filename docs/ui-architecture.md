# UI Architecture

XRTranslate's shared UI infrastructure owns visual tokens, animation timing,
and border rendering policy. Page and plugin UI should consume these shared
capabilities instead of defining a new visual or interaction system locally.

## Theme ownership

`ui::theme::UiTheme` is the persisted theme contract. It is installed into the
egui context once per frame, and shared components read it through the theme
module. Theme variants are whole visual systems, not individual component
toggles. New variants must use the current theme schema and must not change
page identifiers. The current schema is authoritative; legacy border-theme
names and compatibility aliases are intentionally not part of the contract.

The `default` variant uses the standard egui visual treatment. The
`hand_drawn` variant currently enables the opt-in GPU effect backed by the WGPU
SDF/noise shader in `ui::organic_border`; future hand-drawn details belong to
this same variant rather than becoming separate theme settings. Components
select the implementation through `organic_border::show`; callers must not
branch on the variant or draw duplicate fallback borders. The organic renderer
is never a CPU fallback.

## Animation ownership

`ui::theme::AnimationTimings` defines semantic motion tokens such as hover,
active, selection, toggle, page transition, and click feedback. Shared controls
use the semantic helpers in `ui::animation::AnimationSystem`. A component may
choose a different semantic token, but should not introduce an unexplained
literal duration for a common interaction.

Dynamic data text uses the same shared contract. Call
`AnimationSystem::render_data_text` with a stable egui id and a semantic
activity value from `0.0` to `1.0`; the theme maps that value to opacity and a
small settling offset, while `AnimationTimings::data_text` controls the
transition. Callers provide freshness or activity, never raw alpha values, so
future dynamic status, feed, chart, or subtitle text can reuse the system
without duplicating animation logic.

Animation state remains transient egui context data. Theme preferences are
persisted in `ClientSettings`; animation state and GPU resources are not.

## Component contract

Shared components own layout safety as well as painting. In organic mode,
`organic_border::show` reserves the visual gutter required by SDF displacement
and antialiasing, so scroll areas clip neither content nor the border. Page and
plugin code should use `components::card`, `components::section`, and the
organic-border wrapper rather than adding page-specific border offsets.

## Responsive layout ownership

`ui::layout` owns responsive flow, minimum control sizing, column collapse, and
root-window growth. Short controls use `layout::flow_row`; repeated columns use
`layout::should_stack`; shared single-line controls derive their preferred
width from the rendered label through `layout::control_width`. Pages must not
use viewport-width guesses or fixed widths that can push siblings beyond their
container.

Related toolbar actions use `layout::flow_group`: the group starts a fresh row
when the remaining line is too short, then remains free to wrap internally if
the complete viewport is narrower than its preferred minimum. Canvas/world
coordinates stay clipped to their own viewport and never contribute to root
window sizing.

Dynamic text lists measure wrapped content at the current container width and
pass those measurements to `layout::show_variable_virtual_rows`. Virtualized
rows must not assume a fixed height or force text into a single-line preview;
the shared list geometry keeps clipping, visible-range selection, and
scroll-to-end consistent when content or window width changes.

Wrapping and stacking always run before window growth. If an individual widget
still cannot fit at its usable minimum, it reports that minimum through
`layout::require_content_size`. The shared animated-page boundary also detects
unhandled horizontal overflow as a safety net. `layout::finish_frame` merges
all requirements, caps them to the monitor bounds with a reserved margin, and sends eased egui
viewport size commands. It grows only the dimensions that are too small, does
not automatically shrink a user-sized window, and does not resize maximized or
fullscreen windows. The policy is shared by every visual theme.
