use crate::audio_studio::graph::{
    ApplicationSelection, AsrInputMode, AudioGraph, AudioNode, AudioNodeKind, AudioProcessor,
    DeviceId, GraphEndpoint, GraphPosition, LinkId, NodeId, PortId, SystemAudioCapture,
    SystemCapturePolicy, VoiceMeeterBus,
};
use crate::audio_studio::{
    AudioDeviceRole, AudioStudioLifecycle, AudioStudioPreset, AudioStudioUiAction,
    AudioStudioUiSnapshot, HostAudioDevice, HostAudioSnapshot, RouteRiskReport, RouteRiskSeverity,
    VoiceMeeterEdition, VoiceMeeterSnapshot,
};
use crate::ui::{
    graph_canvas,
    graph_editor::{
        GraphEditHistory, GraphEditorState, GraphShortcut, LayeredLayoutOptions, LayoutNode,
        layered_layout,
    },
    graph_style,
};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Frame, Id, Margin, Pos2, Rect, RichText, Sense,
    Stroke, Vec2,
};
use std::collections::{HashMap, HashSet};

const NODE_WIDTH: f32 = 232.0;
const NODE_BASE_HEIGHT: f32 = 130.0;
const NODE_DEVICE_HEIGHT: f32 = 166.0;
const NODE_SYSTEM_AUDIO_HEIGHT: f32 = 208.0;
const NODE_HEADER_HEIGHT: f32 = 50.0;
const PORT_RADIUS: f32 = 6.0;
const VOICEMEETER_GAME_MIC_EXTRA_HEIGHT: f32 = 78.0;

const INK: Color32 = graph_style::INK;
const MUTED: Color32 = graph_style::MUTED;
const CANVAS_FILL: Color32 = graph_style::CANVAS_FILL;
const CANVAS_BORDER: Color32 = graph_style::CANVAS_BORDER;
const GRID: Color32 = graph_style::GRID;
const LINK: Color32 = graph_style::LINK;
const LINK_INACTIVE: Color32 = graph_style::LINK_INACTIVE;
const LINK_SELECTED: Color32 = graph_style::LINK_SELECTED;
const INPUT_PORT: Color32 = graph_style::GRAPH_ACCENT;
const OUTPUT_PORT: Color32 = graph_style::GRAPH_ACCENT;
const SIDECHAIN_PORT: Color32 = Color32::from_rgb(116, 108, 88);
const ERROR: Color32 = Color32::from_rgb(132, 62, 62);
const WARNING: Color32 = Color32::from_rgb(143, 105, 44);
const SUCCESS: Color32 = Color32::from_rgb(48, 91, 78);

#[derive(Clone, Debug, Default)]
struct AudioStudioCanvasState {
    editor: GraphEditorState<NodeId, LinkId, GraphEndpoint, GraphEndpoint>,
    history: GraphEditHistory<AudioGraph>,
    pending_preset_load: Option<AudioStudioPreset>,
    pending_safe_reset: bool,
    signal_envelopes: HashMap<LinkId, f32>,
    last_signal_update_seconds: Option<f64>,
}

impl std::ops::Deref for AudioStudioCanvasState {
    type Target = GraphEditorState<NodeId, LinkId, GraphEndpoint, GraphEndpoint>;

    fn deref(&self) -> &Self::Target {
        &self.editor
    }
}

impl std::ops::DerefMut for AudioStudioCanvasState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.editor
    }
}

#[derive(Clone, Copy)]
struct NodePalette {
    fill: Color32,
    header: Color32,
    accent: Color32,
}

fn node_palette(kind: &AudioNodeKind) -> NodePalette {
    let (fill, header, accent) = match kind {
        AudioNodeKind::Microphone { .. } => ((250, 251, 249), (220, 224, 220), (100, 108, 103)),
        AudioNodeKind::SystemAudio { .. } => ((248, 251, 251), (210, 224, 226), (76, 112, 118)),
        AudioNodeKind::TextToSpeech => ((251, 249, 252), (224, 214, 226), (118, 88, 122)),
        AudioNodeKind::Media { .. } => ((252, 251, 247), (225, 221, 210), (116, 108, 88)),
        AudioNodeKind::Mixer => ((252, 251, 247), (225, 221, 210), (116, 108, 88)),
        AudioNodeKind::Processing { .. } => ((249, 250, 253), (215, 222, 235), (75, 91, 140)),
        AudioNodeKind::AsrTap => ((249, 251, 248), (218, 229, 214), (69, 118, 65)),
        AudioNodeKind::MonitorOutput { .. } => ((250, 251, 249), (220, 224, 220), (100, 108, 103)),
        AudioNodeKind::GameMicrophoneOutput { .. } => {
            ((248, 250, 252), (215, 222, 228), (96, 107, 117))
        }
    };
    NodePalette {
        fill: Color32::from_rgb(fill.0, fill.1, fill.2),
        header: Color32::from_rgb(header.0, header.1, header.2),
        accent: Color32::from_rgb(accent.0, accent.1, accent.2),
    }
}

fn is_gain_node(node: &AudioNode) -> bool {
    matches!(
        node.kind,
        AudioNodeKind::Processing {
            processor: AudioProcessor::Gain { .. },
        }
    )
}

fn gain_node_value(node: &AudioNode) -> f32 {
    if let AudioNodeKind::Processing {
        processor: AudioProcessor::Gain { gain_db },
    } = node.kind
    {
        gain_db
    } else {
        0.0
    }
}

fn node_size(node: &AudioNode) -> Vec2 {
    if is_gain_node(node) {
        return Vec2::splat(52.0);
    }
    let device_node = matches!(
        node.kind,
        AudioNodeKind::Microphone { .. }
            | AudioNodeKind::SystemAudio { .. }
            | AudioNodeKind::MonitorOutput { .. }
            | AudioNodeKind::GameMicrophoneOutput { .. }
    );
    let height = if matches!(node.kind, AudioNodeKind::SystemAudio { .. }) {
        NODE_SYSTEM_AUDIO_HEIGHT
    } else if device_node {
        NODE_DEVICE_HEIGHT
    } else if matches!(
        node.kind,
        AudioNodeKind::TextToSpeech | AudioNodeKind::Media { .. }
    ) {
        166.0
    } else {
        NODE_BASE_HEIGHT
    };
    Vec2::new(NODE_WIDTH, height)
}

fn node_size_in_graph(
    graph: &AudioGraph,
    node: &AudioNode,
    host_audio: &HostAudioSnapshot,
) -> Vec2 {
    let mut size = node_size(node);
    if matches!(node.kind, AudioNodeKind::Mixer) {
        let rows = input_ports(graph, node).len().max(1) as f32;
        size.y = size.y.max(NODE_HEADER_HEIGHT + 22.0 + rows * 30.0 + 12.0);
    }
    if voicemeeter_target(node, host_audio).is_some() {
        size.y += VOICEMEETER_GAME_MIC_EXTRA_HEIGHT;
    }
    size
}

#[cfg(test)]
fn node_size_for_host(node: &AudioNode, host_audio: &HostAudioSnapshot) -> Vec2 {
    let mut size = node_size(node);
    if voicemeeter_target(node, host_audio).is_some() {
        size.y += VOICEMEETER_GAME_MIC_EXTRA_HEIGHT;
    }
    size
}

struct VoiceMeeterTarget<'a> {
    device: &'a HostAudioDevice,
    snapshot: &'a VoiceMeeterSnapshot,
}

fn voicemeeter_target<'a>(
    node: &AudioNode,
    host_audio: &'a HostAudioSnapshot,
) -> Option<VoiceMeeterTarget<'a>> {
    if !matches!(node.kind, AudioNodeKind::GameMicrophoneOutput { .. }) {
        return None;
    }
    let snapshot = host_audio.voicemeeter.as_ref()?;
    let candidates = host_audio
        .devices
        .iter()
        .filter(|device| device.role == AudioDeviceRole::GameMicrophoneSink)
        .collect::<Vec<_>>();
    let device = node
        .kind
        .selected_device()
        .and_then(|selected| {
            candidates
                .iter()
                .find(|device| &device.id == selected)
                .copied()
        })
        .or_else(|| match candidates.as_slice() {
            [device] => Some(*device),
            _ => None,
        })?;
    device.voicemeeter_strip_index?;
    Some(VoiceMeeterTarget { device, snapshot })
}

fn selected_voicemeeter_bus(node: &AudioNode) -> VoiceMeeterBus {
    match node.kind {
        AudioNodeKind::GameMicrophoneOutput {
            voicemeeter_bus, ..
        } => voicemeeter_bus.unwrap_or(VoiceMeeterBus::B1),
        _ => VoiceMeeterBus::B1,
    }
}

fn paired_recording_device(bus: VoiceMeeterBus) -> &'static str {
    match bus {
        VoiceMeeterBus::B1 => "Voicemeeter Out B1",
        VoiceMeeterBus::B2 => "Voicemeeter AUX Out B2",
        VoiceMeeterBus::B3 => "Voicemeeter VAIO3 Out B3",
    }
}

fn node_kind_label(kind: &AudioNodeKind) -> &'static str {
    match kind {
        AudioNodeKind::Microphone { .. } => "Capture · Microphone",
        AudioNodeKind::SystemAudio {
            capture: SystemAudioCapture::Endpoint { .. },
        } => "Capture · Output endpoint",
        AudioNodeKind::SystemAudio {
            capture: SystemAudioCapture::Application { .. },
        } => "Capture · Application audio",
        AudioNodeKind::TextToSpeech => "Source · TTS",
        AudioNodeKind::Media { .. } => "Source · Media / BGM",
        AudioNodeKind::Mixer => "Route · Mixer",
        AudioNodeKind::Processing { .. } => "DSP · Processor",
        AudioNodeKind::AsrTap => "Sink · ASR",
        AudioNodeKind::MonitorOutput { .. } => "Sink · Monitor",
        AudioNodeKind::GameMicrophoneOutput { .. } => "Output · App microphone",
    }
}

fn node_description(graph: &AudioGraph, node: &AudioNode) -> String {
    match &node.kind {
        AudioNodeKind::Microphone { .. } => "Your live voice".into(),
        AudioNodeKind::SystemAudio {
            capture: SystemAudioCapture::Endpoint { capture_policy, .. },
        } => match capture_policy {
            SystemCapturePolicy::AllEndpointAudio => {
                "Every app playing on this output device".into()
            }
            SystemCapturePolicy::ExcludeOwnProcessAudio => {
                "Endpoint audio; XRTranslate audio excluded".into()
            }
            SystemCapturePolicy::SuppressDuringOwnTts => {
                "Endpoint audio; ASR pauses during XRTranslate TTS".into()
            }
        },
        AudioNodeKind::SystemAudio {
            capture: SystemAudioCapture::Application { application, .. },
        } => application.as_ref().map_or_else(
            || "Choose one application's audio".into(),
            |application| format!("Only {} and its child processes", application.display_name),
        ),
        AudioNodeKind::TextToSpeech => "Synthesized translation speech".into(),
        AudioNodeKind::Media {
            source,
            loop_playback,
        } => match source {
            Some(source) if *loop_playback => format!("Looping · {source}"),
            Some(source) => source.clone(),
            None => "Choose BGM in Media Player".into(),
        },
        AudioNodeKind::Mixer => downstream_asr_input_mode(graph, &node.id).map_or_else(
            || "Combine synchronized audio streams".into(),
            |input_mode| {
                format!(
                    "Available recognition inputs · Active: {}",
                    input_mode.label()
                )
            },
        ),
        AudioNodeKind::Processing { processor } => format!("{processor:?}"),
        AudioNodeKind::AsrTap => current_asr_input_mode(graph).map_or_else(
            || "No recognition input is switched on".into(),
            |input_mode| format!("Active recognition path · {}", input_mode.label()),
        ),
        AudioNodeKind::MonitorOutput { .. } => "What you hear locally".into(),
        AudioNodeKind::GameMicrophoneOutput { .. } => {
            "XRTranslate mix → microphone input of another app".into()
        }
    }
}

fn downstream_asr_input_mode(graph: &AudioGraph, start: &NodeId) -> Option<AsrInputMode> {
    let mut visited = HashSet::new();
    let mut pending = vec![start.clone()];
    while let Some(node_id) = pending.pop() {
        if !visited.insert(node_id.clone()) {
            continue;
        }
        if let Some(node) = graph.node(&node_id)
            && !node.bypassed
            && matches!(node.kind, AudioNodeKind::AsrTap)
        {
            return current_asr_input_mode(graph);
        }
        pending.extend(
            graph
                .links
                .iter()
                .filter(|link| link.from.node_id == node_id)
                .filter(|link| {
                    graph
                        .node(&link.to.node_id)
                        .is_some_and(|node| !node.bypassed)
                })
                .map(|link| link.to.node_id.clone()),
        );
    }
    None
}

fn current_asr_input_mode(graph: &AudioGraph) -> Option<AsrInputMode> {
    crate::audio_studio::controller::derive_asr_input_mode(graph).ok()
}

