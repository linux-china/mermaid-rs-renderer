use std::collections::BTreeMap;
use std::f32::consts::PI;

use crate::config::LayoutConfig;
use crate::ir::Graph;
use crate::theme::Theme;

use super::text::{measure_label, measure_label_with_font_size};
use super::{DiagramData, Layout, build_node_layout, resolve_node_style};

// Single source of truth for radar geometry. `render_radar` in src/render.rs
// draws with these same constants so layout bounds and rendered geometry can
// never disagree.
pub(crate) const MAX_RADIUS: f32 = 300.0;
pub(crate) const GRID_STEPS: usize = 5;
pub(crate) const AXIS_LABEL_OFFSET: f32 = 15.0;
pub(crate) const AXIS_LABEL_NUDGE: f32 = 6.0;
pub(crate) const AXIS_LABEL_FONT_SIZE: f32 = 12.0;
pub(crate) const LEGEND_BOX_SIZE: f32 = 12.0;
pub(crate) const LEGEND_GAP: f32 = 4.0;
pub(crate) const LEGEND_OFFSET_FACTOR: f32 = 0.8;
/// Minimum half-extent of the canvas around the chart center. Keeps the
/// historical 700x700 canvas when labels are short.
pub(crate) const MIN_HALF_EXTENT: f32 = MAX_RADIUS + 50.0;
/// Clearance between outermost label geometry and the canvas edge.
const CANVAS_MARGIN: f32 = 10.0;

pub(crate) fn legend_row_height(font_size: f32) -> f32 {
    font_size + 6.0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AxisAnchor {
    Start,
    Middle,
    End,
}

impl AxisAnchor {
    pub(crate) fn as_svg(self) -> &'static str {
        match self {
            AxisAnchor::Start => "start",
            AxisAnchor::Middle => "middle",
            AxisAnchor::End => "end",
        }
    }
}

/// Angle of axis `idx` out of `axis_count`, starting at 12 o'clock and going
/// clockwise (SVG y grows downward).
pub(crate) fn axis_angle(idx: usize, axis_count: usize) -> f32 {
    -PI / 2.0 + 2.0 * PI * idx as f32 / axis_count.max(1) as f32
}

/// Position and anchoring of an axis label, relative to the chart center.
/// Labels on the right anchor at their start and labels on the left anchor at
/// their end so the text always extends outward, away from the grid circle.
pub(crate) fn axis_label_position(angle: f32) -> (f32, f32, AxisAnchor) {
    let label_r = MAX_RADIUS + AXIS_LABEL_OFFSET;
    let mut lx = label_r * angle.cos();
    let ly = label_r * angle.sin();
    let anchor = if angle.cos() > 0.35 {
        lx += AXIS_LABEL_NUDGE;
        AxisAnchor::Start
    } else if angle.cos() < -0.35 {
        lx -= AXIS_LABEL_NUDGE;
        AxisAnchor::End
    } else {
        AxisAnchor::Middle
    };
    (lx, ly, anchor)
}

/// Horizontal bbox of an axis label of width `w` placed by
/// [`axis_label_position`]. Returns (x0, x1) relative to the chart center.
pub(crate) fn axis_label_x_extent(lx: f32, anchor: AxisAnchor, w: f32) -> (f32, f32) {
    match anchor {
        AxisAnchor::Start => (lx, lx + w),
        AxisAnchor::End => (lx - w, lx),
        AxisAnchor::Middle => (lx - w / 2.0, lx + w / 2.0),
    }
}

/// Measured width of an axis label at the radar axis font size. Shared by the
/// layout bounds computation and geometry tests.
pub(crate) fn axis_label_width(axis: &str, theme: &Theme, config: &LayoutConfig) -> f32 {
    measure_label_with_font_size(
        axis,
        AXIS_LABEL_FONT_SIZE,
        config,
        false,
        theme.font_family.as_str(),
    )
    .width
}

