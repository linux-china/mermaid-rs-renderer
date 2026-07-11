use std::collections::BTreeMap;

use crate::config::LayoutConfig;
use crate::ir::{DiagramKind, Direction, Graph};
use crate::theme::Theme;

use super::super::{
    DiagramData, EdgeLayout, LAYOUT_BOUNDARY_PAD, Layout, NodeLayout, STATE_NOTE_GAP_MIN,
    STATE_NOTE_GAP_SCALE, STATE_NOTE_PAD_X_SCALE, STATE_NOTE_PAD_Y_SCALE, StateNoteLayout,
    SubgraphLayout, apply_direction_mirror, bounds_with_edges, measure_label, normalize_layout,
};

pub(in crate::layout) fn finalize_graph_layout(
    graph: &Graph,
    mut nodes: BTreeMap<String, NodeLayout>,
    mut edges: Vec<EdgeLayout>,
    mut subgraphs: Vec<SubgraphLayout>,
    theme: &Theme,
    config: &LayoutConfig,
) -> Layout {
    if matches!(graph.direction, Direction::RightLeft | Direction::BottomTop) {
        apply_direction_mirror(graph.direction, &mut nodes, &mut edges, &mut subgraphs);
    }

    normalize_layout(&mut nodes, &mut edges, &mut subgraphs);
    let mut state_notes = Vec::new();
    if graph.kind == DiagramKind::State && !graph.state_notes.is_empty() {
        let note_pad_x = theme.font_size * STATE_NOTE_PAD_X_SCALE;
        let note_pad_y = theme.font_size * STATE_NOTE_PAD_Y_SCALE;
        let note_gap = (theme.font_size * STATE_NOTE_GAP_SCALE).max(STATE_NOTE_GAP_MIN);
        for note in &graph.state_notes {
            // Composite state targets keep only a hidden anchor node inside
            // their subgraph, so anchor the note to the subgraph bounds.
            let Some((tx, ty, tw, th)) =
                state_note_target_rect(&note.target, graph, &nodes, &subgraphs)
            else {
                continue;
            };
            let label = measure_label(&note.label, theme, config);
            let width = label.width + note_pad_x * 2.0;
            let height = label.height + note_pad_y * 2.0;
            let y = ty + th / 2.0 - height / 2.0;
            let x = match note.position {
                crate::ir::StateNotePosition::LeftOf => tx - note_gap - width,
                crate::ir::StateNotePosition::RightOf => tx + tw + note_gap,
            };
            state_notes.push(StateNoteLayout {
                x,
                y,
                width,
                height,
                label,
                position: note.position,
                target: note.target.clone(),
            });
        }
    }
    let (mut max_x, mut max_y) = bounds_with_edges(&nodes, &subgraphs, &edges);
    for note in &state_notes {
        max_x = max_x.max(note.x + note.width);
        max_y = max_y.max(note.y + note.height);
    }
    let width = max_x + LAYOUT_BOUNDARY_PAD;
    let height = max_y + LAYOUT_BOUNDARY_PAD;

    Layout {
        kind: graph.kind,
        nodes,
        edges,
        subgraphs,
        width,
        height,
        diagram: DiagramData::Graph { state_notes },
    }
}

/// Resolves the rectangle a state note should attach to.
///
/// Simple states resolve to their node layout. Composite states are laid out
/// as subgraphs whose graph node is a hidden anchor, so notes targeting them
/// attach to the subgraph bounds instead.
fn state_note_target_rect(
    target: &str,
    graph: &Graph,
    nodes: &BTreeMap<String, NodeLayout>,
    subgraphs: &[SubgraphLayout],
) -> Option<(f32, f32, f32, f32)> {
    let node = nodes.get(target);
    if let Some(node) = node
        && !node.hidden
    {
        return Some((node.x, node.y, node.width, node.height));
    }
    let sub = graph.subgraphs.iter().find(|sub| {
        sub.id.as_deref() == Some(target) || (!sub.label.is_empty() && sub.label == target)
    });
    if let Some(sub) = sub
        && let Some(layout) = subgraphs
            .iter()
            .find(|layout| layout.label == sub.label && layout.nodes == sub.nodes)
    {
        return Some((layout.x, layout.y, layout.width, layout.height));
    }
    node.map(|node| (node.x, node.y, node.width, node.height))
}