fn input_ports(graph: &AudioGraph, node: &AudioNode) -> Vec<PortId> {
    if matches!(node.kind, AudioNodeKind::Mixer) {
        let mut ports = graph
            .links
            .iter()
            .filter(|link| link.to.node_id == node.id)
            .map(|link| link.to.port_id.clone())
            .collect::<Vec<_>>();
        ports.sort_by(|left, right| {
            left.mixer_input_index()
                .cmp(&right.mixer_input_index())
                .then_with(|| left.0.cmp(&right.0))
        });
        ports.dedup();
        let placeholder = PortId::mixer_input(
            ports
                .iter()
                .filter_map(PortId::mixer_input_index)
                .max()
                .map_or(0, |index| index.saturating_add(1)),
        );
        ports.push(placeholder);
        return ports;
    }
    let mut ports = Vec::new();
    if node.kind.accepts_input(&PortId::input()) {
        ports.push(PortId::input());
    }
    if node.kind.accepts_input(&PortId::sidechain()) {
        ports.push(PortId::sidechain());
    }
    ports
}

fn output_ports(node: &AudioNode) -> Vec<PortId> {
    node.kind
        .provides_output(&PortId::audio())
        .then(|| PortId::audio())
        .into_iter()
        .collect()
}

fn port_color(port: &PortId, output: bool) -> Color32 {
    if port.0 == PortId::SIDECHAIN {
        SIDECHAIN_PORT
    } else if output {
        OUTPUT_PORT
    } else {
        INPUT_PORT
    }
}

fn port_label(port: &PortId, output: bool) -> &'static str {
    if port.0 == PortId::SIDECHAIN {
        "Sidechain"
    } else if output {
        "Audio out"
    } else {
        "Audio in"
    }
}

fn graph_bounds(graph: &AudioGraph, host_audio: &HostAudioSnapshot) -> Option<Rect> {
    graph.nodes.iter().fold(None, |bounds, node| {
        let rect = Rect::from_min_size(
            Pos2::new(node.position.x, node.position.y),
            node_size_in_graph(graph, node, host_audio),
        );
        Some(bounds.map_or(rect, |current: Rect| current.union(rect)))
    })
}

fn auto_layout_graph(graph: &AudioGraph, host_audio: &HostAudioSnapshot) -> AudioGraph {
    let mut arranged = graph.clone();
    let movements = layered_layout(
        graph.nodes.iter().map(|node| LayoutNode {
            id: node.id.clone(),
            size: node_size_in_graph(graph, node, host_audio),
        }),
        graph
            .links
            .iter()
            .map(|link| (link.from.node_id.clone(), link.to.node_id.clone())),
        LayeredLayoutOptions {
            horizontal_gap: 160.0,
            vertical_gap: 64.0,
            ..LayeredLayoutOptions::default()
        },
    );
    for movement in movements {
        if let Some(node) = arranged
            .nodes
            .iter_mut()
            .find(|node| node.id == movement.node_id)
        {
            node.position = GraphPosition {
                x: movement.position[0],
                y: movement.position[1],
            };
        }
    }
    arranged
}

fn display_position(state: &AudioStudioCanvasState, node: &AudioNode) -> GraphPosition {
    let position = state.display_position(&node.id, [node.position.x, node.position.y]);
    GraphPosition {
        x: position[0],
        y: position[1],
    }
}

fn node_rect(
    graph: &AudioGraph,
    canvas: Rect,
    state: &AudioStudioCanvasState,
    node: &AudioNode,
    host_audio: &HostAudioSnapshot,
) -> Rect {
    let position = display_position(state, node);
    state.canvas.graph_rect(
        canvas,
        [position.x, position.y],
        node_size_in_graph(graph, node, host_audio),
    )
}

fn port_position(
    graph: &AudioGraph,
    node: &AudioNode,
    rect: Rect,
    port: &PortId,
    output: bool,
    zoom: f32,
) -> Pos2 {
    let y = if is_gain_node(node) {
        rect.center().y
    } else if port.0 == PortId::SIDECHAIN {
        rect.bottom() - 16.0 * zoom
    } else if !output && matches!(node.kind, AudioNodeKind::Mixer) {
        let row = input_ports(graph, node)
            .iter()
            .position(|candidate| candidate == port)
            .unwrap_or_default();
        rect.top() + (NODE_HEADER_HEIGHT + 22.0 + row as f32 * 30.0) * zoom
    } else {
        rect.top() + (NODE_HEADER_HEIGHT + 20.0) * zoom
    };
    Pos2::new(if output { rect.right() } else { rect.left() }, y)
}

fn endpoint_positions(
    graph: &AudioGraph,
    canvas: Rect,
    state: &AudioStudioCanvasState,
    host_audio: &HostAudioSnapshot,
) -> HashMap<GraphEndpoint, Pos2> {
    let mut positions = HashMap::new();
    for node in &graph.nodes {
        let rect = node_rect(graph, canvas, state, node, host_audio);
        for port in input_ports(graph, node) {
            positions.insert(
                GraphEndpoint {
                    node_id: node.id.clone(),
                    port_id: port.clone(),
                },
                port_position(graph, node, rect, &port, false, state.canvas.zoom),
            );
        }
        for port in output_ports(node) {
            positions.insert(
                GraphEndpoint {
                    node_id: node.id.clone(),
                    port_id: port.clone(),
                },
                port_position(graph, node, rect, &port, true, state.canvas.zoom),
            );
        }
    }
    positions
}

fn state_id(ui: &egui::Ui) -> Id {
    ui.make_persistent_id("audio_studio_canvas_state")
}

#[derive(Clone, Debug)]
enum CanvasCommand {
    DiscoverApplications,
    ReplaceGraph(AudioGraph),
    MoveNode {
        node_id: NodeId,
        position: GraphPosition,
    },
    CommitWire(crate::ui::graph_editor::WireCommit<GraphEndpoint, GraphEndpoint, LinkId>),
    DeleteLink(LinkId),
    RemoveNode(NodeId),
    SetNodeDevice {
        node_id: NodeId,
        device_id: Option<DeviceId>,
    },
    SetSystemAudioCapture {
        node_id: NodeId,
        capture: SystemAudioCapture,
    },
    SetLinkEnabled {
        link_id: LinkId,
        enabled: bool,
    },
    SetNodeVoiceMeeterBus {
        node_id: NodeId,
        bus: Option<VoiceMeeterBus>,
    },
    SetNodeGain {
        node_id: NodeId,
        gain_db: f32,
    },
    ChooseMedia(NodeId),
    EnqueueTts {
        node_id: NodeId,
        text: String,
    },
}