/// Axis names shared by the layout and the renderer: the first curve node
/// whose label carries parseable "axis: value" lines defines the axis order.
fn extract_axes(labels: &[&str]) -> Vec<String> {
    for label in labels {
        let mut lines = label
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty());
        let Some(_name) = lines.next() else {
            continue;
        };
        let mut axes = Vec::new();
        for line in lines {
            let Some((axis_raw, value_raw)) = line.split_once(':') else {
                continue;
            };
            let axis = axis_raw.trim();
            let value = value_raw.trim();
            if axis.is_empty() || value.is_empty() || value.parse::<f32>().is_err() {
                continue;
            }
            axes.push(axis.to_string());
        }
        if !axes.is_empty() {
            return axes;
        }
    }
    Vec::new()
}

pub(super) fn compute_radar_layout(graph: &Graph, theme: &Theme, config: &LayoutConfig) -> Layout {
    let legend_offset = MAX_RADIUS * LEGEND_OFFSET_FACTOR;
    let row_height = legend_row_height(theme.font_size);

    let mut node_ids: Vec<String> = graph.nodes.keys().cloned().collect();
    node_ids.sort_by(|a, b| {
        let order_a = graph.node_order.get(a).copied().unwrap_or(usize::MAX);
        let order_b = graph.node_order.get(b).copied().unwrap_or(usize::MAX);
        order_a.cmp(&order_b).then_with(|| a.cmp(b))
    });

    // Canvas half-extents around the chart center, grown by measured axis
    // label and legend geometry so outward-anchored labels never clip.
    let mut left = MIN_HALF_EXTENT;
    let mut right = MIN_HALF_EXTENT;
    let mut top = MIN_HALF_EXTENT;
    let mut bottom = MIN_HALF_EXTENT;

    let ordered_labels: Vec<&str> = node_ids
        .iter()
        .filter_map(|id| graph.nodes.get(id).map(|node| node.label.as_str()))
        .collect();
    let axes = extract_axes(&ordered_labels);
    for (idx, axis) in axes.iter().enumerate() {
        let angle = axis_angle(idx, axes.len());
        let (lx, ly, anchor) = axis_label_position(angle);
        let w = axis_label_width(axis, theme, config);
        let (x0, x1) = axis_label_x_extent(lx, anchor, w);
        let half_h = AXIS_LABEL_FONT_SIZE / 2.0;
        left = left.max(-x0 + CANVAS_MARGIN);
        right = right.max(x1 + CANVAS_MARGIN);
        top = top.max(-(ly - half_h) + CANVAS_MARGIN);
        bottom = bottom.max(ly + half_h + CANVAS_MARGIN);
    }

    struct LegendEntry {
        id: String,
        nl: crate::layout::NodeLayout,
    }
    let mut legend_entries = Vec::new();
    for (idx, node_id) in node_ids.iter().enumerate() {
        let Some(node) = graph.nodes.get(node_id) else {
            continue;
        };
        let label = measure_label(&node.label, theme, config);
        let width = LEGEND_BOX_SIZE + LEGEND_GAP + label.width;
        let height = label.height.max(LEGEND_BOX_SIZE);
        let mut style = resolve_node_style(node.id.as_str(), graph);
        if style.stroke.is_none() {
            style.stroke = Some("none".to_string());
        }
        if style.stroke_width.is_none() {
            style.stroke_width = Some(0.0);
        }
        let nl = build_node_layout(node, label, width, height, style, graph);
        right = right.max(legend_offset + nl.width + CANVAS_MARGIN);
        bottom = bottom.max(-legend_offset + idx as f32 * row_height + nl.height + CANVAS_MARGIN);
        legend_entries.push(LegendEntry {
            id: node.id.clone(),
            nl,
        });
    }

    let center_x = left;
    let center_y = top;
    let width = left + right;
    let height = top + bottom;

    let mut nodes = BTreeMap::new();
    for (idx, entry) in legend_entries.into_iter().enumerate() {
        let mut nl = entry.nl;
        nl.x = center_x + legend_offset;
        nl.y = center_y - legend_offset + idx as f32 * row_height;
        nodes.insert(entry.id, nl);
    }

    Layout {
        kind: graph.kind,
        nodes,
        edges: Vec::new(),
        subgraphs: Vec::new(),
        width,
        height,
        diagram: DiagramData::Radar(super::RadarLayout {
            title: graph.radar_title.clone(),
            center_x,
            center_y,
        }),
    }
}