fn render_graph_canvas(
    snapshot: &AudioStudioUiSnapshot,
    ui: &mut egui::Ui,
    state: &mut AudioStudioCanvasState,
    commands: &mut Vec<CanvasCommand>,
    actions: &mut Vec<AudioStudioUiAction>,
) {
    let graph = &snapshot.selected_graph;
    if state.editor.reset_for_graph(graph.id.0.clone()) {
        state.history.clear();
    }

    Frame::new()
        .fill(CANVAS_FILL)
        .stroke(Stroke::new(1.0, CANVAS_BORDER))
        .corner_radius(CornerRadius::same(3))
        .inner_margin(Margin::same(4))
        .show(ui, |ui| {
            let size = Vec2::new(ui.available_width(), ui.available_height().max(240.0));
            let (canvas, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
            state.canvas.canvas_size = canvas.size();

            if state.canvas.fit_pending {
                if let Some(bounds) = graph_bounds(graph, &snapshot.host_audio) {
                    state
                        .canvas
                        .fit_to_bounds(bounds, canvas.size(), Vec2::new(NODE_WIDTH, 120.0));
                } else {
                    state.canvas.pan = Vec2::new(36.0, 36.0);
                    state.canvas.zoom = 1.0;
                }
                state.canvas.fit_pending = false;
            }

            let pointer = response.interact_pointer_pos();
            let pointer_over_gain_node = pointer.is_some_and(|pointer| {
                graph
                    .nodes
                    .iter()
                    .any(|node| {
                        is_gain_node(node)
                            && node_rect(graph, canvas, state, node, &snapshot.host_audio)
                                .contains(pointer)
                    })
            });

            let mut canvas_ui = graph_canvas::canvas_viewport(ui, canvas);
            state.handle_navigation(canvas, &response, &canvas_ui, true, !pointer_over_gain_node);
            graph_canvas::paint_grid(&canvas_ui, canvas, &state.canvas, GRID);

            let positions = endpoint_positions(graph, canvas, state, &snapshot.host_audio);
            let pointer_over_node = pointer.is_some_and(|pointer| {
                graph
                    .nodes
                    .iter()
                    .any(|node| {
                        node_rect(graph, canvas, state, node, &snapshot.host_audio)
                            .contains(pointer)
                    })
            });
            let pointer_over_link = pointer.is_some_and(|pointer| {
                crate::ui::graph_editor::closest_link(
                    pointer,
                    graph.links.iter().filter_map(|link| {
                        let (from, to) = (positions.get(&link.from)?, positions.get(&link.to)?);
                        Some((link.id.clone(), graph_canvas::bezier_points(*from, *to)))
                    }),
                    11.0,
                )
                .is_some()
            });
            let wire_cancelled = state.handle_secondary_wire_cancel(canvas, &canvas_ui);
            if response.secondary_clicked()
                && !wire_cancelled
                && !pointer_over_node
                && !pointer_over_link
            {
                state.add_node_center = response
                    .interact_pointer_pos()
                    .map(|pointer| state.canvas.graph_position(canvas, pointer));
            }
            if !wire_cancelled
                && !pointer_over_node
                && !pointer_over_link
                && !state.wire_active()
            {
                response.context_menu(|ui| {
                    render_add_node_menu(snapshot, state, ui, actions);
                });
            }
            let selectable_nodes = graph
                .nodes
                .iter()
                .map(|node| {
                    (
                        node.id.clone(),
                        node_rect(graph, canvas, state, node, &snapshot.host_audio),
                    )
                })
                .collect::<Vec<_>>();
            state.handle_canvas_selection(
                &response,
                &canvas_ui,
                true,
                pointer_over_node,
                pointer_over_link,
                selectable_nodes,
            );
            let animated_signals = update_animated_signals(snapshot, state, &canvas_ui);
            render_links(
                graph,
                &snapshot.risk_report,
                &canvas_ui,
                &positions,
                &animated_signals,
                state,
                &response,
            );
            for node in &graph.nodes {
                render_node(
                    graph,
                    node,
                    &snapshot.host_audio,
                    &snapshot.risk_report,
                    canvas,
                    &mut canvas_ui,
                    state,
                    commands,
                );
            }

            let positions = endpoint_positions(graph, canvas, state, &snapshot.host_audio);
            render_wire_preview(&canvas_ui, state, &positions);
            finish_wire_drag(graph, &canvas_ui, state, &positions, commands);

            match crate::ui::graph_editor::shortcut(&canvas_ui) {
                Some(GraphShortcut::Delete) => {
                    let (nodes, links) = state.take_selection();
                    commands.extend(nodes.into_iter().map(CanvasCommand::RemoveNode));
                    commands.extend(links.into_iter().map(CanvasCommand::DeleteLink));
                }
                Some(GraphShortcut::Cancel) => state.cancel_current_operation(),
                Some(GraphShortcut::Undo) => {
                    if let Some(previous) = state.history.undo(graph.clone()) {
                        commands.push(CanvasCommand::ReplaceGraph(previous));
                    }
                }
                Some(GraphShortcut::Redo) => {
                    if let Some(next) = state.history.redo(graph.clone()) {
                        commands.push(CanvasCommand::ReplaceGraph(next));
                    }
                }
                None => {}
            }

            if let Some(selection) = state.selection_rect() {
                graph_canvas::paint_selection_box(&canvas_ui, selection, LINK_SELECTED);
            }
            let hints = vec![
                format!(
                    "Navigate · Space + left drag / middle drag to pan · Mouse wheel to zoom · {:.0}%",
                    state.canvas.zoom * 100.0
                ),
                "Select · Left drag on canvas to box select · Shift + click to multi-select"
                    .to_owned(),
                "Connect · Drag socket to connect / unplug · Click empty space to cancel wire"
                    .to_owned(),
                "Actions · Del to delete · Ctrl+Z: undo · Ctrl+Y: redo".to_owned(),
            ];
            crate::ui::graph_editor::paint_navigation_hint(&canvas_ui, canvas, &hints, MUTED);
        });
}

fn render_links(
    graph: &AudioGraph,
    risks: &RouteRiskReport,
    ui: &egui::Ui,
    positions: &HashMap<GraphEndpoint, Pos2>,
    animated_signals: &AnimatedLinkSignals,
    state: &mut AudioStudioCanvasState,
    canvas_response: &egui::Response,
) {
    let pointer = canvas_response.interact_pointer_pos();
    let closest = pointer.and_then(|pointer| {
        crate::ui::graph_editor::closest_link(
            pointer,
            graph.links.iter().filter_map(|link| {
                let (from, to) = (positions.get(&link.from)?, positions.get(&link.to)?);
                Some((link.id.clone(), graph_canvas::bezier_points(*from, *to)))
            }),
            11.0,
        )
    });
    for link in &graph.links {
        let (Some(from), Some(to)) = (positions.get(&link.from), positions.get(&link.to)) else {
            continue;
        };
        let points = graph_canvas::bezier_points(*from, *to);
        let selected = state.selected_links.contains(&link.id);
        let risk_severity = risks
            .risks
            .iter()
            .filter(|risk| risk.link_ids.contains(&link.id))
            .map(|risk| risk.severity)
            .min_by_key(|severity| match severity {
                RouteRiskSeverity::Blocking => 0,
                RouteRiskSeverity::Warning => 1,
                RouteRiskSeverity::Info => 2,
            });
        let risk_color = match risk_severity {
            Some(RouteRiskSeverity::Blocking) => ERROR,
            Some(RouteRiskSeverity::Warning) => WARNING,
            Some(RouteRiskSeverity::Info) => Color32::from_rgb(55, 105, 160),
            None => LINK,
        };
        if !link.enabled && !selected {
            graph_canvas::paint_dashed_wire(ui, points, Stroke::new(1.5, LINK_INACTIVE));
        } else {
            graph_canvas::paint_wire(
                ui,
                points,
                Stroke::new(
                    if selected { 3.0 } else { 2.0 },
                    if selected { LINK_SELECTED } else { risk_color },
                ),
            );
            if animated_signals.active.contains(&link.id) {
                let level = animated_signals
                    .levels
                    .get(&link.id)
                    .copied()
                    .unwrap_or(0.0);
                if level >= 0.001 {
                    paint_signal_particles(
                        ui,
                        points,
                        animated_signals.time_seconds,
                        level,
                        if selected { LINK_SELECTED } else { risk_color },
                        &link.id,
                    );
                }
            }
        }
    }
    if canvas_response.clicked_by(egui::PointerButton::Primary) {
        if let Some(link_id) = closest {
            let extend = ui.input(|input| input.modifiers.shift || input.modifiers.ctrl);
            state.select_link(link_id, extend);
        }
    }
}

struct AnimatedLinkSignals {
    active: HashSet<LinkId>,
    levels: HashMap<LinkId, f32>,
    time_seconds: f64,
}

fn update_animated_signals(
    snapshot: &AudioStudioUiSnapshot,
    state: &mut AudioStudioCanvasState,
    ui: &egui::Ui,
) -> AnimatedLinkSignals {
    let graph = &snapshot.selected_graph;
    let active = active_link_ids(snapshot);
    let targets = link_signal_targets(graph, snapshot);
    let now = ui.input(|input| input.time);
    let elapsed = state
        .last_signal_update_seconds
        .replace(now)
        .map_or(1.0 / 30.0, |previous| (now - previous).clamp(0.0, 0.25)) as f32;

    state
        .signal_envelopes
        .retain(|link_id, _| graph.links.iter().any(|link| &link.id == link_id));
    for link in &graph.links {
        let target = if active.contains(&link.id) {
            targets.get(&link.id).copied().unwrap_or(0.0)
        } else {
            0.0
        }
        .clamp(0.0, 1.0);
        let envelope = state.signal_envelopes.entry(link.id.clone()).or_default();
        let time_constant = if target > *envelope { 0.055 } else { 0.32 };
        let blend = 1.0 - (-elapsed / time_constant).exp();
        *envelope += (target - *envelope) * blend;
        if envelope.abs() < 0.000_1 {
            *envelope = 0.0;
        }
    }

    if !active.is_empty() {
        // Metering is already performed in the audio callbacks. Repainting at
        // 40 Hz keeps motion smooth without tying UI work to the PCM rate.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(25));
    }

    AnimatedLinkSignals {
        levels: active
            .iter()
            .map(|link_id| {
                (
                    link_id.clone(),
                    state.signal_envelopes.get(link_id).copied().unwrap_or(0.0),
                )
            })
            .collect(),
        active,
        time_seconds: now,
    }
}

fn active_link_ids(snapshot: &AudioStudioUiSnapshot) -> HashSet<LinkId> {
    let live_routing = snapshot.live_routing_matches_graph
        && matches!(snapshot.lifecycle, AudioStudioLifecycle::Active { .. });
    let translation = snapshot.host_audio.translation_workflow_running;
    if !live_routing && !translation {
        return HashSet::new();
    }

    let graph = &snapshot.selected_graph;
    let mut pending = graph
        .nodes
        .iter()
        .filter(|node| {
            (translation && matches!(node.kind, AudioNodeKind::AsrTap))
                || (live_routing
                    && matches!(
                        node.kind,
                        AudioNodeKind::MonitorOutput { .. }
                            | AudioNodeKind::GameMicrophoneOutput { .. }
                    ))
        })
        .map(|node| node.id.clone())
        .collect::<Vec<_>>();
    let mut visited_nodes = HashSet::new();
    let mut links = HashSet::new();
    while let Some(node_id) = pending.pop() {
        if !visited_nodes.insert(node_id.clone()) {
            continue;
        }
        for link in graph
            .links
            .iter()
            .filter(|link| link.enabled && link.to.node_id == node_id)
        {
            links.insert(link.id.clone());
            pending.push(link.from.node_id.clone());
        }
    }
    links
}

fn link_signal_targets(
    graph: &AudioGraph,
    snapshot: &AudioStudioUiSnapshot,
) -> HashMap<LinkId, f32> {
    let mut memo = HashMap::<NodeId, f32>::new();
    let mut visiting = HashSet::<NodeId>::new();
    graph
        .links
        .iter()
        .filter(|link| link.enabled)
        .map(|link| {
            let level = node_output_signal(
                graph,
                &link.from.node_id,
                snapshot,
                &mut memo,
                &mut visiting,
            );
            (link.id.clone(), level.clamp(0.0, 1.0))
        })
        .collect()
}

fn node_output_signal(
    graph: &AudioGraph,
    node_id: &NodeId,
    snapshot: &AudioStudioUiSnapshot,
    memo: &mut HashMap<NodeId, f32>,
    visiting: &mut HashSet<NodeId>,
) -> f32 {
    if let Some(level) = memo.get(node_id) {
        return *level;
    }
    if !visiting.insert(node_id.clone()) {
        return 0.0;
    }
    let Some(node) = graph.node(node_id) else {
        visiting.remove(node_id);
        return 0.0;
    };
    let level = match &node.kind {
        AudioNodeKind::Microphone { .. } => snapshot.signal_levels.microphone,
        AudioNodeKind::SystemAudio { .. } => snapshot.signal_levels.system_audio,
        AudioNodeKind::TextToSpeech => snapshot.signal_levels.tts,
        AudioNodeKind::Media { .. } => 0.0,
        AudioNodeKind::Mixer => {
            graph
                .links
                .iter()
                .filter(|link| {
                    link.enabled
                        && link.to.node_id == *node_id
                        && link.to.port_id.0 != PortId::SIDECHAIN
                })
                .map(|link| node_output_signal(graph, &link.from.node_id, snapshot, memo, visiting))
                .map(|input| input * input)
                .sum::<f32>()
                .sqrt()
                .clamp(0.0, 1.0)
        }
        AudioNodeKind::Processing { processor } => {
            let input_signal = graph
                .links
                .iter()
                .filter(|link| {
                    link.enabled
                        && link.to.node_id == *node_id
                        && link.to.port_id.0 != PortId::SIDECHAIN
                })
                .map(|link| node_output_signal(graph, &link.from.node_id, snapshot, memo, visiting))
                .fold(0.0_f32, |acc, val| acc.max(val));
            let multiplier = match processor {
                AudioProcessor::Gain { gain_db } => 10.0_f32.powf(gain_db / 20.0),
                _ => 1.0,
            };
            (input_signal * multiplier).clamp(0.0, 1.0)
        }
        AudioNodeKind::AsrTap
        | AudioNodeKind::MonitorOutput { .. }
        | AudioNodeKind::GameMicrophoneOutput { .. } => 0.0,
    };
    visiting.remove(node_id);
    memo.insert(node_id.clone(), level);
    level
}

fn paint_signal_particles(
    ui: &egui::Ui,
    points: [Pos2; 4],
    time_seconds: f64,
    level: f32,
    color: Color32,
    link_id: &LinkId,
) {
    if level < 0.001 {
        return;
    }

    const PARTICLES: usize = 4;
    const TRAIL_RADII: [f32; 3] = [1.0, 0.62, 0.34];

    // High sensitivity & large dynamic range:
    // Only active when sound is present; completely silent routes show zero particles.
    let boosted_level = ((level - 0.001) / 0.999 * 2.2).clamp(0.0, 1.0);
    let dynamic_factor = boosted_level.sqrt() * 0.75 + (boosted_level * boosted_level) * 0.25;
    let radius = 1.8 + dynamic_factor * 11.7;
    let trail_gap = 0.015 + dynamic_factor * 0.024;

    // True physical forward integration (Phase Accumulator):
    // Speed varies dynamically from 0.18 (idle) to 0.66 (energetic audio rush).
    // By accumulating delta phase: phase = (prev_phase + dt * speed),
    // particles accelerate strictly forward when speaking and smoothly decelerate without rewinding.
    let id = ui.make_persistent_id(("audio_signal_particle_phase", &link_id.0));
    let (last_time, prev_phase) = ui.memory(|m| {
        m.data
            .get_temp::<(f64, f32)>(id)
            .unwrap_or_else(|| {
                let id_offset = (link_id.0.bytes().fold(0_u32, |hash, byte| {
                    hash.wrapping_mul(31).wrapping_add(u32::from(byte))
                }) as f64
                    / u32::MAX as f64) as f32;
                (time_seconds, id_offset)
            })
    });

    let dt = (time_seconds - last_time).clamp(0.0, 0.1) as f32;
    let speed = 0.18 + dynamic_factor * 0.48;
    let phase = (prev_phase + dt * speed).fract();

    ui.memory_mut(|m| {
        m.data.insert_temp(id, (time_seconds, phase));
    });

    for particle in 0..PARTICLES {
        let head = (phase + particle as f32 / PARTICLES as f32).fract();

        // Ambient glow halo on energetic voice/audio peaks
        if boosted_level > 0.05 {
            let lead_point = graph_canvas::cubic_point(points, head);
            let glow_alpha = (((boosted_level - 0.05) / 0.95) * 90.0).clamp(0.0, 85.0) as u8;
            let glow_color =
                Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), glow_alpha);
            ui.painter()
                .circle_filled(lead_point, radius * 1.85, glow_color);
        }

        for (trail_index, scale) in TRAIL_RADII.into_iter().enumerate() {
            let t = head - trail_index as f32 * trail_gap;
            if t < 0.0 {
                continue;
            }
            ui.painter()
                .circle_filled(graph_canvas::cubic_point(points, t), radius * scale, color);
        }
    }
}

fn render_gain_orb_node(
    graph: &AudioGraph,
    node: &AudioNode,
    host_audio: &HostAudioSnapshot,
    canvas: Rect,
    ui: &mut egui::Ui,
    state: &mut AudioStudioCanvasState,
    commands: &mut Vec<CanvasCommand>,
) {
    let rect = node_rect(graph, canvas, state, node, host_audio);
    let center = rect.center();
    let zoom = state.canvas.zoom;
    let radius = (rect.width() * 0.5).max(10.0);

    let id = ui.make_persistent_id(("audio_gain_orb", &node.id.0));
    let response = ui.interact(rect, id, Sense::click_and_drag());

    // Selection
    if response.clicked() {
        let extend = ui.input(|input| input.modifiers.shift || input.modifiers.ctrl);
        state.select_node(node.id.clone(), extend);
    }

    // Double-click resets to 1.0x (0.0 dB)
    if response.double_clicked() {
        commands.push(CanvasCommand::SetNodeGain {
            node_id: node.id.clone(),
            gain_db: 0.0,
        });
    }

    // Node drag on canvas
    if response.drag_started() {
        state.begin_node_drag(
            node.id.clone(),
            graph
                .nodes
                .iter()
                .map(|n| (n.id.clone(), [n.position.x, n.position.y])),
        );
    }
    if response.dragged() && state.drag_node.as_ref() == Some(&node.id) {
        state.update_node_drag(response.drag_delta());
    }
    if response.drag_stopped() && state.drag_node.as_ref() == Some(&node.id) {
        commands.extend(
            state
                .finish_node_drag(Some(16.0))
                .into_iter()
                .map(|movement| CanvasCommand::MoveNode {
                    node_id: movement.node_id,
                    position: GraphPosition {
                        x: movement.position[0],
                        y: movement.position[1],
                    },
                }),
        );
    }

    let current_gain_db = gain_node_value(node);
    let mut linear_gain = 10.0_f32.powf(current_gain_db / 20.0);

    // Mouse wheel scroll adjustment:
    let is_hovered = response.hovered();
    let wheel_steps = if is_hovered {
        ui.input(|i| {
            let mut steps = 0.0_f32;
            for event in &i.raw.events {
                if let egui::Event::MouseWheel { delta, .. } = event {
                    if delta.y > 0.0 {
                        steps += 1.0;
                    } else if delta.y < 0.0 {
                        steps -= 1.0;
                    }
                }
            }
            steps
        })
    } else {
        0.0
    };

    let mut changed_by_wheel = false;
    if wheel_steps != 0.0 {
        linear_gain = ((linear_gain + wheel_steps * 0.1) * 10.0).round() / 10.0;
        linear_gain = linear_gain.clamp(0.0, 5.0);
        let new_gain_db = if linear_gain < 0.001 {
            -60.0
        } else {
            20.0 * linear_gain.log10()
        };
        commands.push(CanvasCommand::SetNodeGain {
            node_id: node.id.clone(),
            gain_db: new_gain_db,
        });
        changed_by_wheel = true;
    }

    // Control state tracking:
    let now = ui.input(|i| i.time);
    let last_interact_id = id.with("last_interact");
    let mut last_interact = ui.data(|d| d.get_temp::<f64>(last_interact_id)).unwrap_or(0.0);
    if is_hovered || response.dragged() || changed_by_wheel {
        last_interact = now;
        ui.data_mut(|d| d.insert_temp(last_interact_id, now));
    }
    let is_controlling = is_hovered || (now - last_interact) < 1.2;
    if is_controlling && !is_hovered {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
    }

    let selected = state.selected_nodes.contains(&node.id);
    let base_border = crate::ui::theme::border();
    let border_color = if selected {
        LINK_SELECTED
    } else if is_controlling {
        crate::ui::theme::text_strong()
    } else {
        base_border
    };
    let stroke_width = if selected { 2.2 } else if is_controlling { 1.4 } else { 1.0 } * zoom;

    // 1. Unfilled sphere body remains transparent (per user: "球内没有水的部分保持透明")

    // 2. Liquid fill
    let fill_fraction = if linear_gain <= 1.0 {
        (linear_gain * 0.5).clamp(0.0, 0.5)
    } else {
        (0.5 + 0.125 * (linear_gain - 1.0)).clamp(0.5, 1.0)
    };
    let y_water = rect.bottom() - fill_fraction * (2.0 * radius);
    let dy_from_center = y_water - center.y;

    if fill_fraction > 0.002 {
        let liquid_alpha = if is_controlling { 135 } else { 85 };
        let liquid_color = Color32::from_rgba_unmultiplied(
            border_color.r(),
            border_color.g(),
            border_color.b(),
            liquid_alpha,
        );

        if fill_fraction >= 0.995 || dy_from_center <= -radius {
            ui.painter().circle_filled(center, radius, liquid_color);
        } else if dy_from_center < radius {
            let half_chord = (radius * radius - dy_from_center * dy_from_center).max(0.0).sqrt();
            let left_intersect = Pos2::new(center.x - half_chord, y_water);
            let right_intersect = Pos2::new(center.x + half_chord, y_water);

            let start_angle = (dy_from_center / radius).clamp(-1.0, 1.0).asin();
            let steps = 32;
            let mut liquid_polygon = Vec::with_capacity(steps + 2);
            liquid_polygon.push(left_intersect);
            liquid_polygon.push(right_intersect);
            for i in 1..steps {
                let frac = i as f32 / steps as f32;
                let theta = start_angle + (std::f32::consts::PI - 2.0 * start_angle) * frac;
                let px = center.x + radius * theta.cos();
                let py = center.y + radius * theta.sin();
                liquid_polygon.push(Pos2::new(px, py));
            }
            ui.painter().add(egui::Shape::convex_polygon(
                liquid_polygon,
                liquid_color,
                Stroke::NONE,
            ));

            // Water surface line
            ui.painter().line_segment(
                [left_intersect, right_intersect],
                Stroke::new(1.1 * zoom, border_color),
            );
        }
    }

    // 3. Middle reference line (equator line at 1.0x)
    let mid_color = Color32::from_rgba_unmultiplied(
        border_color.r(),
        border_color.g(),
        border_color.b(),
        if is_controlling { 110 } else { 55 },
    );
    ui.painter().line_segment(
        [
            Pos2::new(center.x - radius * 0.8, center.y),
            Pos2::new(center.x + radius * 0.8, center.y),
        ],
        Stroke::new(0.8 * zoom, mid_color),
    );

    // 4. Fine outer circle outline
    ui.painter().circle_stroke(
        center,
        radius,
        Stroke::new(stroke_width, border_color),
    );

    // 5. Value display floating ABOVE the ball only when controlling (per user: "数值仅在控制时显示在球的上方")
    if is_controlling {
        let value_text = if linear_gain < 0.01 {
            "MUTE".to_string()
        } else if (linear_gain - 1.0).abs() < 0.02 {
            "1.0x (0 dB)".to_string()
        } else {
            format!("{:.1}x ({:+.1} dB)", linear_gain, current_gain_db)
        };

        let label_pos = Pos2::new(center.x, rect.top() - 14.0 * zoom);
        ui.painter().text(
            label_pos,
            Align2::CENTER_BOTTOM,
            value_text,
            FontId::proportional((11.5 * zoom).clamp(8.5, 14.0)),
            crate::ui::theme::text_strong(),
        );
    }

    // 6. Ports (In on left, Out on right)
    for port in input_ports(graph, node) {
        render_port(graph, node, &port, false, rect, ui, state, host_audio, commands);
    }
    for port in output_ports(node) {
        render_port(graph, node, &port, true, rect, ui, state, host_audio, commands);
    }
}

fn render_node(
    graph: &AudioGraph,
    node: &AudioNode,
    host_audio: &HostAudioSnapshot,
    risks: &RouteRiskReport,
    canvas: Rect,
    ui: &mut egui::Ui,
    state: &mut AudioStudioCanvasState,
    commands: &mut Vec<CanvasCommand>,
) {
    if is_gain_node(node) {
        render_gain_orb_node(graph, node, host_audio, canvas, ui, state, commands);
        return;
    }
    let devices = &host_audio.devices;
    let rect = node_rect(graph, canvas, state, node, host_audio);
    let header = Rect::from_min_max(
        rect.min,
        Pos2::new(
            rect.right(),
            rect.top() + NODE_HEADER_HEIGHT * state.canvas.zoom,
        ),
    );
    let response = ui.interact(
        header,
        ui.make_persistent_id(("audio_node", &node.id.0)),
        Sense::click_and_drag(),
    );
    if response.clicked() {
        let extend = ui.input(|input| input.modifiers.shift || input.modifiers.ctrl);
        state.select_node(node.id.clone(), extend);
    }
    if response.drag_started() {
        state.begin_node_drag(
            node.id.clone(),
            graph
                .nodes
                .iter()
                .map(|node| (node.id.clone(), [node.position.x, node.position.y])),
        );
    }
    if response.dragged() && state.drag_node.as_ref() == Some(&node.id) {
        state.update_node_drag(response.drag_delta());
    }
    if response.drag_stopped() && state.drag_node.as_ref() == Some(&node.id) {
        commands.extend(
            state
                .finish_node_drag(Some(16.0))
                .into_iter()
                .map(|movement| CanvasCommand::MoveNode {
                    node_id: movement.node_id,
                    position: GraphPosition {
                        x: movement.position[0],
                        y: movement.position[1],
                    },
                }),
        );
    }

    let selected = state.selected_nodes.contains(&node.id);
    let node_risks = risks
        .risks
        .iter()
        .filter(|risk| risk.node_ids.contains(&node.id))
        .collect::<Vec<_>>();
    let strongest_risk = node_risks
        .iter()
        .map(|risk| risk.severity)
        .min_by_key(|severity| match severity {
            RouteRiskSeverity::Blocking => 0,
            RouteRiskSeverity::Warning => 1,
            RouteRiskSeverity::Info => 2,
        });
    let palette = node_palette(&node.kind);
    let rounding = CornerRadius::same((5.0 * state.canvas.zoom).clamp(2.0, 7.0) as u8);
    ui.painter().rect_filled(rect, rounding, palette.fill);
    ui.painter().rect_stroke(
        rect,
        rounding,
        Stroke::new(
            if selected {
                3.0
            } else if strongest_risk.is_some() {
                2.2
            } else if node.bypassed {
                1.0
            } else {
                1.4
            },
            if selected {
                LINK_SELECTED
            } else if let Some(severity) = strongest_risk {
                match severity {
                    RouteRiskSeverity::Blocking => ERROR,
                    RouteRiskSeverity::Warning => WARNING,
                    RouteRiskSeverity::Info => Color32::from_rgb(55, 105, 160),
                }
            } else if node.bypassed {
                MUTED
            } else {
                palette.accent
            },
        ),
        egui::epaint::StrokeKind::Inside,
    );
    ui.painter().rect_filled(header, rounding, palette.header);
    let scale = state.canvas.zoom;
    ui.painter().text(
        Pos2::new(rect.left() + 12.0 * scale, rect.top() + 7.0 * scale),
        Align2::LEFT_TOP,
        &node.label,
        FontId::proportional((13.0 * scale).clamp(8.0, 16.0)),
        INK,
    );
    ui.painter().text(
        Pos2::new(rect.left() + 12.0 * scale, rect.top() + 28.0 * scale),
        Align2::LEFT_TOP,
        node_kind_label(&node.kind),
        FontId::monospace((8.0 * scale).clamp(6.0, 10.0)),
        palette.accent,
    );
    if !node_risks.is_empty() {
        let badge = Rect::from_center_size(
            Pos2::new(rect.right() - 15.0 * scale, rect.top() + 15.0 * scale),
            Vec2::splat(20.0 * scale),
        );
        let badge_color = match strongest_risk.unwrap_or(RouteRiskSeverity::Info) {
            RouteRiskSeverity::Blocking => ERROR,
            RouteRiskSeverity::Warning => WARNING,
            RouteRiskSeverity::Info => Color32::from_rgb(55, 105, 160),
        };
        ui.painter()
            .circle_filled(badge.center(), 8.0 * scale, badge_color);
        ui.painter().text(
            badge.center(),
            Align2::CENTER_CENTER,
            "!",
            FontId::proportional((11.0 * scale).clamp(8.0, 13.0)),
            Color32::WHITE,
        );
        ui.interact(
            badge,
            ui.make_persistent_id(("audio_node_risks", &node.id.0)),
            Sense::hover(),
        )
        .on_hover_text(
            node_risks
                .iter()
                .map(|risk| risk.summary.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    if !matches!(node.kind, AudioNodeKind::Mixer) {
        ui.painter().text(
            Pos2::new(rect.left() + 13.0 * scale, rect.top() + 91.0 * scale),
            Align2::LEFT_TOP,
            node_description(graph, node),
            FontId::proportional((10.5 * scale).clamp(7.0, 12.5)),
            MUTED,
        );
    }
    if scale < 0.65 {
        if device_role(&node.kind).is_some()
            || matches!(node.kind, AudioNodeKind::SystemAudio { .. })
        {
            let selected =
                node.kind.selected_device().is_some() || node.kind.selected_application().is_some();
            ui.painter().text(
                Pos2::new(rect.left() + 13.0 * scale, rect.bottom() - 24.0 * scale),
                Align2::LEFT_CENTER,
                format!("Device · {}", device_selection_text(&node.kind, devices)),
                FontId::monospace((8.5 * scale).clamp(6.0, 10.5)),
                if selected { INK } else { WARNING },
            );
        }
    }

    for port in input_ports(graph, node) {
        render_port(graph, node, &port, false, rect, ui, state, host_audio, commands);
    }
    for port in output_ports(node) {
        render_port(graph, node, &port, true, rect, ui, state, host_audio, commands);
    }
    render_node_control(graph, node, host_audio, rect, ui, state, commands);
}

fn device_role(kind: &AudioNodeKind) -> Option<AudioDeviceRole> {
    match kind {
        AudioNodeKind::Microphone { .. } => Some(AudioDeviceRole::MicrophoneCapture),
        AudioNodeKind::MonitorOutput { .. } => Some(AudioDeviceRole::MonitorRender),
        AudioNodeKind::GameMicrophoneOutput { .. } => Some(AudioDeviceRole::GameMicrophoneSink),
        _ => None,
    }
}

fn device_selection_text(kind: &AudioNodeKind, devices: &[HostAudioDevice]) -> String {
    if let AudioNodeKind::SystemAudio { capture } = kind {
        return match capture {
            SystemAudioCapture::Endpoint { device_id, .. } => device_id
                .as_ref()
                .and_then(|selected| {
                    devices.iter().find(|device| {
                        device.role == AudioDeviceRole::SystemAudioCapture && &device.id == selected
                    })
                })
                .map(|device| format!("Endpoint · {}", device.name))
                .unwrap_or_else(|| "ENDPOINT · System default".into()),
            SystemAudioCapture::Application { application, .. } => {
                application.as_ref().map_or_else(
                    || "APP · Select an application".into(),
                    |application| format!("App · {}", application.display_name),
                )
            }
        };
    }
    let Some(role) = device_role(kind) else {
        return String::new();
    };
    let candidates = devices
        .iter()
        .filter(|device| device.role == role && !device.id.0.trim().is_empty())
        .collect::<Vec<_>>();
    if let Some(selected) = kind.selected_device()
        && let Some(device) = candidates.iter().find(|device| &device.id == selected)
    {
        return device.name.clone();
    }
    if role == AudioDeviceRole::GameMicrophoneSink {
        return match candidates.as_slice() {
            [device] => format!("Auto · {}", device.name),
            [] => "No virtual mic feed output found".into(),
            _ => "Select virtual mic feed output".into(),
        };
    }
    "System default".into()
}

fn render_node_control(
    graph: &AudioGraph,
    node: &AudioNode,
    host_audio: &HostAudioSnapshot,
    rect: Rect,
    ui: &mut egui::Ui,
    state: &AudioStudioCanvasState,
    commands: &mut Vec<CanvasCommand>,
) {
    if state.canvas.zoom < 0.65 {
        return;
    }
    let scale = state.canvas.zoom;
    let devices = &host_audio.devices;
    let is_asr_node = graph.reaches_asr_sink(&node.id);
    let locked_by = if is_asr_node {
        host_audio.translation_workflow_locked_by.as_deref()
    } else {
        None
    };
    if matches!(node.kind, AudioNodeKind::SystemAudio { .. }) {
        let mut child = ui.new_child(egui::UiBuilder::new());
        child.add_enabled_ui(locked_by.is_none(), |ui| {
            render_system_audio_control(node, host_audio, rect, ui, scale, commands);
        });
    } else if let Some(role) = device_role(&node.kind) {
        let candidates = devices
            .iter()
            .filter(|device| device.role == role && !device.id.0.trim().is_empty())
            .collect::<Vec<_>>();
        let selected = node
            .kind
            .selected_device()
            .cloned()
            .filter(|id| candidates.iter().any(|device| &device.id == id));
        let selected_text = device_selection_text(&node.kind, devices);
        let control_rect = Rect::from_min_max(
            Pos2::new(rect.left() + 10.0 * scale, rect.bottom() - 36.0 * scale),
            Pos2::new(rect.right() - 10.0 * scale, rect.bottom() - 8.0 * scale),
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(control_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        child.add_enabled_ui(locked_by.is_none(), |child| {
            crate::ui::components::combobox_ui_with_width(
                child,
                ("audio_device", &node.id.0),
                &selected_text,
                Some(control_rect.width()),
                |ui| {
                    let automatic_label = if role == AudioDeviceRole::GameMicrophoneSink {
                        match candidates.as_slice() {
                            [device] => format!("Auto · {}", device.name),
                            [] => "No virtual mic feed output found".into(),
                            _ => "Select virtual mic feed output".into(),
                        }
                    } else {
                        "System default".into()
                    };
                    if ui
                        .selectable_label(selected.is_none(), automatic_label)
                        .clicked()
                    {
                        commands.push(CanvasCommand::SetNodeDevice {
                            node_id: node.id.clone(),
                            device_id: None,
                        });
                        if role == AudioDeviceRole::GameMicrophoneSink
                            && !matches!(candidates.as_slice(), [device] if device.voicemeeter_strip_index.is_some())
                        {
                            commands.push(CanvasCommand::SetNodeVoiceMeeterBus {
                                node_id: node.id.clone(),
                                bus: None,
                            });
                        }
                    }
                    for device in candidates {
                        let label = if device.is_default {
                            format!("{} · default", device.name)
                        } else {
                            device.name.clone()
                        };
                        if ui
                            .selectable_label(selected.as_ref() == Some(&device.id), label)
                            .clicked()
                        {
                            commands.push(CanvasCommand::SetNodeDevice {
                                node_id: node.id.clone(),
                                device_id: Some(device.id.clone()),
                            });
                            if role == AudioDeviceRole::GameMicrophoneSink
                                && device.voicemeeter_strip_index.is_none()
                            {
                                commands.push(CanvasCommand::SetNodeVoiceMeeterBus {
                                    node_id: node.id.clone(),
                                    bus: None,
                                });
                            }
                        }
                    }
                });
        });
        if let Some(target) = voicemeeter_target(node, host_audio) {
            render_voicemeeter_bus_control(node, target, rect, ui, scale, commands);
        }
    } else if matches!(node.kind, AudioNodeKind::Media { .. }) {
        let button_rect = Rect::from_min_max(
            Pos2::new(rect.left() + 12.0 * scale, rect.bottom() - 36.0 * scale),
            Pos2::new(rect.right() - 12.0 * scale, rect.bottom() - 8.0 * scale),
        );
        let response = ui.put(button_rect, egui::Button::new("Choose BGM / media…"));
        if response.clicked() {
            commands.push(CanvasCommand::ChooseMedia(node.id.clone()));
        }
    } else if matches!(node.kind, AudioNodeKind::TextToSpeech) {
        let text_id = ui.make_persistent_id(("audio_tts_text", &node.id.0));
        let mut text = ui
            .ctx()
            .data_mut(|data| data.get_temp::<String>(text_id).unwrap_or_default());
        let row_rect = Rect::from_min_max(
            Pos2::new(rect.left() + 10.0 * scale, rect.bottom() - 38.0 * scale),
            Pos2::new(rect.right() - 10.0 * scale, rect.bottom() - 7.0 * scale),
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(row_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        crate::ui::components::text_edit_ui(
            &mut child,
            ("audio_speak_into_route", &node.id.0),
            egui::TextEdit::singleline(&mut text)
                .hint_text("Speak into route…")
                .desired_width((row_rect.width() - 54.0).max(40.0)),
        );
        let send = child
            .add_enabled(!text.trim().is_empty(), egui::Button::new("Send"))
            .clicked();
        if send {
            commands.push(CanvasCommand::EnqueueTts {
                node_id: node.id.clone(),
                text: text.trim().to_owned(),
            });
            text.clear();
        }
        ui.ctx().data_mut(|data| data.insert_temp(text_id, text));
    }
}

fn render_system_audio_control(
    node: &AudioNode,
    host_audio: &HostAudioSnapshot,
    rect: Rect,
    ui: &mut egui::Ui,
    scale: f32,
    commands: &mut Vec<CanvasCommand>,
) {
    let AudioNodeKind::SystemAudio { capture } = &node.kind else {
        return;
    };
    let mode_rect = Rect::from_min_max(
        Pos2::new(rect.left() + 10.0 * scale, rect.bottom() - 76.0 * scale),
        Pos2::new(rect.right() - 10.0 * scale, rect.bottom() - 47.0 * scale),
    );
    let source_rect = Rect::from_min_max(
        Pos2::new(rect.left() + 10.0 * scale, rect.bottom() - 40.0 * scale),
        Pos2::new(rect.right() - 10.0 * scale, rect.bottom() - 9.0 * scale),
    );
    let mut mode_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(mode_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let endpoint_mode = matches!(capture, SystemAudioCapture::Endpoint { .. });
    let selected_mode_text = if endpoint_mode {
        "Entire output device"
    } else {
        "One application"
    };
    crate::ui::components::combobox_ui_with_width(
        &mut mode_ui,
        ("system_audio_mode", &node.id.0),
        selected_mode_text,
        Some(mode_rect.width()),
        |ui| {
            if ui
                .selectable_label(endpoint_mode, "Entire output device")
                .on_hover_text("Captures every application playing on the selected output device")
                .clicked()
                && !endpoint_mode
            {
                commands.push(CanvasCommand::SetSystemAudioCapture {
                    node_id: node.id.clone(),
                    capture: SystemAudioCapture::Endpoint {
                        device_id: None,
                        capture_policy: SystemCapturePolicy::AllEndpointAudio,
                    },
                });
            }
            if ui
                .selectable_label(!endpoint_mode, "One application")
                .on_hover_text("Captures only the selected application and its child processes")
                .clicked()
                && endpoint_mode
            {
                commands.push(CanvasCommand::SetSystemAudioCapture {
                    node_id: node.id.clone(),
                    capture: SystemAudioCapture::Application {
                        application: None,
                        resolved_process_id: None,
                    },
                });
            }
        });

    let mut source_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(source_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    match capture {
        SystemAudioCapture::Endpoint {
            device_id,
            capture_policy,
        } => {
            let candidates = host_audio
                .devices
                .iter()
                .filter(|device| {
                    device.role == AudioDeviceRole::SystemAudioCapture
                        && !device.id.0.trim().is_empty()
                })
                .collect::<Vec<_>>();
            let selected_text = device_id
                .as_ref()
                .and_then(|selected| candidates.iter().find(|device| &device.id == selected))
                .map(|device| device.name.clone())
                .unwrap_or_else(|| "System default".into());
            crate::ui::components::combobox_ui_with_width(
                &mut source_ui,
                ("system_audio_endpoint", &node.id.0),
                selected_text,
                Some(source_rect.width()),
                |ui| {
                    if ui
                        .selectable_label(device_id.is_none(), "System default")
                        .clicked()
                    {
                        commands.push(CanvasCommand::SetSystemAudioCapture {
                            node_id: node.id.clone(),
                            capture: SystemAudioCapture::Endpoint {
                                device_id: None,
                                capture_policy: *capture_policy,
                            },
                        });
                    }
                    for device in candidates {
                        let label = if device.is_default {
                            format!("{} · default", device.name)
                        } else {
                            device.name.clone()
                        };
                        if ui
                            .selectable_label(device_id.as_ref() == Some(&device.id), label)
                            .clicked()
                        {
                            commands.push(CanvasCommand::SetSystemAudioCapture {
                                node_id: node.id.clone(),
                                capture: SystemAudioCapture::Endpoint {
                                    device_id: Some(device.id.clone()),
                                    capture_policy: *capture_policy,
                                },
                            });
                        }
                    }
                });
        }
        SystemAudioCapture::Application { application, .. } => {
            let selected_text = application
                .as_ref()
                .map(|application| application.display_name.clone())
                .unwrap_or_else(|| "Select an application…".into());
            let combo = crate::ui::components::combobox_ui_with_width(
                &mut source_ui,
                ("system_audio_application", &node.id.0),
                selected_text,
                Some(source_rect.width()),
                |ui| {
                    if host_audio.applications.is_empty() {
                        ui.label("No application audio sessions found · refreshed on access");
                    }
                    for candidate in &host_audio.applications {
                        let label = if candidate.active {
                            format!("{} · playing", candidate.display_name)
                        } else {
                            candidate.display_name.clone()
                        };
                        let selected = application
                            .as_ref()
                            .is_some_and(|application| application.id == candidate.id);
                        if ui.selectable_label(selected, label).clicked() {
                            commands.push(CanvasCommand::SetSystemAudioCapture {
                                node_id: node.id.clone(),
                                capture: SystemAudioCapture::Application {
                                    application: Some(ApplicationSelection {
                                        id: candidate.id.clone(),
                                        display_name: candidate.display_name.clone(),
                                    }),
                                    resolved_process_id: None,
                                },
                            });
                        }
                    }
                    if let Some(selection) = application
                        && !host_audio
                            .applications
                            .iter()
                            .any(|candidate| candidate.id == selection.id)
                    {
                        ui.separator();
                        ui.label(format!("{} · not running", selection.display_name));
                    }
                });
            if combo.response.clicked() {
                commands.push(CanvasCommand::DiscoverApplications);
            }
        }
    }
}

fn render_voicemeeter_bus_control(
    node: &AudioNode,
    target: VoiceMeeterTarget<'_>,
    rect: Rect,
    ui: &mut egui::Ui,
    scale: f32,
    commands: &mut Vec<CanvasCommand>,
) {
    let selected_bus = selected_voicemeeter_bus(node);
    let target_rect = Rect::from_min_max(
        Pos2::new(rect.left() + 11.0 * scale, rect.top() + 116.0 * scale),
        Pos2::new(rect.right() - 11.0 * scale, rect.top() + 146.0 * scale),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(target_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.label(
        RichText::new("App mic")
            .font(FontId::monospace((8.5 * scale).clamp(7.0, 10.0)))
            .color(MUTED)
            .strong(),
    );
    for bus in [VoiceMeeterBus::B1, VoiceMeeterBus::B2, VoiceMeeterBus::B3] {
        if !target.snapshot.supports_bus(bus) {
            continue;
        }
        if child
            .selectable_label(selected_bus == bus, bus.label())
            .on_hover_text(format!(
                "XRTranslate mix → {}",
                paired_recording_device(bus)
            ))
            .clicked()
        {
            commands.push(CanvasCommand::SetNodeVoiceMeeterBus {
                node_id: node.id.clone(),
                bus: Some(bus),
            });
        }
    }
    ui.painter().text(
        Pos2::new(rect.left() + 13.0 * scale, rect.top() + 157.0 * scale),
        Align2::LEFT_TOP,
        format!(
            "Choose in the target app: {}",
            paired_recording_device(selected_bus)
        ),
        FontId::proportional((9.5 * scale).clamp(7.0, 11.0)),
        if target.snapshot.running {
            INK
        } else {
            WARNING
        },
    );
    let input_name = target
        .device
        .voicemeeter_strip_index
        .and_then(|strip| {
            target
                .snapshot
                .inputs
                .iter()
                .find(|input| input.strip_index == strip)
                .map(|input| input.name.as_str())
        })
        .unwrap_or(target.device.name.as_str());
    ui.interact(
        Rect::from_min_max(
            Pos2::new(rect.left() + 10.0 * scale, rect.top() + 151.0 * scale),
            Pos2::new(rect.right() - 10.0 * scale, rect.top() + 180.0 * scale),
        ),
        ui.make_persistent_id(("voicemeeter_pairing", &node.id.0)),
        Sense::hover(),
    )
    .on_hover_text(format!("XRTranslate output: {input_name}"));
}

fn render_port(
    graph: &AudioGraph,
    node: &AudioNode,
    port: &PortId,
    output: bool,
    node_rect: Rect,
    ui: &mut egui::Ui,
    state: &mut AudioStudioCanvasState,
    host_audio: &HostAudioSnapshot,
    commands: &mut Vec<CanvasCommand>,
) {
    let center = port_position(graph, node, node_rect, port, output, state.canvas.zoom);
    let radius = (PORT_RADIUS * state.canvas.zoom).clamp(4.0, 8.0);
    let endpoint = GraphEndpoint {
        node_id: node.id.clone(),
        port_id: port.clone(),
    };
    let response = ui
        .interact(
            Rect::from_center_size(
                center,
                Vec2::splat(if !output { 20.0 } else { radius * 4.0 }),
            ),
            ui.make_persistent_id(("audio_port", &node.id.0, &port.0, output)),
            Sense::click_and_drag(),
        )
        .on_hover_text(if output {
            "Drag audio output to an input"
        } else {
            "Drag to connect; drag an existing input to rewire it"
        });
    let commit = if output {
        state.interact_output_port(&response, endpoint.clone())
    } else {
        let connected = graph
            .links
            .iter()
            .find(|link| link.to == endpoint)
            .map(|link| (link.from.clone(), link.id.clone()));
        state.interact_input_port(&response, endpoint.clone(), connected)
    };
    if let Some(commit) = commit {
        commands.push(CanvasCommand::CommitWire(commit));
    }

    let connected_link = (!output)
        .then(|| graph.links.iter().find(|link| link.to == endpoint))
        .flatten();
    let color = if connected_link.is_some_and(|link| !link.enabled) {
        LINK_INACTIVE
    } else {
        port_color(port, output)
    };
    ui.painter().circle_filled(center, radius, color);
    ui.painter()
        .circle_stroke(center, radius + 1.0, Stroke::new(1.0, Color32::WHITE));
    if is_gain_node(node) {
        return;
    }
    let shows_input_toggle = !output && connected_link.is_some() && state.canvas.zoom >= 0.65;
    let label_position = if output {
        Pos2::new(center.x - 11.0 * state.canvas.zoom, center.y)
    } else if shows_input_toggle {
        Pos2::new(center.x + 54.0, center.y)
    } else {
        Pos2::new(center.x + 11.0 * state.canvas.zoom, center.y)
    };
    ui.painter().text(
        label_position,
        if output {
            Align2::RIGHT_CENTER
        } else {
            Align2::LEFT_CENTER
        },
        if matches!(node.kind, AudioNodeKind::Mixer) && !output {
            connected_link
                .and_then(|link| graph.node(&link.from.node_id))
                .map(|source| source.label.as_str())
                .unwrap_or("Connect another input")
        } else if state.canvas.zoom < 0.65 {
            if port.0 == PortId::SIDECHAIN {
                "SC"
            } else if output {
                "Out"
            } else {
                "IN"
            }
        } else {
            port_label(port, output)
        },
        FontId::monospace((8.0 * state.canvas.zoom).clamp(6.0, 10.0)),
        color,
    );

    if shows_input_toggle && let Some(link) = connected_link {
        let toggle_rect = Rect::from_min_size(
            Pos2::new(center.x + 11.0, center.y - 10.0),
            Vec2::new(36.0, 20.0),
        );
        let mut enabled = link.enabled;
        let mut toggle_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(toggle_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let is_asr_link = graph.is_asr_pipeline_link(link);
        let locked_by = if is_asr_link {
            host_audio.translation_workflow_locked_by.as_deref()
        } else {
            None
        };
        let toggle = toggle_ui
            .add_enabled_ui(locked_by.is_none(), |ui| {
                ui.push_id(("audio_link_toggle", &link.id.0), |ui| {
                    crate::ui::components::pill_toggle(ui, &mut enabled)
                })
                .inner
            })
            .inner;
        let toggle = if let Some(owner) = locked_by {
            toggle.on_hover_text(format!(
                "{owner} currently owns this translation session; Audio Studio cannot modify this input route"
            ))
        } else if link.enabled {
            toggle.on_hover_text("This input is on · click to exclude this path")
        } else {
            toggle.on_hover_text("This input is off · click to include this path")
        };
        if toggle.changed() {
            commands.push(CanvasCommand::SetLinkEnabled {
                link_id: link.id.clone(),
                enabled,
            });
        }
    }
}

fn render_wire_preview(
    ui: &egui::Ui,
    state: &AudioStudioCanvasState,
    positions: &HashMap<GraphEndpoint, Pos2>,
) {
    let Some(pointer) = ui
        .ctx()
        .pointer_hover_pos()
        .or_else(|| ui.ctx().pointer_latest_pos())
    else {
        return;
    };
    if let Some(from) = state
        .wire_from
        .as_ref()
        .and_then(|from| positions.get(from))
    {
        graph_canvas::paint_wire(
            ui,
            graph_canvas::bezier_points(*from, pointer),
            Stroke::new(2.5, OUTPUT_PORT),
        );
    } else if let Some(to) = state
        .wire_from_input
        .as_ref()
        .and_then(|to| positions.get(to))
    {
        graph_canvas::paint_wire(
            ui,
            graph_canvas::bezier_points(pointer, *to),
            Stroke::new(2.5, OUTPUT_PORT),
        );
    } else {
        return;
    }
    ui.painter().circle_filled(pointer, 4.0, OUTPUT_PORT);
}

fn finish_wire_drag(
    graph: &AudioGraph,
    ui: &egui::Ui,
    state: &mut AudioStudioCanvasState,
    positions: &HashMap<GraphEndpoint, Pos2>,
    commands: &mut Vec<CanvasCommand>,
) {
    if !state.wire_active() || !ui.input(|input| input.pointer.any_released()) {
        return;
    }
    let pointer = ui.ctx().pointer_latest_pos();
    if let Some(from) = state.wire_from.clone() {
        let input_target = pointer.and_then(|pointer| {
            crate::ui::graph_editor::nearest_port(
                pointer,
                positions.iter().filter_map(|(endpoint, position)| {
                    (endpoint.node_id != from.node_id
                        && graph
                            .node(&endpoint.node_id)
                            .is_some_and(|node| node.kind.accepts_input(&endpoint.port_id)))
                    .then(|| (endpoint.clone(), *position))
                }),
                16.0,
            )
        });
        if let Some(commit) = state.finish_wire(input_target) {
            commands.push(CanvasCommand::CommitWire(commit));
        }
        return;
    }
    let input_node = state
        .wire_from_input
        .as_ref()
        .map(|endpoint| endpoint.node_id.clone());
    let output_target = pointer.and_then(|pointer| {
        crate::ui::graph_editor::nearest_port(
            pointer,
            positions.iter().filter_map(|(endpoint, position)| {
                (input_node.as_ref() != Some(&endpoint.node_id)
                    && graph
                        .node(&endpoint.node_id)
                        .is_some_and(|node| node.kind.provides_output(&endpoint.port_id)))
                .then(|| (endpoint.clone(), *position))
            }),
            16.0,
        )
    });
    if let Some(from) = output_target
        && let Some(commit) = state.finish_reverse_wire(from)
    {
        commands.push(CanvasCommand::CommitWire(commit));
    } else {
        state.cancel_wire();
    }
}

pub(crate) fn render(
    snapshot: &AudioStudioUiSnapshot,
    ui: &mut egui::Ui,
) -> Vec<AudioStudioUiAction> {
    ui.scope(|ui| {
        graph_style::apply(ui);
        render_scoped(snapshot, ui)
    })
    .inner
}

fn render_scoped(snapshot: &AudioStudioUiSnapshot, ui: &mut egui::Ui) -> Vec<AudioStudioUiAction> {
    let id = state_id(ui);
    let mut state = ui.ctx().data_mut(|data| {
        data.get_temp::<AudioStudioCanvasState>(id)
            .unwrap_or_default()
    });
    let mut actions = Vec::new();

    render_header(snapshot, ui, &mut state, &mut actions);
    ui.add_space(7.0);
    render_status(snapshot, ui);
    ui.add_space(7.0);

    let mut commands = Vec::new();
    render_graph_canvas(snapshot, ui, &mut state, &mut commands, &mut actions);
    for command in commands {
        if matches!(
            command,
            CanvasCommand::MoveNode { .. }
                | CanvasCommand::CommitWire(_)
                | CanvasCommand::DeleteLink(_)
                | CanvasCommand::RemoveNode(_)
                | CanvasCommand::SetNodeDevice { .. }
                | CanvasCommand::SetSystemAudioCapture { .. }
                | CanvasCommand::SetLinkEnabled { .. }
                | CanvasCommand::SetNodeVoiceMeeterBus { .. }
                | CanvasCommand::SetNodeGain { .. }
        ) {
            state.history.push(snapshot.selected_graph.clone());
        }
        let action = match command {
            CanvasCommand::DiscoverApplications => AudioStudioUiAction::DiscoverApplications,
            CanvasCommand::ReplaceGraph(graph) => AudioStudioUiAction::ReplaceSelectedGraph(graph),
            CanvasCommand::MoveNode { node_id, position } => {
                AudioStudioUiAction::MoveNode { node_id, position }
            }
            CanvasCommand::CommitWire(commit) => match (commit.replaced, commit.to) {
                (Some(link_id), Some(to)) => AudioStudioUiAction::Rewire {
                    link_id,
                    from: commit.from,
                    to,
                },
                (Some(link_id), None) => AudioStudioUiAction::DeleteLink(link_id),
                (None, Some(to)) => AudioStudioUiAction::Connect {
                    from: commit.from,
                    to,
                },
                (None, None) => continue,
            },
            CanvasCommand::DeleteLink(link_id) => AudioStudioUiAction::DeleteLink(link_id),
            CanvasCommand::RemoveNode(node_id) => AudioStudioUiAction::RemoveNode(node_id),
            CanvasCommand::SetNodeDevice { node_id, device_id } => {
                AudioStudioUiAction::SetNodeDevice { node_id, device_id }
            }
            CanvasCommand::SetSystemAudioCapture { node_id, capture } => {
                AudioStudioUiAction::SetSystemAudioCapture { node_id, capture }
            }
            CanvasCommand::SetLinkEnabled { link_id, enabled } => {
                AudioStudioUiAction::SetLinkEnabled { link_id, enabled }
            }
            CanvasCommand::SetNodeVoiceMeeterBus { node_id, bus } => {
                AudioStudioUiAction::SetNodeVoiceMeeterBus { node_id, bus }
            }
            CanvasCommand::SetNodeGain { node_id, gain_db } => {
                AudioStudioUiAction::SetNodeGain { node_id, gain_db }
            }
            CanvasCommand::ChooseMedia(node_id) => AudioStudioUiAction::ChooseMedia(node_id),
            CanvasCommand::EnqueueTts { node_id, text } => {
                AudioStudioUiAction::EnqueueTts { node_id, text }
            }
        };
        actions.push(action);
    }
    ui.ctx().data_mut(|data| data.insert_temp(id, state));
    actions
}

fn render_header(
    snapshot: &AudioStudioUiSnapshot,
    ui: &mut egui::Ui,
    state: &mut AudioStudioCanvasState,
    actions: &mut Vec<AudioStudioUiAction>,
) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new("Audio Studio").size(21.0).strong().color(INK));
            ui.label(
                RichText::new(
                    "Connect audio sources, processing and outputs without hidden feedback paths",
                )
                .size(11.5)
                .color(MUTED),
            );
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            route_lifecycle_chip(ui, &snapshot.lifecycle);
            workflow_status_chip(ui, snapshot.host_audio.translation_workflow_running);
        });
    });
    ui.add_space(8.0);

    crate::ui::layout::flow_row(ui, |ui| {
        ui.label(mono_label("System graph /"));
        ui.label(
            RichText::new("One global audio route")
                .font(FontId::monospace(10.0))
                .color(SUCCESS),
        )
        .on_hover_text(
            "This canvas is the one audio topology used by XRTranslate. There are no competing graph pages.",
        );
        ui.menu_button("Load preset", |ui| {
            ui.label(
                RichText::new("Replaces the current graph")
                    .font(FontId::monospace(9.5))
                    .color(WARNING),
            );
            for preset in AudioStudioPreset::ALL {
                if ui.button(preset.display_name()).clicked() {
                    state.pending_preset_load = Some(preset);
                    state.pending_safe_reset = false;
                    ui.close();
                }
            }
        });
        ui.menu_button("+ Node", |ui| {
            render_add_node_menu(snapshot, state, ui, actions);
        });
        ui.separator();
        if small_button(ui, "Auto layout", true).clicked() {
            let arranged = auto_layout_graph(&snapshot.selected_graph, &snapshot.host_audio);
            if arranged != snapshot.selected_graph {
                state.history.push(snapshot.selected_graph.clone());
                actions.push(AudioStudioUiAction::ReplaceSelectedGraph(arranged));
                state.canvas.fit_pending = true;
            }
        }
        if small_button(ui, "Fit", true).clicked() {
            state.canvas.fit_pending = true;
        }
        if small_button(ui, "−", true).clicked() {
            state.canvas.zoom = (state.canvas.zoom - 0.1).clamp(0.25, 1.6);
        }
        if small_button(ui, "+", true).clicked() {
            state.canvas.zoom = (state.canvas.zoom + 0.1).clamp(0.25, 1.6);
        }
        let can_undo = state.history.can_undo();
        if small_button(ui, "Undo", can_undo).clicked()
            && let Some(previous) = state.history.undo(snapshot.selected_graph.clone())
        {
            actions.push(AudioStudioUiAction::ReplaceSelectedGraph(previous));
        }
        let can_redo = state.history.can_redo();
        if small_button(ui, "Redo", can_redo).clicked()
            && let Some(next) = state.history.redo(snapshot.selected_graph.clone())
        {
            actions.push(AudioStudioUiAction::ReplaceSelectedGraph(next));
        }
        let has_selection = !state.selected_nodes.is_empty() || !state.selected_links.is_empty();
        if small_button(ui, "Delete selection", has_selection)
            .on_hover_text("Delete only the selected nodes or links from this route.")
            .clicked()
        {
            state.history.push(snapshot.selected_graph.clone());
            let (nodes, links) = state.take_selection();
            actions.extend(nodes.into_iter().map(AudioStudioUiAction::RemoveNode));
            actions.extend(links.into_iter().map(AudioStudioUiAction::DeleteLink));
        }
    });
    ui.add_space(4.0);

    crate::ui::layout::flow_row(ui, |ui| {
        ui.label(mono_label("Current /"));
        ui.label(
            RichText::new(snapshot.selected_graph.name.as_str())
                .font(FontId::monospace(10.5))
                .color(INK),
        );
        if snapshot.dirty {
            status_text(ui, "Unsaved changes", WARNING);
        } else {
            status_text(ui, "Saved", SUCCESS);
        }
        if small_button(ui, "Save", snapshot.dirty).clicked() {
            actions.push(AudioStudioUiAction::Save);
        }
        if small_button(ui, "Reset audio system", true)
            .on_hover_text(
                "Replace the whole graph with the complete default audio system after confirmation.",
            )
            .clicked()
        {
            state.pending_preset_load = None;
            state.pending_safe_reset = true;
        }
    });

    if let Some(preset) = state.pending_preset_load {
        ui.add_space(4.0);
        crate::ui::layout::flow_row(ui, |ui| {
            status_text(
                ui,
                &format!("Replace the global graph with ‘{}’?", preset.display_name()),
                WARNING,
            );
            if small_button(ui, "Replace graph", true)
                .on_hover_text(
                    "Replace every current node and connection. Complete enabled output paths update automatically.",
                )
                .clicked()
            {
                actions.push(AudioStudioUiAction::LoadPreset(preset));
                state.history = GraphEditHistory::default();
                state.clear_selection();
                state.canvas.fit_pending = true;
                state.pending_preset_load = None;
            }
            if small_button(ui, "Cancel", true).clicked() {
                state.pending_preset_load = None;
            }
        });
    } else if state.pending_safe_reset {
        ui.add_space(4.0);
        crate::ui::layout::flow_row(ui, |ui| {
            status_text(ui, "Reset to the complete default audio system?", WARNING);
            if small_button(ui, "Reset to complete default", true).clicked() {
                actions.push(AudioStudioUiAction::ResetToDefault);
                state.history = GraphEditHistory::default();
                state.clear_selection();
                state.canvas.fit_pending = true;
                state.pending_safe_reset = false;
            }
            if small_button(ui, "Cancel", true).clicked() {
                state.pending_safe_reset = false;
            }
        });
    }
}

fn asr_path_node_ids(graph: &AudioGraph) -> Option<HashSet<NodeId>> {
    let sinks = graph
        .nodes
        .iter()
        .filter(|node| !node.bypassed && matches!(node.kind, AudioNodeKind::AsrTap))
        .map(|node| node.id.clone())
        .collect::<Vec<_>>();
    let [sink] = sinks.as_slice() else {
        return None;
    };
    let mut path = HashSet::new();
    let mut pending = vec![sink.clone()];
    while let Some(node_id) = pending.pop() {
        if !path.insert(node_id.clone()) {
            continue;
        }
        pending.extend(
            graph
                .links
                .iter()
                .filter(|link| link.to.node_id == node_id)
                .filter_map(|link| {
                    graph
                        .node(&link.from.node_id)
                        .filter(|node| !node.bypassed)
                        .map(|_| link.from.node_id.clone())
                }),
        );
    }
    Some(path)
}

fn asr_path_is_ready(snapshot: &AudioStudioUiSnapshot) -> bool {
    let Some(path) = asr_path_node_ids(&snapshot.resolved_graph) else {
        return false;
    };
    !snapshot.validation.issues.iter().any(|issue| {
        if let Some(node_id) = &issue.node_id {
            return path.contains(node_id);
        }
        if let Some(link_id) = &issue.link_id {
            return snapshot.resolved_graph.links.iter().any(|link| {
                &link.id == link_id
                    && (path.contains(&link.from.node_id) || path.contains(&link.to.node_id))
            });
        }
        true
    })
}

fn render_status(snapshot: &AudioStudioUiSnapshot, ui: &mut egui::Ui) {
    Frame::new()
        .fill(Color32::from_rgb(249, 250, 248))
        .stroke(Stroke::new(1.0, CANVAS_BORDER))
        .corner_radius(CornerRadius::same(3))
        .inner_margin(Margin::symmetric(11, 7))
        .show(ui, |ui| {
            crate::ui::layout::flow_row(ui, |ui| {
                if snapshot.validation.is_valid() {
                    status_text(ui, "✓ Audio system ready", SUCCESS);
                } else {
                    let issue_count = snapshot.validation.issues.len();
                    status_text(
                        ui,
                        &if issue_count == 1 {
                            "⚠ 1 setup item needs attention".to_owned()
                        } else {
                            format!("⚠ {issue_count} setup items need attention")
                        },
                        ERROR,
                    );
                }
                ui.separator();
                if snapshot.host_audio.discovery_complete {
                    let device_count = snapshot.host_audio.devices.len();
                    let app_count = snapshot.host_audio.applications.len();
                    status_text(
                        ui,
                        &format!(
                            "{device_count} {} · {app_count} {} · Lists refresh when opened",
                            if device_count == 1 {
                                "device"
                            } else {
                                "devices"
                            },
                            if app_count == 1 { "app" } else { "apps" },
                        ),
                        INK,
                    );
                } else {
                    status_text(ui, "Discovering audio devices…", WARNING);
                    ui.spinner();
                }
                ui.separator();
                render_feedback_status(snapshot, ui);
                ui.separator();
                render_game_microphone_status(snapshot, ui);
                if let Some(voicemeeter) = snapshot.host_audio.voicemeeter.as_ref() {
                    ui.separator();
                    render_voicemeeter_status(snapshot, voicemeeter, ui);
                }
            });

            ui.add_space(5.0);
            crate::ui::layout::flow_row(ui, |ui| {
                let has_asr = snapshot
                    .selected_graph
                    .nodes
                    .iter()
                    .any(|node| !node.bypassed && matches!(node.kind, AudioNodeKind::AsrTap));
                let asr_path_enabled = snapshot.selected_graph.nodes.iter().any(|node| {
                    !node.bypassed
                        && matches!(node.kind, AudioNodeKind::AsrTap)
                        && snapshot.selected_graph.has_enabled_source_path(&node.id)
                });
                let asr_mode = current_asr_input_mode(&snapshot.resolved_graph);
                if has_asr && !asr_path_enabled {
                    status_text(ui, "ASR input · Off", MUTED);
                } else if let Some(input_mode) = asr_mode
                    && asr_path_is_ready(snapshot)
                {
                    status_text(
                        ui,
                        &format!("ASR input · {} · Ready", input_mode.label()),
                        SUCCESS,
                    );
                } else if has_asr {
                    status_text(ui, "ASR input · Selected path needs attention", WARNING);
                } else {
                    status_text(ui, "ASR input · Not used by this graph", MUTED);
                }

                let has_game_microphone = snapshot.selected_graph.nodes.iter().any(|node| {
                    !node.bypassed
                        && matches!(node.kind, AudioNodeKind::GameMicrophoneOutput { .. })
                });
                let has_monitor = snapshot.selected_graph.nodes.iter().any(|node| {
                    !node.bypassed && matches!(node.kind, AudioNodeKind::MonitorOutput { .. })
                });
                if has_game_microphone || has_monitor {
                    ui.separator();
                    let selected_is_live =
                        matches!(snapshot.lifecycle, AudioStudioLifecycle::Active { .. });
                    let destination = match (has_game_microphone, has_monitor) {
                        (true, true) => "Monitor + app microphone",
                        (true, false) => "App microphone",
                        (false, true) => "Monitor output",
                        (false, false) => unreachable!(),
                    };
                    status_text(
                        ui,
                        &format!(
                            "Output paths · {destination} · {}",
                            if selected_is_live && snapshot.live_routing_matches_graph {
                                "Automatic · Running"
                            } else if selected_is_live {
                                "Automatic · Updating"
                            } else {
                                "Waiting for an enabled path"
                            }
                        ),
                        if selected_is_live && snapshot.live_routing_matches_graph {
                            SUCCESS
                        } else {
                            WARNING
                        },
                    );
                    if !selected_is_live {
                        ui.label(
                            RichText::new(
                                "Connect and switch on a complete path to start it automatically. Moving dots show paths that are actually carrying audio.",
                            )
                            .size(10.0)
                            .color(MUTED),
                        );
                    }
                }
            });

            if let Some(error) =
                snapshot
                    .last_error
                    .as_deref()
                    .or_else(|| match &snapshot.lifecycle {
                        AudioStudioLifecycle::Error { message, .. } => Some(message.as_str()),
                        _ => None,
                    })
            {
                ui.add_space(4.0);
                ui.label(RichText::new(error).size(11.0).color(ERROR));
            }
            for issue in snapshot.validation.issues.iter().take(3) {
                ui.add_space(2.0);
                let target = issue
                    .node_id
                    .as_ref()
                    .map(|node| format!(" [{}]", node.0))
                    .or_else(|| issue.link_id.as_ref().map(|link| format!(" [{}]", link.0)))
                    .unwrap_or_default();
                ui.label(
                    RichText::new(format!("• {}{target}", issue.message))
                        .size(10.5)
                        .color(ERROR),
                );
            }
            if !snapshot.risk_report.risks.is_empty() {
                ui.add_space(3.0);
                egui::CollapsingHeader::new(format!(
                    "Route risk details · {}",
                    snapshot.risk_report.risks.len()
                ))
                .default_open(snapshot.risk_report.blocking_count() > 0)
                .show(ui, |ui| {
                    for risk in &snapshot.risk_report.risks {
                        let color = match risk.severity {
                            RouteRiskSeverity::Blocking => ERROR,
                            RouteRiskSeverity::Warning => WARNING,
                            RouteRiskSeverity::Info => Color32::from_rgb(55, 105, 160),
                        };
                        let level = match risk.severity {
                            RouteRiskSeverity::Blocking => "Blocking",
                            RouteRiskSeverity::Warning => "Warning",
                            RouteRiskSeverity::Info => "Info",
                        };
                        ui.label(
                            RichText::new(format!("{level} · {}", risk.summary))
                                .size(10.5)
                                .strong()
                                .color(color),
                        );
                        ui.label(RichText::new(&risk.detail).size(10.0).color(INK));
                        ui.label(
                            RichText::new(format!("Fix: {}", risk.remediation))
                                .size(10.0)
                                .color(MUTED),
                        );
                        ui.label(
                            RichText::new(format!(
                                "Path: {}",
                                risk.node_ids
                                    .iter()
                                    .map(|node| node.0.as_str())
                                    .collect::<Vec<_>>()
                                    .join(" → ")
                            ))
                            .font(FontId::monospace(9.0))
                            .color(color),
                        );
                        ui.add_space(3.0);
                    }
                });
            }
            if let Some(voicemeeter) = snapshot.host_audio.voicemeeter.as_ref() {
                render_voicemeeter_advanced(voicemeeter, ui);
            }
            if snapshot
                .selected_graph
                .nodes
                .iter()
                .any(|node| matches!(node.kind, AudioNodeKind::AsrTap))
            {
                ui.add_space(3.0);
                let message = if snapshot.host_audio.translation_workflow_running {
                    "ASR input changes apply the next time the Translation workflow starts."
                } else {
                    "ASR input is configured here; start recognition from the Translation page."
                };
                ui.label(RichText::new(message).size(10.0).color(MUTED));
            }
        });
}

fn render_add_node_menu(
    snapshot: &AudioStudioUiSnapshot,
    state: &mut AudioStudioCanvasState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AudioStudioUiAction>,
) {
    ui.label(mono_label("Sources"));
    add_node_button(
        ui,
        snapshot,
        state,
        actions,
        "Microphone",
        "microphone",
        AudioNodeKind::Microphone { device_id: None },
    );
    add_node_button(
        ui,
        snapshot,
        state,
        actions,
        "System audio (TTS-safe)",
        "system-audio",
        AudioNodeKind::SystemAudio {
            capture: SystemAudioCapture::Endpoint {
                device_id: None,
                capture_policy: SystemCapturePolicy::SuppressDuringOwnTts,
            },
        },
    );
    add_node_button(
        ui,
        snapshot,
        state,
        actions,
        "Text to speech",
        "tts",
        AudioNodeKind::TextToSpeech,
    );
    add_node_button(
        ui,
        snapshot,
        state,
        actions,
        "Media / BGM",
        "media",
        AudioNodeKind::Media {
            source: None,
            loop_playback: true,
        },
    );
    ui.separator();
    ui.label(mono_label("Routing / DSP"));
    add_node_button(
        ui,
        snapshot,
        state,
        actions,
        "Mixer",
        "mixer",
        AudioNodeKind::Mixer,
    );
    for (label, slug, processor) in [
        ("Gain", "gain", AudioProcessor::Gain { gain_db: 0.0 }),
        (
            "Noise gate",
            "noise-gate",
            AudioProcessor::NoiseGate {
                threshold_db: -45.0,
            },
        ),
        (
            "Compressor",
            "compressor",
            AudioProcessor::Compressor {
                threshold_db: -18.0,
                ratio: 3.0,
            },
        ),
        (
            "Limiter",
            "limiter",
            AudioProcessor::Limiter { ceiling_db: -1.0 },
        ),
        (
            "Ducker (sidechain)",
            "ducker",
            AudioProcessor::Ducker {
                attenuation_db: -14.0,
            },
        ),
    ] {
        add_node_button(
            ui,
            snapshot,
            state,
            actions,
            label,
            slug,
            AudioNodeKind::Processing { processor },
        );
    }
    ui.separator();
    ui.label(mono_label("Outputs"));
    add_node_button(
        ui,
        snapshot,
        state,
        actions,
        "ASR input",
        "asr",
        AudioNodeKind::AsrTap,
    );
    add_node_button(
        ui,
        snapshot,
        state,
        actions,
        "Monitor / headphones",
        "monitor",
        AudioNodeKind::MonitorOutput { device_id: None },
    );
    add_node_button(
        ui,
        snapshot,
        state,
        actions,
        "App microphone output",
        "game-microphone",
        AudioNodeKind::GameMicrophoneOutput {
            device_id: None,
            voicemeeter_bus: None,
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn add_node_button(
    ui: &mut egui::Ui,
    snapshot: &AudioStudioUiSnapshot,
    state: &mut AudioStudioCanvasState,
    actions: &mut Vec<AudioStudioUiAction>,
    label: &str,
    slug: &str,
    kind: AudioNodeKind,
) {
    if !ui.button(label).clicked() {
        return;
    }
    let id = unique_node_id(&snapshot.selected_graph, slug);
    let mut node = AudioNode::new(id, label, kind);
    let preferred = state.add_node_center.take();
    let position = state.new_node_position(
        snapshot.selected_graph.nodes.iter().map(|node| {
            Rect::from_min_size(Pos2::new(node.position.x, node.position.y), node_size(node))
        }),
        node_size(&node),
        preferred,
        16.0,
    );
    node.position = GraphPosition {
        x: position[0],
        y: position[1],
    };
    state.history.push(snapshot.selected_graph.clone());
    state.select_node(node.id.clone(), false);
    actions.push(AudioStudioUiAction::AddNode(node));
    state.canvas.fit_pending = true;
    ui.close();
}

fn unique_node_id(graph: &AudioGraph, slug: &str) -> String {
    if !graph.nodes.iter().any(|node| node.id.0 == slug) {
        return slug.to_owned();
    }
    (2..)
        .map(|suffix| format!("{slug}-{suffix}"))
        .find(|candidate| graph.nodes.iter().all(|node| node.id.0 != *candidate))
        .expect("an unbounded numeric suffix always has an unused value")
}

fn render_feedback_status(snapshot: &AudioStudioUiSnapshot, ui: &mut egui::Ui) {
    let blocking = snapshot.risk_report.blocking_count();
    let warnings = snapshot.risk_report.warning_count();
    if blocking > 0 {
        status_text(
            ui,
            &format!(
                "{blocking} blocking feedback {}",
                if blocking == 1 { "risk" } else { "risks" }
            ),
            ERROR,
        );
    } else if warnings > 0 {
        status_text(
            ui,
            &format!(
                "{warnings} potential route {}",
                if warnings == 1 { "risk" } else { "risks" }
            ),
            WARNING,
        );
    } else {
        status_text(ui, "No blocking or warning risks detected", SUCCESS);
    }
}

fn render_game_microphone_status(snapshot: &AudioStudioUiSnapshot, ui: &mut egui::Ui) {
    let game_microphone = snapshot
        .selected_graph
        .nodes
        .iter()
        .find(|node| matches!(node.kind, AudioNodeKind::GameMicrophoneOutput { .. }));
    let Some(game_microphone) = game_microphone else {
        status_text(ui, "No app microphone output", MUTED);
        return;
    };
    if snapshot
        .host_audio
        .capabilities
        .game_microphone_without_external_driver
    {
        status_text(ui, "App microphone output · Built in", SUCCESS);
        return;
    }
    let candidates = snapshot
        .host_audio
        .devices
        .iter()
        .filter(|device| {
            device.role == AudioDeviceRole::GameMicrophoneSink && !device.id.0.trim().is_empty()
        })
        .collect::<Vec<_>>();
    if let Some(selected) = game_microphone.kind.selected_device()
        && let Some(device) = candidates.iter().find(|device| &device.id == selected)
    {
        status_text(ui, &format!("Virtual mic · {}", device.name), SUCCESS);
        return;
    }
    match candidates.as_slice() {
        [device] => status_text(
            ui,
            &format!("Virtual mic · Automatic · {}", device.name),
            SUCCESS,
        ),
        [] => {
            status_text(ui, "Virtual microphone required", ERROR);
            ui.label(
                RichText::new(
                    "Install or select a virtual input so another app can receive this mixed route",
                )
                .size(10.0)
                .color(ERROR),
            );
        }
        devices => status_text(
            ui,
            &format!(
                "Select a virtual mic feed · {} {}",
                devices.len(),
                if devices.len() == 1 {
                    "output"
                } else {
                    "outputs"
                }
            ),
            WARNING,
        ),
    }
}

fn render_voicemeeter_status(
    snapshot: &AudioStudioUiSnapshot,
    voicemeeter: &VoiceMeeterSnapshot,
    ui: &mut egui::Ui,
) {
    let route_uses_voicemeeter = snapshot.selected_graph.nodes.iter().any(|node| {
        voicemeeter_target(node, &snapshot.host_audio).is_some()
            || node.kind.selected_device().is_some_and(|selected| {
                snapshot
                    .host_audio
                    .devices
                    .iter()
                    .any(|device| &device.id == selected && device.requires_voicemeeter())
            })
    });
    if voicemeeter.running {
        status_text(ui, "VoiceMeeter · Running", SUCCESS);
    } else if route_uses_voicemeeter {
        status_text(ui, "VoiceMeeter · Will start with the route", WARNING);
    } else {
        status_text(ui, "VoiceMeeter · Installed · Not needed", MUTED);
    }
}

fn render_voicemeeter_advanced(voicemeeter: &VoiceMeeterSnapshot, ui: &mut egui::Ui) {
    ui.add_space(3.0);
    egui::CollapsingHeader::new(
        RichText::new("Advanced VoiceMeeter details")
            .size(10.0)
            .color(MUTED),
    )
    .default_open(false)
    .show(ui, |ui| {
        let edition = match voicemeeter.edition {
            VoiceMeeterEdition::Standard => "Standard",
            VoiceMeeterEdition::Banana => "Banana",
            VoiceMeeterEdition::Potato => "Potato",
        };
        let version = voicemeeter
            .version
            .as_deref()
            .unwrap_or("version unavailable");
        ui.label(
            RichText::new(format!("{edition} · {version}"))
                .font(FontId::monospace(9.5))
                .color(MUTED),
        );
        let buses = [VoiceMeeterBus::B1, VoiceMeeterBus::B2, VoiceMeeterBus::B3]
            .into_iter()
            .filter(|bus| voicemeeter.supports_bus(*bus))
            .map(|bus| bus.label())
            .collect::<Vec<_>>()
            .join(" / ");
        ui.label(
            RichText::new(format!(
                "{} XRTranslate {} · microphone targets {buses}",
                voicemeeter.inputs.len(),
                if voicemeeter.inputs.len() == 1 {
                    "input"
                } else {
                    "inputs"
                }
            ))
            .font(FontId::monospace(9.5))
            .color(MUTED),
        );
    });
}

fn route_lifecycle_chip(ui: &mut egui::Ui, lifecycle: &AudioStudioLifecycle) {
    let (label, color) = match lifecycle {
        AudioStudioLifecycle::Inactive => ("Output paths idle", MUTED),
        AudioStudioLifecycle::Activating { .. } => ("Applying output paths…", WARNING),
        AudioStudioLifecycle::Active { .. } => ("● Output paths active", SUCCESS),
        AudioStudioLifecycle::Deactivating { .. } => ("Stopping output paths…", WARNING),
        AudioStudioLifecycle::Error { .. } => ("Output path error", ERROR),
    };
    Frame::new()
        .fill(Color32::from_rgba_unmultiplied(
            color.r(),
            color.g(),
            color.b(),
            18,
        ))
        .stroke(Stroke::new(1.0, color))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(9, 4))
        .show(ui, |ui| {
            ui.label(
                RichText::new(label)
                    .font(FontId::monospace(10.0))
                    .color(color)
                    .strong(),
            );
        });
}

fn workflow_status_chip(ui: &mut egui::Ui, running: bool) {
    let (label, color) = if running {
        ("● Translation running", SUCCESS)
    } else {
        ("Translation stopped", MUTED)
    };
    Frame::new()
        .stroke(Stroke::new(1.0, color))
        .corner_radius(CornerRadius::same(3))
        .inner_margin(Margin::symmetric(9, 5))
        .show(ui, |ui| {
            ui.label(
                RichText::new(label)
                    .font(FontId::monospace(9.5))
                    .color(color),
            )
            .on_hover_text("Read-only status; control translation from the Translation page.");
        });
}

fn status_text(ui: &mut egui::Ui, text: &str, color: Color32) {
    ui.label(
        RichText::new(text)
            .font(FontId::monospace(9.5))
            .color(color)
            .strong(),
    );
}

fn mono_label(text: &str) -> RichText {
    RichText::new(text)
        .font(FontId::monospace(9.5))
        .color(MUTED)
        .strong()
}

fn small_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(
            RichText::new(label)
                .font(FontId::monospace(9.5))
                .color(if enabled { INK } else { MUTED }),
        )
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::new(1.0, CANVAS_BORDER))
        .corner_radius(CornerRadius::same(2))
        .min_size(Vec2::new(34.0, 25.0)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_studio::presets::graph_for_preset;

    fn host_device(id: &str, name: &str, role: AudioDeviceRole) -> HostAudioDevice {
        HostAudioDevice {
            id: DeviceId(id.to_owned()),
            name: name.to_owned(),
            role,
            is_default: false,
            voicemeeter_strip_index: None,
        }
    }

    #[test]
    fn game_microphone_empty_selection_names_only_a_unique_sink_candidate() {
        let kind = AudioNodeKind::GameMicrophoneOutput {
            device_id: None,
            voicemeeter_bus: None,
        };
        let sink = host_device(
            "virtual-1",
            "Voicemeeter Output",
            AudioDeviceRole::GameMicrophoneSink,
        );
        let unrelated = host_device("render-1", "Speakers", AudioDeviceRole::MonitorRender);
        assert_eq!(
            device_selection_text(&kind, &[sink.clone(), unrelated]),
            "Auto · Voicemeeter Output"
        );

        let second_sink = host_device(
            "virtual-2",
            "Virtual Cable",
            AudioDeviceRole::GameMicrophoneSink,
        );
        assert_eq!(
            device_selection_text(&kind, &[sink, second_sink]),
            "Select virtual mic feed output"
        );
    }

    #[test]
    fn audio_auto_layout_preserves_node_clearance_and_link_direction() {
        let graph = graph_for_preset(AudioStudioPreset::TtsToGameMicrophone);
        let arranged = auto_layout_graph(&graph, &HostAudioSnapshot::default());
        for (index, node) in arranged.nodes.iter().enumerate() {
            let rect =
                Rect::from_min_size(Pos2::new(node.position.x, node.position.y), node_size(node));
            for other in arranged.nodes.iter().skip(index + 1) {
                let other_rect = Rect::from_min_size(
                    Pos2::new(other.position.x, other.position.y),
                    node_size(other),
                );
                assert!(!rect.intersects(other_rect));
            }
        }
        for link in &arranged.links {
            let from = arranged
                .nodes
                .iter()
                .find(|node| node.id == link.from.node_id)
                .unwrap();
            let to = arranged
                .nodes
                .iter()
                .find(|node| node.id == link.to.node_id)
                .unwrap();
            assert!(to.position.x >= from.position.x + node_size(from).x + 150.0);
        }
    }

    #[test]
    fn preset_templates_start_with_visible_clearance_between_nodes() {
        for preset in AudioStudioPreset::ALL {
            let graph = graph_for_preset(preset);
            for (index, node) in graph.nodes.iter().enumerate() {
                let rect = Rect::from_min_size(
                    Pos2::new(node.position.x, node.position.y),
                    node_size(node),
                )
                .expand(20.0);
                for other in graph.nodes.iter().skip(index + 1) {
                    let other_rect = Rect::from_min_size(
                        Pos2::new(other.position.x, other.position.y),
                        node_size(other),
                    )
                    .expand(20.0);
                    assert!(
                        !rect.intersects(other_rect),
                        "{preset:?}: {} overlaps {}",
                        node.id.0,
                        other.id.0
                    );
                }
            }
        }
    }

    #[test]
    fn voicemeeter_controls_require_capability_and_strip_metadata() {
        use crate::audio_studio::{VoiceMeeterEdition, VoiceMeeterSnapshot, VoiceMeeterStripIndex};

        let mut node = AudioNode::new(
            "game-microphone",
            "Game microphone",
            AudioNodeKind::GameMicrophoneOutput {
                device_id: Some(DeviceId::new("vm-feed")),
                voicemeeter_bus: None,
            },
        );
        let mut device = host_device(
            "vm-feed",
            "VoiceMeeter Input",
            AudioDeviceRole::GameMicrophoneSink,
        );
        device.voicemeeter_strip_index = Some(VoiceMeeterStripIndex(4));
        let mut host = HostAudioSnapshot {
            devices: vec![device],
            ..HostAudioSnapshot::default()
        };
        assert!(voicemeeter_target(&node, &host).is_none());
        assert_eq!(node_size_for_host(&node, &host), node_size(&node));

        host.voicemeeter = Some(VoiceMeeterSnapshot {
            edition: VoiceMeeterEdition::Banana,
            running: false,
            version: None,
            inputs: Vec::new(),
            buses: vec![VoiceMeeterBus::B1, VoiceMeeterBus::B2],
        });
        assert_eq!(selected_voicemeeter_bus(&node), VoiceMeeterBus::B1);
        assert!(voicemeeter_target(&node, &host).is_some());
        assert_eq!(
            node_size_for_host(&node, &host).y,
            node_size(&node).y + VOICEMEETER_GAME_MIC_EXTRA_HEIGHT
        );

        if let AudioNodeKind::GameMicrophoneOutput {
            voicemeeter_bus, ..
        } = &mut node.kind
        {
            *voicemeeter_bus = Some(VoiceMeeterBus::B2);
        }
        assert_eq!(selected_voicemeeter_bus(&node), VoiceMeeterBus::B2);
        assert_eq!(
            paired_recording_device(VoiceMeeterBus::B2),
            "Voicemeeter AUX Out B2"
        );
    }
}
