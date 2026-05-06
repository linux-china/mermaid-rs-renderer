use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use crate::config::LayoutConfig;
use crate::ir::{DiagramKind, Graph};

use super::super::geometry::{endpoint_side_for_point, side_points_outward};
use super::super::label_placement;
use super::super::routing::*;
use super::super::{
    EDGE_OCCUPANCY_CELL_RATIO, EdgeLayout, FLOWCHART_EDGE_LABEL_WRAP_TRIGGER_CHARS,
    LayoutStageMetrics, MIN_NODE_SPACING_FLOOR, MULTI_EDGE_OFFSET_RATIO, NodeLayout,
    SubgraphLayout, TextBlock, anchor_layout_for_edge,
};
use super::path_cleanup;
use super::plan;
use super::post_route;
use super::roles;
use super::route_labels;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum PortTrack {
    Side(EdgeSide),
    Axis(PortAxis),
}

fn is_branching_port_shape(node: &NodeLayout) -> bool {
    matches!(
        node.shape,
        crate::ir::NodeShape::Diamond
            | crate::ir::NodeShape::Hexagon
            | crate::ir::NodeShape::Parallelogram
            | crate::ir::NodeShape::ParallelogramAlt
            | crate::ir::NodeShape::Trapezoid
            | crate::ir::NodeShape::TrapezoidAlt
            | crate::ir::NodeShape::Asymmetric
    )
}

fn opposite_side(side: EdgeSide) -> EdgeSide {
    match side {
        EdgeSide::Left => EdgeSide::Right,
        EdgeSide::Right => EdgeSide::Left,
        EdgeSide::Top => EdgeSide::Bottom,
        EdgeSide::Bottom => EdgeSide::Top,
    }
}

fn use_axis_wide_port_track(
    node: &NodeLayout,
    side: EdgeSide,
    degree: usize,
    side_counts: [usize; 4],
) -> bool {
    if degree <= 2 {
        return false;
    }
    let axis_total = side_counts[side_slot(side)] + side_counts[side_slot(opposite_side(side))];
    if axis_total <= 2 {
        return false;
    }
    let side_count = side_counts[side_slot(side)];
    let opposite_count = side_counts[side_slot(opposite_side(side))];
    side_count > 1 || opposite_count > 1 || (is_branching_port_shape(node) && axis_total >= 3)
}

fn port_track_for_assignment(
    node: &NodeLayout,
    side: EdgeSide,
    degree: usize,
    side_counts: [usize; 4],
) -> PortTrack {
    if use_axis_wide_port_track(node, side, degree, side_counts) {
        PortTrack::Axis(port_axis(side))
    } else {
        PortTrack::Side(side)
    }
}

fn port_track_node_len(node: &NodeLayout, track: PortTrack) -> f32 {
    match track {
        PortTrack::Side(side) => {
            if side_is_vertical(side) {
                node.height
            } else {
                node.width
            }
        }
        PortTrack::Axis(PortAxis::X) => node.width,
        PortTrack::Axis(PortAxis::Y) => node.height,
    }
}

fn port_track_node_start(node: &NodeLayout, track: PortTrack) -> f32 {
    match track {
        PortTrack::Side(side) => {
            if side_is_vertical(side) {
                node.y
            } else {
                node.x
            }
        }
        PortTrack::Axis(PortAxis::X) => node.x,
        PortTrack::Axis(PortAxis::Y) => node.y,
    }
}

#[derive(Debug, Clone, Copy)]
struct NodeBounds {
    min_x: f32,
    max_x: f32,
    min_y: f32,
    max_y: f32,
}

fn visible_node_bounds(nodes: &BTreeMap<String, NodeLayout>) -> Option<NodeBounds> {
    let mut bounds = NodeBounds {
        min_x: f32::MAX,
        max_x: f32::MIN,
        min_y: f32::MAX,
        max_y: f32::MIN,
    };
    let mut any = false;
    for node in nodes.values() {
        if node.hidden || node.anchor_subgraph.is_some() {
            continue;
        }
        any = true;
        bounds.min_x = bounds.min_x.min(node.x);
        bounds.max_x = bounds.max_x.max(node.x + node.width);
        bounds.min_y = bounds.min_y.min(node.y);
        bounds.max_y = bounds.max_y.max(node.y + node.height);
    }
    any.then_some(bounds)
}

fn enforce_flowchart_endpoint_ports(
    graph: &Graph,
    nodes: &BTreeMap<String, NodeLayout>,
    edge_ports: &[EdgePortInfo],
    routed_points: &mut [Vec<(f32, f32)>],
    config: &LayoutConfig,
) {
    let stub_len = (routing_cell_size(config) * 0.6)
        .max(6.0)
        .min(config.node_spacing.max(MIN_NODE_SPACING_FLOOR) * 0.35);
    for (idx, edge) in graph.edges.iter().enumerate() {
        let Some(points) = routed_points.get_mut(idx) else {
            continue;
        };
        if points.len() < 2 {
            continue;
        }
        if edge.from == edge.to {
            if let Some(node) = nodes.get(&edge.from) {
                let start_side = endpoint_side_for_point(node, points[0]);
                if !side_points_outward(start_side, points[0], points[1]) {
                    let stub = port_stub_point(points[0], start_side, stub_len);
                    if (stub.0 - points[1].0).abs() > 0.5 || (stub.1 - points[1].1).abs() > 0.5 {
                        points.insert(1, stub);
                    }
                }
                let len = points.len();
                if len >= 2 {
                    let end = points[len - 1];
                    let prev = points[len - 2];
                    let end_side = endpoint_side_for_point(node, end);
                    if !side_points_outward(end_side, end, prev) {
                        let stub = port_stub_point(end, end_side, stub_len);
                        if (stub.0 - prev.0).abs() > 0.5 || (stub.1 - prev.1).abs() > 0.5 {
                            points.insert(len - 1, stub);
                        }
                    }
                }
                *points = compress_path(points);
            }
            continue;
        }
        let Some(port) = edge_ports.get(idx).copied() else {
            continue;
        };
        if nodes.get(&edge.from).is_some()
            && !side_points_outward(port.start_side, points[0], points[1])
        {
            let stub = port_stub_point(points[0], port.start_side, stub_len);
            if (stub.0 - points[1].0).abs() > 0.5 || (stub.1 - points[1].1).abs() > 0.5 {
                points.insert(1, stub);
            }
        }
        let len = points.len();
        if len >= 2
            && nodes.get(&edge.to).is_some()
            && !side_points_outward(port.end_side, points[len - 1], points[len - 2])
        {
            let stub = port_stub_point(points[len - 1], port.end_side, stub_len);
            if (stub.0 - points[len - 2].0).abs() > 0.5 || (stub.1 - points[len - 2].1).abs() > 0.5
            {
                points.insert(len - 1, stub);
            }
        }
        *points = compress_path(points);
    }
}

fn collect_other_flowchart_segments(
    routed_points: &[Vec<(f32, f32)>],
    excluded_idx: usize,
) -> Vec<Segment> {
    let mut segments = Vec::new();
    for (idx, points) in routed_points.iter().enumerate() {
        if idx == excluded_idx {
            continue;
        }
        for segment in points.windows(2) {
            segments.push((segment[0], segment[1]));
        }
    }
    segments
}

fn endpoint_repair_score(
    points: &[(f32, f32)],
    edge: &crate::ir::Edge,
    nodes: &BTreeMap<String, NodeLayout>,
    other_segments: &[Segment],
) -> Option<(usize, usize, usize, f32, usize, f32)> {
    let hard = path_cleanup::flowchart_endpoint_direction_violation_count(points, edge, nodes);
    if hard > 0 {
        return None;
    }
    if path_cleanup::flowchart_path_hits_non_endpoint_nodes(points, &edge.from, &edge.to, nodes) {
        return None;
    }
    let debt = path_cleanup::flowchart_endpoint_reentry_count(points, edge, nodes);
    let (crossings, overlap) = edge_crossings_with_existing(points, other_segments);
    let bends = path_bend_count(points);
    let len = path_length(points);
    Some((hard, debt, crossings, overlap, bends, len))
}

fn score_is_better(
    candidate: (usize, usize, usize, f32, usize, f32),
    best: (usize, usize, usize, f32, usize, f32),
) -> bool {
    if candidate.0 != best.0 {
        return candidate.0 < best.0;
    }
    if candidate.1 != best.1 {
        return candidate.1 < best.1;
    }
    if candidate.2 != best.2 {
        return candidate.2 < best.2;
    }
    if (candidate.3 - best.3).abs() > 0.05 {
        return candidate.3 < best.3;
    }
    if candidate.4 != best.4 {
        return candidate.4 < best.4;
    }
    candidate.5 + 1.0 < best.5
}

fn repair_flowchart_endpoint_reentries_by_rerouting(
    graph: &Graph,
    nodes: &BTreeMap<String, NodeLayout>,
    subgraphs: &[SubgraphLayout],
    obstacles: &[Obstacle],
    label_obstacles: &[Obstacle],
    routing_grid: Option<&RoutingGrid>,
    edge_ports: &mut [EdgePortInfo],
    lane_offsets: &[f32],
    routed_points: &mut [Vec<(f32, f32)>],
    config: &LayoutConfig,
) {
    const SIDES: [EdgeSide; 4] = [
        EdgeSide::Left,
        EdgeSide::Right,
        EdgeSide::Top,
        EdgeSide::Bottom,
    ];
    for idx in 0..routed_points.len() {
        let Some(edge) = graph.edges.get(idx) else {
            continue;
        };
        if edge.from == edge.to || routed_points[idx].len() < 2 {
            continue;
        }
        let other_segments = collect_other_flowchart_segments(routed_points, idx);
        let Some(baseline_score) =
            endpoint_repair_score(&routed_points[idx], edge, nodes, &other_segments)
        else {
            continue;
        };
        if baseline_score.1 == 0 {
            continue;
        }

        let (Some(from_layout), Some(to_layout)) = (nodes.get(&edge.from), nodes.get(&edge.to))
        else {
            continue;
        };
        let temp_from = from_layout.anchor_subgraph.and_then(|anchor_idx| {
            subgraphs
                .get(anchor_idx)
                .map(|sub| anchor_layout_for_edge(from_layout, sub, graph.direction, true))
        });
        let temp_to = to_layout.anchor_subgraph.and_then(|anchor_idx| {
            subgraphs
                .get(anchor_idx)
                .map(|sub| anchor_layout_for_edge(to_layout, sub, graph.direction, false))
        });
        let from = temp_from.as_ref().unwrap_or(from_layout);
        let to = temp_to.as_ref().unwrap_or(to_layout);
        let current_port = edge_ports.get(idx).copied().unwrap_or(EdgePortInfo {
            start_side: EdgeSide::Right,
            end_side: EdgeSide::Left,
            start_offset: 0.0,
            end_offset: 0.0,
        });
        let stub_len = port_stub_length(config, from, to);
        let mut best_score = baseline_score;
        let mut best_points: Option<Vec<(f32, f32)>> = None;
        let mut best_port = current_port;

        for start_side in SIDES {
            for end_side in SIDES {
                let candidate_port = EdgePortInfo {
                    start_side,
                    end_side,
                    start_offset: if start_side == current_port.start_side {
                        current_port.start_offset
                    } else {
                        0.0
                    },
                    end_offset: if end_side == current_port.end_side {
                        current_port.end_offset
                    } else {
                        0.0
                    },
                };
                let route_ctx = RouteContext {
                    from_id: &edge.from,
                    to_id: &edge.to,
                    from,
                    to,
                    direction: graph.direction,
                    config,
                    obstacles,
                    label_obstacles,
                    fast_route: false,
                    base_offset: lane_offsets.get(idx).copied().unwrap_or_default(),
                    start_side: candidate_port.start_side,
                    end_side: candidate_port.end_side,
                    start_offset: candidate_port.start_offset,
                    end_offset: candidate_port.end_offset,
                    stub_len,
                    start_inset: if edge.arrow_start {
                        crate::edge_geometry::arrowhead_inset(graph.kind, edge.arrow_start_kind)
                    } else {
                        0.0
                    },
                    end_inset: if edge.arrow_end {
                        crate::edge_geometry::arrowhead_inset(graph.kind, edge.arrow_end_kind)
                    } else {
                        0.0
                    },
                    prefer_shorter_ties: true,
                    preferred_label_id: None,
                    preferred_label_center: None,
                    preferred_label_obstacle: None,
                    preferred_label_clearance: 0.0,
                    force_preferred_label_via: false,
                    coarse_grid_retry: true,
                };
                let candidate = route_edge_with_avoidance(
                    &route_ctx,
                    None,
                    routing_grid,
                    Some(other_segments.as_slice()),
                );
                let Some(score) = endpoint_repair_score(&candidate, edge, nodes, &other_segments)
                else {
                    continue;
                };
                if score.1 >= baseline_score.1 {
                    continue;
                }
                if score.5 > baseline_score.5 * 4.0 + config.node_spacing * 4.0 {
                    continue;
                }
                if score_is_better(score, best_score) {
                    best_score = score;
                    best_points = Some(candidate);
                    best_port = candidate_port;
                }
            }
        }

        if let Some(points) = best_points {
            routed_points[idx] = points;
            if let Some(port) = edge_ports.get_mut(idx) {
                *port = best_port;
            }
        }
    }
}

fn choose_outer_back_edge_sides(
    from: &NodeLayout,
    to: &NodeLayout,
    direction: crate::ir::Direction,
    bounds: Option<NodeBounds>,
    fallback: (EdgeSide, EdgeSide, bool),
) -> (EdgeSide, EdgeSide, bool) {
    let Some(bounds) = bounds else {
        return fallback;
    };

    if is_horizontal(direction) {
        let upper_clearance = from.y.min(to.y) - bounds.min_y;
        let lower_clearance = bounds.max_y - (from.y + from.height).max(to.y + to.height);
        let side = if (upper_clearance - lower_clearance).abs() <= 1.0 {
            let avg_y = (from.y + from.height * 0.5 + to.y + to.height * 0.5) * 0.5;
            if avg_y <= (bounds.min_y + bounds.max_y) * 0.5 {
                EdgeSide::Top
            } else {
                EdgeSide::Bottom
            }
        } else if upper_clearance <= lower_clearance {
            EdgeSide::Top
        } else {
            EdgeSide::Bottom
        };
        (side, side, fallback.2)
    } else {
        let left_clearance = from.x.min(to.x) - bounds.min_x;
        let right_clearance = bounds.max_x - (from.x + from.width).max(to.x + to.width);
        let side = if (left_clearance - right_clearance).abs() <= 1.0 {
            let avg_x = (from.x + from.width * 0.5 + to.x + to.width * 0.5) * 0.5;
            if avg_x <= (bounds.min_x + bounds.max_x) * 0.5 {
                EdgeSide::Left
            } else {
                EdgeSide::Right
            }
        } else if left_clearance <= right_clearance {
            EdgeSide::Left
        } else {
            EdgeSide::Right
        };
        (side, side, fallback.2)
    }
}

fn side_direction(side: EdgeSide) -> (f32, f32) {
    match side {
        EdgeSide::Left => (-1.0, 0.0),
        EdgeSide::Right => (1.0, 0.0),
        EdgeSide::Top => (0.0, -1.0),
        EdgeSide::Bottom => (0.0, 1.0),
    }
}

fn node_center(node: &NodeLayout) -> (f32, f32) {
    (node.x + node.width * 0.5, node.y + node.height * 0.5)
}

fn side_alignment_penalty(side: EdgeSide, from: &NodeLayout, to: &NodeLayout) -> f32 {
    let (fx, fy) = node_center(from);
    let (tx, ty) = node_center(to);
    let dx = tx - fx;
    let dy = ty - fy;
    let len = (dx * dx + dy * dy).sqrt().max(1.0);
    let dir = side_direction(side);
    let dot = (dir.0 * dx + dir.1 * dy) / len;
    // Ports that point away from the opposite endpoint are visually surprising,
    // especially for low-degree leaves around diamonds and hubs. Keep the
    // penalty continuous so a route with much better hard geometry can still win.
    (1.0 - dot).max(0.0)
}

fn candidate_horizontal_sides(from: &NodeLayout, to: &NodeLayout) -> (EdgeSide, EdgeSide, bool) {
    let from_c = node_center(from);
    let to_c = node_center(to);
    if to_c.0 >= from_c.0 {
        (EdgeSide::Right, EdgeSide::Left, to.x + to.width < from.x)
    } else {
        (EdgeSide::Left, EdgeSide::Right, to.x > from.x + from.width)
    }
}

fn candidate_vertical_sides(from: &NodeLayout, to: &NodeLayout) -> (EdgeSide, EdgeSide, bool) {
    let from_c = node_center(from);
    let to_c = node_center(to);
    if to_c.1 >= from_c.1 {
        (EdgeSide::Bottom, EdgeSide::Top, to.y + to.height < from.y)
    } else {
        (EdgeSide::Top, EdgeSide::Bottom, to.y > from.y + from.height)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RoutedSideSearchProfile {
    max_candidates: usize,
    fast_route: bool,
    use_grid: bool,
    use_existing_segments: bool,
}

fn flowchart_side_search_profile(
    graph: &Graph,
    layout_node_count: usize,
    tiny_graph: bool,
) -> Option<RoutedSideSearchProfile> {
    if graph.kind != DiagramKind::Flowchart || graph.edges.is_empty() {
        return None;
    }

    let edge_count = graph.edges.len();
    let node_count = layout_node_count.max(1);
    let dense = edge_count.saturating_mul(2) >= node_count.saturating_mul(3);
    let compound = !graph.subgraphs.is_empty();

    // Port-side scoring is now available for every flowchart, including compound
    // graphs. Keep the expensive full router for small/medium diagrams where it
    // materially improves ports, and fall back to bounded fast scoring for large
    // or dense diagrams so port assignment remains predictable.
    let profile = if edge_count <= 64 {
        RoutedSideSearchProfile {
            max_candidates: 5,
            fast_route: tiny_graph,
            use_grid: !tiny_graph,
            use_existing_segments: true,
        }
    } else if edge_count <= 160 || compound || dense {
        RoutedSideSearchProfile {
            max_candidates: 4,
            fast_route: true,
            use_grid: false,
            use_existing_segments: edge_count <= 160,
        }
    } else {
        RoutedSideSearchProfile {
            max_candidates: 3,
            fast_route: true,
            use_grid: false,
            use_existing_segments: false,
        }
    };

    Some(profile)
}

fn push_unique_side_candidate_limited(
    candidates: &mut Vec<(EdgeSide, EdgeSide, bool)>,
    candidate: (EdgeSide, EdgeSide, bool),
    limit: usize,
) {
    if candidates
        .iter()
        .any(|(start, end, _)| *start == candidate.0 && *end == candidate.1)
    {
        return;
    }
    if candidates.len() < limit.max(1) {
        candidates.push(candidate);
    }
}

fn push_priority_side_candidate_limited(
    candidates: &mut Vec<(EdgeSide, EdgeSide, bool)>,
    candidate: (EdgeSide, EdgeSide, bool),
    limit: usize,
) {
    if candidates
        .iter()
        .any(|(start, end, _)| *start == candidate.0 && *end == candidate.1)
    {
        return;
    }
    let limit = limit.max(1);
    if candidates.len() < limit {
        candidates.push(candidate);
    } else if let Some(slot) = candidates.last_mut() {
        *slot = candidate;
    }
}

fn collect_routed_side_candidates(
    from: &NodeLayout,
    to: &NodeLayout,
    primary: (EdgeSide, EdgeSide, bool),
    balanced: (EdgeSide, EdgeSide, bool),
    edge_role: roles::FlowchartEdgeRole,
    graph_direction: crate::ir::Direction,
    content_bounds: Option<NodeBounds>,
    limit: usize,
) -> Vec<(EdgeSide, EdgeSide, bool)> {
    let mut candidates = Vec::with_capacity(limit.max(1));
    push_unique_side_candidate_limited(&mut candidates, primary, limit);
    push_unique_side_candidate_limited(&mut candidates, balanced, limit);

    let horizontal = candidate_horizontal_sides(from, to);
    let vertical = candidate_vertical_sides(from, to);
    let from_c = node_center(from);
    let to_c = node_center(to);
    if (to_c.0 - from_c.0).abs() >= (to_c.1 - from_c.1).abs() {
        push_unique_side_candidate_limited(&mut candidates, horizontal, limit);
        push_unique_side_candidate_limited(&mut candidates, vertical, limit);
    } else {
        push_unique_side_candidate_limited(&mut candidates, vertical, limit);
        push_unique_side_candidate_limited(&mut candidates, horizontal, limit);
    }
    if edge_role.is_back_edge {
        push_priority_side_candidate_limited(
            &mut candidates,
            choose_outer_back_edge_sides(from, to, graph_direction, content_bounds, balanced),
            limit,
        );
    }
    candidates
}

fn allow_low_degree_balancing_for_edge(
    edge: &crate::ir::Edge,
    edge_role: roles::FlowchartEdgeRole,
    from_degree: usize,
    to_degree: usize,
) -> bool {
    from_degree <= 4
        && to_degree <= 4
        && (edge.style == crate::ir::EdgeStyle::Dotted
            || edge_role.is_back_edge
            || edge_role.crosses_subgraph_boundary)
}

fn routed_side_candidate_score(
    from_id: &str,
    to_id: &str,
    from: &NodeLayout,
    to: &NodeLayout,
    candidate: (EdgeSide, EdgeSide, bool),
    primary: (EdgeSide, EdgeSide, bool),
    edge_role: roles::FlowchartEdgeRole,
    graph_direction: crate::ir::Direction,
    node_degrees: &HashMap<String, usize>,
    side_loads: &HashMap<String, [usize; 4]>,
    obstacles: &[Obstacle],
    label_obstacles: &[Obstacle],
    routing_grid: Option<&RoutingGrid>,
    existing_segments: &[Segment],
    config: &LayoutConfig,
    profile: RoutedSideSearchProfile,
) -> f32 {
    let route_ctx = RouteContext {
        from_id,
        to_id,
        from,
        to,
        direction: graph_direction,
        config,
        obstacles,
        label_obstacles,
        fast_route: profile.fast_route,
        base_offset: 0.0,
        start_side: candidate.0,
        end_side: candidate.1,
        start_offset: 0.0,
        end_offset: 0.0,
        stub_len: port_stub_length(config, from, to),
        start_inset: 0.0,
        end_inset: 0.0,
        prefer_shorter_ties: true,
        preferred_label_id: None,
        preferred_label_center: None,
        preferred_label_obstacle: None,
        preferred_label_clearance: 0.0,
        force_preferred_label_via: false,
        coarse_grid_retry: true,
    };
    let existing = (!existing_segments.is_empty()).then_some(existing_segments);
    let grid = profile.use_grid.then_some(routing_grid).flatten();
    let points = route_edge_with_avoidance(&route_ctx, None, grid, existing);
    let hard_hits = path_obstacle_intersections(&points, obstacles, from_id, to_id) as f32;
    let label_hits = path_label_intersections(&points, label_obstacles, None) as f32;
    let (crossings, overlap) = if existing_segments.is_empty() {
        (0usize, 0.0)
    } else {
        edge_crossings_with_existing(&points, existing_segments)
    };
    let bends = path_bend_count(&points) as f32;
    let len = path_length(&points);
    let from_load = side_load_for_node(side_loads, from_id, candidate.0) as f32;
    let to_load = side_load_for_node(side_loads, to_id, candidate.1) as f32;
    let congestion = from_load * from_load + to_load * to_load + (from_load + to_load) * 0.5;
    let from_degree = node_degrees.get(from_id).copied().unwrap_or(0);
    let to_degree = node_degrees.get(to_id).copied().unwrap_or(0);
    let low_degree_edge = from_degree <= 4 && to_degree <= 4;
    let primary_deviation = if candidate.0 == primary.0 && candidate.1 == primary.1 {
        0.0
    } else if low_degree_edge {
        24.0
    } else {
        8.0
    };
    let alignment = side_alignment_penalty(candidate.0, from, to)
        + side_alignment_penalty(candidate.1, to, from);
    let backward_penalty = if candidate.2 && !primary.2 { 8.0 } else { 0.0 };
    let back_edge_outer_bonus = if edge_role.is_back_edge
        && candidate.0 == candidate.1
        && edge_axis_is_horizontal(candidate.0) != edge_axis_is_horizontal(primary.0)
    {
        -10.0
    } else {
        0.0
    };

    hard_hits * 100_000.0
        + label_hits * 20_000.0
        + crossings as f32 * 1_600.0
        + overlap * 70.0
        + bends * 42.0
        + len * 0.09
        + congestion * 5.5
        + alignment * 34.0
        + primary_deviation
        + backward_penalty
        + back_edge_outer_bonus
}

fn choose_routed_flowchart_sides(
    from_id: &str,
    to_id: &str,
    from: &NodeLayout,
    to: &NodeLayout,
    primary: (EdgeSide, EdgeSide, bool),
    balanced: (EdgeSide, EdgeSide, bool),
    edge_role: roles::FlowchartEdgeRole,
    graph_direction: crate::ir::Direction,
    content_bounds: Option<NodeBounds>,
    node_degrees: &HashMap<String, usize>,
    side_loads: &HashMap<String, [usize; 4]>,
    obstacles: &[Obstacle],
    label_obstacles: &[Obstacle],
    routing_grid: Option<&RoutingGrid>,
    existing_segments: &[Segment],
    config: &LayoutConfig,
    profile: RoutedSideSearchProfile,
) -> (EdgeSide, EdgeSide, bool) {
    let candidates = collect_routed_side_candidates(
        from,
        to,
        primary,
        balanced,
        edge_role,
        graph_direction,
        content_bounds,
        profile.max_candidates,
    );

    let scored_existing_segments = if profile.use_existing_segments {
        existing_segments
    } else {
        &[]
    };
    let mut best = primary;
    let mut best_score = f32::INFINITY;
    for candidate in candidates {
        let score = routed_side_candidate_score(
            from_id,
            to_id,
            from,
            to,
            candidate,
            primary,
            edge_role,
            graph_direction,
            node_degrees,
            side_loads,
            obstacles,
            label_obstacles,
            routing_grid,
            scored_existing_segments,
            config,
            profile,
        );
        if score < best_score {
            best_score = score;
            best = candidate;
        }
    }
    best
}

#[derive(Debug, Clone, Copy)]
struct PortRouteScore {
    hard: usize,
    endpoint_reentries: usize,
    non_endpoint_hits: usize,
    label_hits: usize,
    port_collisions: usize,
    crossings: usize,
    overlap: f32,
    bends: usize,
    len: f32,
    offset_drift: f32,
}

fn port_route_score_is_better(candidate: PortRouteScore, best: PortRouteScore) -> bool {
    if candidate.hard != best.hard {
        return candidate.hard < best.hard;
    }
    if candidate.endpoint_reentries != best.endpoint_reentries {
        return candidate.endpoint_reentries < best.endpoint_reentries;
    }
    if candidate.non_endpoint_hits != best.non_endpoint_hits {
        return candidate.non_endpoint_hits < best.non_endpoint_hits;
    }
    if candidate.bends != best.bends {
        return candidate.bends < best.bends;
    }
    if (candidate.len - best.len).abs() > 1.0 {
        return candidate.len < best.len;
    }
    if (candidate.offset_drift - best.offset_drift).abs() > 0.5 {
        return candidate.offset_drift < best.offset_drift;
    }
    if candidate.label_hits != best.label_hits {
        return candidate.label_hits < best.label_hits;
    }
    if candidate.crossings != best.crossings {
        return candidate.crossings < best.crossings;
    }
    if (candidate.overlap - best.overlap).abs() > 0.05 {
        return candidate.overlap < best.overlap;
    }
    if candidate.port_collisions != best.port_collisions {
        return candidate.port_collisions < best.port_collisions;
    }
    false
}

fn effective_edge_endpoint_layouts(
    graph: &Graph,
    nodes: &BTreeMap<String, NodeLayout>,
    subgraphs: &[SubgraphLayout],
    edge: &crate::ir::Edge,
) -> Option<(NodeLayout, NodeLayout)> {
    let from_layout = nodes.get(&edge.from)?;
    let to_layout = nodes.get(&edge.to)?;
    let from = from_layout
        .anchor_subgraph
        .and_then(|anchor_idx| {
            subgraphs
                .get(anchor_idx)
                .map(|sub| anchor_layout_for_edge(from_layout, sub, graph.direction, true))
        })
        .unwrap_or_else(|| from_layout.clone());
    let to = to_layout
        .anchor_subgraph
        .and_then(|anchor_idx| {
            subgraphs
                .get(anchor_idx)
                .map(|sub| anchor_layout_for_edge(to_layout, sub, graph.direction, false))
        })
        .unwrap_or_else(|| to_layout.clone());
    Some((from, to))
}

fn port_axis_center(node: &NodeLayout, side: EdgeSide) -> f32 {
    if side_is_vertical(side) {
        node.y + node.height / 2.0
    } else {
        node.x + node.width / 2.0
    }
}

fn max_port_offset_for_side(node: &NodeLayout, side: EdgeSide) -> f32 {
    if side_is_vertical(side) {
        (node.height / 2.0 - 1.0).max(0.0)
    } else {
        (node.width / 2.0 - 1.0).max(0.0)
    }
}

fn clamp_port_offset_for_side(node: &NodeLayout, side: EdgeSide, offset: f32) -> f32 {
    let max_offset = max_port_offset_for_side(node, side);
    if max_offset > 0.0 {
        offset.clamp(-max_offset, max_offset)
    } else {
        0.0
    }
}

fn ideal_offset_for_side(remote: (f32, f32), node: &NodeLayout, side: EdgeSide) -> f32 {
    clamp_port_offset_for_side(
        node,
        side,
        ideal_port_pos(remote, node, side) - port_axis_center(node, side),
    )
}

fn push_unique_offset(candidates: &mut Vec<f32>, value: f32, limit: usize) {
    if candidates
        .iter()
        .any(|existing| (*existing - value).abs() <= 0.75)
    {
        return;
    }
    if candidates.len() < limit.max(1) {
        candidates.push(value);
    }
}

fn port_offset_candidates(
    node: &NodeLayout,
    side: EdgeSide,
    current_offset: f32,
    remote: (f32, f32),
    config: &LayoutConfig,
    limit: usize,
) -> Vec<f32> {
    let mut offsets = Vec::with_capacity(limit.max(1));
    let current = clamp_port_offset_for_side(node, side, current_offset);
    let ideal = ideal_offset_for_side(remote, node, side);
    let step = routing_cell_size(config)
        .max(config.node_spacing * 0.18)
        .min(max_port_offset_for_side(node, side).max(1.0));

    push_unique_offset(&mut offsets, current, limit);
    push_unique_offset(&mut offsets, ideal, limit);
    push_unique_offset(&mut offsets, 0.0, limit);
    if limit > 3 && step > 0.5 {
        push_unique_offset(
            &mut offsets,
            clamp_port_offset_for_side(node, side, ideal + step),
            limit,
        );
        push_unique_offset(
            &mut offsets,
            clamp_port_offset_for_side(node, side, ideal - step),
            limit,
        );
    }
    offsets
}

fn port_refinement_offset_limit(graph: &Graph, profile: RoutedSideSearchProfile) -> usize {
    if graph.edges.len() <= 32 && profile.max_candidates >= 5 {
        4
    } else if graph.edges.len() <= 160 {
        3
    } else {
        2
    }
}

fn route_points_for_port_candidate(
    graph: &Graph,
    edge: &crate::ir::Edge,
    from: &NodeLayout,
    to: &NodeLayout,
    port: EdgePortInfo,
    base_offset: f32,
    obstacles: &[Obstacle],
    label_obstacles: &[Obstacle],
    routing_grid: Option<&RoutingGrid>,
    existing_segments: &[Segment],
    config: &LayoutConfig,
    profile: RoutedSideSearchProfile,
) -> Vec<(f32, f32)> {
    let stub_len = port_stub_length(config, from, to);
    let route_ctx = RouteContext {
        from_id: &edge.from,
        to_id: &edge.to,
        from,
        to,
        direction: graph.direction,
        config,
        obstacles,
        label_obstacles,
        fast_route: profile.fast_route,
        base_offset,
        start_side: port.start_side,
        end_side: port.end_side,
        start_offset: port.start_offset,
        end_offset: port.end_offset,
        stub_len,
        start_inset: if edge.arrow_start {
            crate::edge_geometry::arrowhead_inset(graph.kind, edge.arrow_start_kind)
        } else {
            0.0
        },
        end_inset: if edge.arrow_end {
            crate::edge_geometry::arrowhead_inset(graph.kind, edge.arrow_end_kind)
        } else {
            0.0
        },
        prefer_shorter_ties: true,
        preferred_label_id: None,
        preferred_label_center: None,
        preferred_label_obstacle: None,
        preferred_label_clearance: 0.0,
        force_preferred_label_via: false,
        coarse_grid_retry: true,
    };
    let grid = profile.use_grid.then_some(routing_grid).flatten();
    // For full port refinement, generate the candidate's natural path first and
    // score crossings/overlap against provisional segments afterward. Feeding the
    // provisional segments into the router here makes straight backbone edges bend
    // just to avoid a preview-only conflict, which then defeats later straight-line
    // cleanup and produces visibly worse ports.
    let _ = existing_segments;
    route_edge_with_avoidance(&route_ctx, None, grid, None)
}

fn collect_port_choice_segments(
    graph: &Graph,
    nodes: &BTreeMap<String, NodeLayout>,
    subgraphs: &[SubgraphLayout],
    edge_ports: &[EdgePortInfo],
    excluded_idx: usize,
) -> Vec<Segment> {
    let mut segments = Vec::new();
    for (idx, edge) in graph.edges.iter().enumerate() {
        if idx == excluded_idx || edge.from == edge.to {
            continue;
        }
        let Some(port) = edge_ports.get(idx).copied() else {
            continue;
        };
        let Some((from, to)) = effective_edge_endpoint_layouts(graph, nodes, subgraphs, edge)
        else {
            continue;
        };
        segments.push((
            anchor_point_for_node(&from, port.start_side, port.start_offset),
            anchor_point_for_node(&to, port.end_side, port.end_offset),
        ));
    }
    segments
}

fn port_collision_count(
    graph: &Graph,
    nodes: &BTreeMap<String, NodeLayout>,
    edge_ports: &[EdgePortInfo],
    edge_idx: usize,
    candidate: EdgePortInfo,
    config: &LayoutConfig,
) -> usize {
    let Some(edge) = graph.edges.get(edge_idx) else {
        return 0;
    };
    let min_sep = routing_cell_size(config)
        .max(config.node_spacing * 0.22)
        .min(28.0);
    let mut collisions = 0usize;
    for (other_idx, other_edge) in graph.edges.iter().enumerate() {
        if other_idx == edge_idx {
            continue;
        }
        let Some(other) = edge_ports.get(other_idx).copied() else {
            continue;
        };
        for (node_id, side, offset) in [
            (&edge.from, candidate.start_side, candidate.start_offset),
            (&edge.to, candidate.end_side, candidate.end_offset),
        ] {
            let Some(node) = nodes.get(node_id) else {
                continue;
            };
            let candidate_axis =
                port_axis_center(node, side) + clamp_port_offset_for_side(node, side, offset);
            for (other_node_id, other_side, other_offset) in [
                (&other_edge.from, other.start_side, other.start_offset),
                (&other_edge.to, other.end_side, other.end_offset),
            ] {
                if other_node_id == node_id && other_side == side {
                    let other_axis = port_axis_center(node, other_side)
                        + clamp_port_offset_for_side(node, other_side, other_offset);
                    if (candidate_axis - other_axis).abs() < min_sep {
                        collisions += 1;
                    }
                }
            }
        }
    }
    collisions
}

fn score_port_route_candidate(
    points: &[(f32, f32)],
    edge: &crate::ir::Edge,
    nodes: &BTreeMap<String, NodeLayout>,
    obstacles: &[Obstacle],
    label_obstacles: &[Obstacle],
    existing_segments: &[Segment],
    current: EdgePortInfo,
    candidate: EdgePortInfo,
    port_collisions: usize,
) -> PortRouteScore {
    let direction_violations =
        path_cleanup::flowchart_endpoint_direction_violation_count(points, edge, nodes);
    let obstacle_hits = path_obstacle_intersections(points, obstacles, &edge.from, &edge.to);
    let non_endpoint_hits = usize::from(path_cleanup::flowchart_path_hits_non_endpoint_nodes(
        points, &edge.from, &edge.to, nodes,
    ));
    let endpoint_reentries = path_cleanup::flowchart_endpoint_reentry_count(points, edge, nodes);
    let label_hits = path_label_intersections(points, label_obstacles, None);
    let (crossings, overlap) = if existing_segments.is_empty() {
        (0usize, 0.0)
    } else {
        edge_crossings_with_existing(points, existing_segments)
    };
    let side_drift = if candidate.start_side == current.start_side {
        (candidate.start_offset - current.start_offset).abs()
    } else {
        32.0 + candidate.start_offset.abs() * 0.25
    } + if candidate.end_side == current.end_side {
        (candidate.end_offset - current.end_offset).abs()
    } else {
        32.0 + candidate.end_offset.abs() * 0.25
    };
    PortRouteScore {
        hard: direction_violations + obstacle_hits,
        endpoint_reentries,
        non_endpoint_hits,
        label_hits,
        port_collisions,
        crossings,
        overlap,
        bends: path_bend_count(points),
        len: path_length(points),
        offset_drift: side_drift,
    }
}

fn refine_flowchart_ports_with_route_candidates(
    graph: &Graph,
    nodes: &BTreeMap<String, NodeLayout>,
    subgraphs: &[SubgraphLayout],
    obstacles: &[Obstacle],
    label_obstacles: &[Obstacle],
    routing_grid: Option<&RoutingGrid>,
    edge_roles: &[roles::FlowchartEdgeRole],
    content_bounds: Option<NodeBounds>,
    node_degrees: &HashMap<String, usize>,
    side_loads: &HashMap<String, [usize; 4]>,
    edge_ports: &mut [EdgePortInfo],
    config: &LayoutConfig,
    profile: RoutedSideSearchProfile,
) {
    let offset_limit = port_refinement_offset_limit(graph, profile);
    let predicted_lane_assignments = plan::plan_edge_lanes(graph, nodes, subgraphs, config);
    let predicted_lane_offsets =
        predicted_lane_assignments.effective_offsets(edge_ports, graph.kind, config);
    for (idx, edge) in graph.edges.iter().enumerate() {
        if edge.from == edge.to {
            continue;
        }
        let Some(current) = edge_ports.get(idx).copied() else {
            continue;
        };
        let Some((from, to)) = effective_edge_endpoint_layouts(graph, nodes, subgraphs, edge)
        else {
            continue;
        };
        let base_offset = predicted_lane_offsets.get(idx).copied().unwrap_or_default();
        let from_degree = node_degrees.get(&edge.from).copied().unwrap_or(0);
        let to_degree = node_degrees.get(&edge.to).copied().unwrap_or(0);
        let edge_role = edge_roles.get(idx).copied().unwrap_or_default();
        let primary = edge_sides(&from, &to, graph.direction);
        let balanced = edge_sides_balanced(
            &edge.from,
            &edge.to,
            &from,
            &to,
            allow_low_degree_balancing_for_edge(edge, edge_role, from_degree, to_degree),
            edge_role.is_back_edge,
            graph.direction,
            node_degrees,
            side_loads,
        );
        let side_candidates = collect_routed_side_candidates(
            &from,
            &to,
            primary,
            balanced,
            edge_role,
            graph.direction,
            content_bounds,
            profile.max_candidates,
        );
        let existing_segments = if profile.use_existing_segments {
            collect_port_choice_segments(graph, nodes, subgraphs, edge_ports, idx)
        } else {
            Vec::new()
        };
        let baseline_points = route_points_for_port_candidate(
            graph,
            edge,
            &from,
            &to,
            current,
            base_offset,
            obstacles,
            label_obstacles,
            routing_grid,
            &existing_segments,
            config,
            profile,
        );
        let baseline_collisions =
            port_collision_count(graph, nodes, edge_ports, idx, current, config);
        let mut best_score = score_port_route_candidate(
            &baseline_points,
            edge,
            nodes,
            obstacles,
            label_obstacles,
            &existing_segments,
            current,
            current,
            baseline_collisions,
        );
        let mut best_port = current;
        let remote_to = node_center(&to);
        let remote_from = node_center(&from);

        for (start_side, end_side, _) in side_candidates {
            let start_current = if start_side == current.start_side {
                current.start_offset
            } else {
                ideal_offset_for_side(remote_to, &from, start_side)
            };
            let end_current = if end_side == current.end_side {
                current.end_offset
            } else {
                ideal_offset_for_side(remote_from, &to, end_side)
            };
            let start_offsets = port_offset_candidates(
                &from,
                start_side,
                start_current,
                remote_to,
                config,
                offset_limit,
            );
            let end_offsets = port_offset_candidates(
                &to,
                end_side,
                end_current,
                remote_from,
                config,
                offset_limit,
            );
            for start_offset in &start_offsets {
                for end_offset in &end_offsets {
                    let candidate_port = EdgePortInfo {
                        start_side,
                        end_side,
                        start_offset: *start_offset,
                        end_offset: *end_offset,
                    };
                    let points = route_points_for_port_candidate(
                        graph,
                        edge,
                        &from,
                        &to,
                        candidate_port,
                        base_offset,
                        obstacles,
                        label_obstacles,
                        routing_grid,
                        &existing_segments,
                        config,
                        profile,
                    );
                    let collisions =
                        port_collision_count(graph, nodes, edge_ports, idx, candidate_port, config);
                    let score = score_port_route_candidate(
                        &points,
                        edge,
                        nodes,
                        obstacles,
                        label_obstacles,
                        &existing_segments,
                        current,
                        candidate_port,
                        collisions,
                    );
                    if best_score.hard == 0 && score.hard > 0 {
                        continue;
                    }
                    if best_score.non_endpoint_hits == 0 && score.non_endpoint_hits > 0 {
                        continue;
                    }
                    if score.endpoint_reentries > best_score.endpoint_reentries {
                        continue;
                    }
                    if score.len > best_score.len * 3.0 + config.node_spacing * 4.0 {
                        continue;
                    }
                    if port_route_score_is_better(score, best_score) {
                        best_score = score;
                        best_port = candidate_port;
                    }
                }
            }
        }

        if let Some(port) = edge_ports.get_mut(idx) {
            *port = best_port;
        }
    }
}

pub(in crate::layout) struct RoutedEdgeBuildContext<'a> {
    pub(in crate::layout) graph: &'a Graph,
    pub(in crate::layout) nodes: &'a BTreeMap<String, NodeLayout>,
    pub(in crate::layout) subgraphs: &'a [SubgraphLayout],
    pub(in crate::layout) config: &'a LayoutConfig,
    pub(in crate::layout) layout_node_count: usize,
    pub(in crate::layout) edge_route_labels: &'a [Option<TextBlock>],
    pub(in crate::layout) edge_start_labels: &'a [Option<TextBlock>],
    pub(in crate::layout) edge_end_labels: &'a [Option<TextBlock>],
    pub(in crate::layout) label_dummy_ids: &'a [Option<String>],
    pub(in crate::layout) tiny_graph: bool,
    pub(in crate::layout) stage_metrics: Option<&'a mut LayoutStageMetrics>,
}

pub(in crate::layout) fn build_routed_edges(ctx: RoutedEdgeBuildContext<'_>) -> Vec<EdgeLayout> {
    let RoutedEdgeBuildContext {
        graph,
        nodes,
        subgraphs,
        config,
        layout_node_count,
        edge_route_labels,
        edge_start_labels,
        edge_end_labels,
        label_dummy_ids,
        tiny_graph,
        stage_metrics,
    } = ctx;
    let obstacles = build_obstacles(nodes, subgraphs, config);
    let label_obstacles = build_label_obstacles_for_routing(nodes, subgraphs);
    let routing_grid = if config.flowchart.routing.enable_grid_router && !tiny_graph {
        build_routing_grid(&obstacles, config)
    } else {
        None
    };
    let mut stage_metrics = stage_metrics;

    let port_assignment_start = Instant::now();
    let content_bounds = visible_node_bounds(nodes);
    let mut node_degrees: HashMap<String, usize> = HashMap::new();
    for edge in &graph.edges {
        *node_degrees.entry(edge.from.clone()).or_insert(0) += 1;
        *node_degrees.entry(edge.to.clone()).or_insert(0) += 1;
    }
    let edge_roles = roles::classify_edge_roles(graph);
    let mut side_loads: HashMap<String, [usize; 4]> = HashMap::new();
    let routed_side_search_profile =
        flowchart_side_search_profile(graph, layout_node_count, tiny_graph);
    let mut edge_ports: Vec<EdgePortInfo> = vec![
        EdgePortInfo {
            start_side: EdgeSide::Right,
            end_side: EdgeSide::Left,
            start_offset: 0.0,
            end_offset: 0.0,
        };
        graph.edges.len()
    ];
    let mut selected_edge_sides: Vec<(EdgeSide, EdgeSide)> =
        vec![(EdgeSide::Right, EdgeSide::Left); graph.edges.len()];
    let mut port_candidates: HashMap<(String, PortTrack), Vec<PortCandidate>> = HashMap::new();
    let mut side_choice_segments: Vec<Segment> = Vec::with_capacity(graph.edges.len());
    for (idx, edge) in graph.edges.iter().enumerate() {
        let (Some(from_layout), Some(to_layout)) = (nodes.get(&edge.from), nodes.get(&edge.to))
        else {
            continue;
        };
        let temp_from = from_layout.anchor_subgraph.and_then(|anchor_idx| {
            subgraphs
                .get(anchor_idx)
                .map(|sub| anchor_layout_for_edge(from_layout, sub, graph.direction, true))
        });
        let temp_to = to_layout.anchor_subgraph.and_then(|anchor_idx| {
            subgraphs
                .get(anchor_idx)
                .map(|sub| anchor_layout_for_edge(to_layout, sub, graph.direction, false))
        });
        let from = temp_from.as_ref().unwrap_or(from_layout);
        let to = temp_to.as_ref().unwrap_or(to_layout);
        let use_balanced_sides = !matches!(graph.kind, DiagramKind::Architecture);
        let from_degree = node_degrees.get(&edge.from).copied().unwrap_or(0);
        let to_degree = node_degrees.get(&edge.to).copied().unwrap_or(0);
        let edge_role = edge_roles.get(idx).copied().unwrap_or_default();
        let allow_low_degree_balancing =
            allow_low_degree_balancing_for_edge(edge, edge_role, from_degree, to_degree);
        let primary_sides = edge_sides(from, to, graph.direction);
        let balanced = edge_sides_balanced(
            &edge.from,
            &edge.to,
            from,
            to,
            allow_low_degree_balancing,
            edge_role.is_back_edge,
            graph.direction,
            &node_degrees,
            &side_loads,
        );
        let mut selected_sides = if use_balanced_sides
            && edge.from != edge.to
            && let Some(profile) = routed_side_search_profile
        {
            choose_routed_flowchart_sides(
                &edge.from,
                &edge.to,
                from,
                to,
                primary_sides,
                balanced,
                edge_role,
                graph.direction,
                content_bounds,
                &node_degrees,
                &side_loads,
                &obstacles,
                &label_obstacles,
                routing_grid.as_ref(),
                &side_choice_segments,
                config,
                profile,
            )
        } else if use_balanced_sides {
            if edge_role.is_back_edge {
                choose_outer_back_edge_sides(from, to, graph.direction, content_bounds, balanced)
            } else {
                balanced
            }
        } else {
            primary_sides
        };
        if use_balanced_sides
            && !edge_role.is_back_edge
            && (selected_sides.0 != primary_sides.0 || selected_sides.1 != primary_sides.1)
        {
            let candidate_points = [
                anchor_point_for_node(from, selected_sides.0, 0.0),
                anchor_point_for_node(to, selected_sides.1, 0.0),
            ];
            let primary_points = [
                anchor_point_for_node(from, primary_sides.0, 0.0),
                anchor_point_for_node(to, primary_sides.1, 0.0),
            ];
            let (candidate_crossings, _) =
                edge_crossings_with_existing(&candidate_points, &side_choice_segments);
            let (primary_crossings, _) =
                edge_crossings_with_existing(&primary_points, &side_choice_segments);
            if candidate_crossings > primary_crossings {
                selected_sides = primary_sides;
            }
        }
        let (start_side, end_side, _is_backward) = selected_sides;
        bump_side_load(&mut side_loads, &edge.from, start_side);
        bump_side_load(&mut side_loads, &edge.to, end_side);
        edge_ports[idx] = EdgePortInfo {
            start_side,
            end_side,
            start_offset: 0.0,
            end_offset: 0.0,
        };
        selected_edge_sides[idx] = (start_side, end_side);

        let from_anchor = anchor_point_for_node(from, start_side, 0.0);
        let to_anchor = anchor_point_for_node(to, end_side, 0.0);
        side_choice_segments.push((from_anchor, to_anchor));
    }
    let mut node_side_counts: HashMap<String, [usize; 4]> = HashMap::new();
    for (idx, edge) in graph.edges.iter().enumerate() {
        let (start_side, end_side) = selected_edge_sides[idx];
        bump_side_load(&mut node_side_counts, &edge.from, start_side);
        bump_side_load(&mut node_side_counts, &edge.to, end_side);
    }
    for (idx, edge) in graph.edges.iter().enumerate() {
        let (Some(from_layout), Some(to_layout)) = (nodes.get(&edge.from), nodes.get(&edge.to))
        else {
            continue;
        };
        let temp_from = from_layout.anchor_subgraph.and_then(|anchor_idx| {
            subgraphs
                .get(anchor_idx)
                .map(|sub| anchor_layout_for_edge(from_layout, sub, graph.direction, true))
        });
        let temp_to = to_layout.anchor_subgraph.and_then(|anchor_idx| {
            subgraphs
                .get(anchor_idx)
                .map(|sub| anchor_layout_for_edge(to_layout, sub, graph.direction, false))
        });
        let from = temp_from.as_ref().unwrap_or(from_layout);
        let to = temp_to.as_ref().unwrap_or(to_layout);
        let from_degree = node_degrees.get(&edge.from).copied().unwrap_or(0);
        let to_degree = node_degrees.get(&edge.to).copied().unwrap_or(0);
        let (start_side, end_side) = selected_edge_sides[idx];
        let start_counts = node_side_counts.get(&edge.from).copied().unwrap_or([0; 4]);
        let end_counts = node_side_counts.get(&edge.to).copied().unwrap_or([0; 4]);
        let from_anchor = anchor_point_for_node(from, start_side, 0.0);
        let to_anchor = anchor_point_for_node(to, end_side, 0.0);
        let start_other = ideal_port_pos((to_anchor.0, to_anchor.1), from, start_side);
        let end_other = ideal_port_pos((from_anchor.0, from_anchor.1), to, end_side);
        let start_track = port_track_for_assignment(from, start_side, from_degree, start_counts);
        let end_track = port_track_for_assignment(to, end_side, to_degree, end_counts);
        port_candidates
            .entry((edge.from.clone(), start_track))
            .or_default()
            .push(PortCandidate {
                edge_idx: idx,
                is_start: true,
                other_pos: start_other,
            });
        port_candidates
            .entry((edge.to.clone(), end_track))
            .or_default()
            .push(PortCandidate {
                edge_idx: idx,
                is_start: false,
                other_pos: end_other,
            });
    }
    let routing_cell = routing_cell_size(config);
    for ((node_id, track), candidates) in port_candidates {
        let Some(node) = nodes.get(&node_id) else {
            continue;
        };
        let mut min_other = f32::MAX;
        let mut max_other = f32::MIN;
        for candidate in &candidates {
            min_other = min_other.min(candidate.other_pos);
            max_other = max_other.max(candidate.other_pos);
        }
        let span = (max_other - min_other).max(0.0);
        let mut order: Vec<usize> = (0..candidates.len()).collect();
        order.sort_by(|&a, &b| {
            candidates[a]
                .other_pos
                .partial_cmp(&candidates[b].other_pos)
                .unwrap_or(Ordering::Equal)
        });
        let node_len = port_track_node_len(node, track);
        let pad = (node_len * config.flowchart.port_pad_ratio)
            .min(config.flowchart.port_pad_max)
            .max(config.flowchart.port_pad_min);
        let usable = (node_len - 2.0 * pad).max(1.0);
        let nominal_sep = usable / (candidates.len() as f32 + 1.0);
        let labeled_edges = candidates
            .iter()
            .filter(|candidate| {
                graph.edges.get(candidate.edge_idx).is_some_and(|edge| {
                    edge.label
                        .as_deref()
                        .is_some_and(|label| !label.trim().is_empty())
                        || edge
                            .start_label
                            .as_deref()
                            .is_some_and(|label| !label.trim().is_empty())
                        || edge
                            .end_label
                            .as_deref()
                            .is_some_and(|label| !label.trim().is_empty())
                })
            })
            .count() as f32;
        let congestion = candidates.len() as f32;
        let sep_boost =
            1.0 + (labeled_edges * 0.07).min(0.35) + ((congestion - 3.0).max(0.0) * 0.03).min(0.25);
        let grid_floor = if routing_cell > 0.0 {
            routing_cell * 0.85
        } else {
            0.0
        };
        let desired_sep = (nominal_sep * sep_boost).max(grid_floor);
        let feasible_sep = if candidates.len() <= 1 {
            usable
        } else {
            usable / (candidates.len() as f32 - 0.15)
        };
        let min_sep = desired_sep.min(feasible_sep.max(nominal_sep));
        let snap_to_grid = config.flowchart.routing.snap_ports_to_grid
            && routing_cell > 0.0
            && min_sep >= routing_cell * 0.75;
        let node_start = port_track_node_start(node, track);
        let ideal_span = span;
        let span_frac = if usable > 1.0 {
            (ideal_span / usable).min(2.0)
        } else {
            1.0
        };
        let position_weight = (0.5 + 0.35 * span_frac).clamp(0.50, 0.85);
        let rank_weight = 1.0 - position_weight;
        let desired: Vec<(usize, f32)> = order
            .iter()
            .enumerate()
            .map(|(rank, &idx)| {
                let candidate = &candidates[idx];
                let pos_in_node = candidate.other_pos - node_start;
                let t_pos = ((pos_in_node - pad) / usable).clamp(0.0, 1.0);
                let t_rank = (rank as f32 + 0.5) / candidates.len() as f32;
                let t = t_pos * position_weight + t_rank * rank_weight;
                let pos = pad + t * usable;
                (idx, pos)
            })
            .collect();
        let mut assigned = vec![0.0; candidates.len()];
        let mut prev = pad;
        for (order_idx, (cand_idx, pos)) in desired.iter().enumerate() {
            let mut p = *pos;
            if order_idx == 0 {
                p = p.max(pad);
            } else {
                p = p.max(prev + min_sep);
            }
            assigned[*cand_idx] = p;
            prev = p;
        }
        let mut next = pad + usable;
        for (order_idx, (cand_idx, _pos)) in desired.iter().enumerate().rev() {
            let mut p = assigned[*cand_idx];
            if order_idx + 1 == desired.len() {
                p = p.min(next);
            } else {
                p = p.min(next - min_sep);
            }
            assigned[*cand_idx] = p;
            next = p;
        }
        for (rank, &cand_idx) in order.iter().enumerate() {
            let candidate = &candidates[cand_idx];
            let mut offset = assigned[cand_idx] - node_len / 2.0;
            if snap_to_grid {
                offset = (offset / routing_cell).round() * routing_cell;
            }
            if config.flowchart.port_side_bias != 0.0 {
                let side_bias_scale = if candidates.len() > 2 {
                    1.0 + ((candidates.len() as f32 - 2.0) * 0.08).min(0.6)
                } else {
                    1.0
                };
                offset += config.flowchart.port_side_bias
                    * side_bias_scale
                    * (rank as f32 - (candidates.len() as f32 - 1.0) / 2.0);
            }
            if let Some(info) = edge_ports.get_mut(candidate.edge_idx) {
                if candidate.is_start {
                    info.start_offset = offset;
                } else {
                    info.end_offset = offset;
                }
            }
        }
    }
    if let Some(profile) = routed_side_search_profile {
        refine_flowchart_ports_with_route_candidates(
            graph,
            nodes,
            subgraphs,
            &obstacles,
            &label_obstacles,
            routing_grid.as_ref(),
            &edge_roles,
            content_bounds,
            &node_degrees,
            &side_loads,
            &mut edge_ports,
            config,
            profile,
        );
    }
    if let Some(metrics) = stage_metrics.as_mut() {
        metrics.port_assignment_us = metrics
            .port_assignment_us
            .saturating_add(port_assignment_start.elapsed().as_micros());
    }

    let edge_routing_start = Instant::now();
    let lane_assignments = plan::plan_edge_lanes(graph, nodes, subgraphs, config);
    let lane_offsets = lane_assignments.effective_offsets(&edge_ports, graph.kind, config);
    let pair_counts = lane_assignments.pair_counts;
    let pair_index = lane_assignments.pair_index;
    let cross_edge_offsets = lane_assignments.cross_edge_offsets;

    let mut route_order: Vec<(u8, f32, f32, usize)> = Vec::with_capacity(graph.edges.len());
    let dense_flowchart_routing = graph.kind == DiagramKind::Flowchart
        && graph.edges.len() >= 18
        && graph.edges.len() * 2 >= layout_node_count * 3;
    for (idx, edge) in graph.edges.iter().enumerate() {
        let (Some(from_layout), Some(to_layout)) = (nodes.get(&edge.from), nodes.get(&edge.to))
        else {
            continue;
        };
        let temp_from = from_layout.anchor_subgraph.and_then(|idx| {
            subgraphs
                .get(idx)
                .map(|sub| anchor_layout_for_edge(from_layout, sub, graph.direction, true))
        });
        let temp_to = to_layout.anchor_subgraph.and_then(|idx| {
            subgraphs
                .get(idx)
                .map(|sub| anchor_layout_for_edge(to_layout, sub, graph.direction, false))
        });
        let from = temp_from.as_ref().unwrap_or(from_layout);
        let to = temp_to.as_ref().unwrap_or(to_layout);
        let from_center = (from.x + from.width / 2.0, from.y + from.height / 2.0);
        let to_center = (to.x + to.width / 2.0, to.y + to.height / 2.0);
        let dx = to_center.0 - from_center.0;
        let dy = to_center.1 - from_center.1;
        let cross_axis = if is_horizontal(graph.direction) {
            dy.abs()
        } else {
            dx.abs()
        };
        let main_axis = if is_horizontal(graph.direction) {
            dx.abs()
        } else {
            dy.abs()
        };
        let (_, _, is_backward) = edge_sides(from, to, graph.direction);
        let is_dotted = edge.style == crate::ir::EdgeStyle::Dotted;
        let has_label = edge.label.is_some();
        let is_secondary = is_dotted || has_label;
        let has_open_triangle = matches!(
            edge.arrow_start_kind,
            Some(crate::ir::EdgeArrowhead::OpenTriangle)
        ) || matches!(
            edge.arrow_end_kind,
            Some(crate::ir::EdgeArrowhead::OpenTriangle)
        );
        let priority = if graph.kind == DiagramKind::Class {
            if has_open_triangle {
                0u8
            } else if is_secondary || is_backward {
                1u8
            } else {
                2u8
            }
        } else if graph.kind == DiagramKind::State {
            if is_backward {
                0u8
            } else if has_label || is_dotted {
                1u8
            } else {
                2u8
            }
        } else if is_dotted {
            if dense_flowchart_routing { 1u8 } else { 2u8 }
        } else if has_label || is_backward {
            1u8
        } else {
            0u8
        };
        route_order.push((priority, cross_axis, main_axis, idx));
    }
    let steep_count = route_order
        .iter()
        .filter(|(_, cross_axis, main_axis, _)| *cross_axis > *main_axis * 0.8)
        .count();
    let use_cross_axis_order = graph.edges.len() >= 10 && steep_count * 4 >= graph.edges.len();
    if use_cross_axis_order {
        route_order.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal))
                .then_with(|| b.2.partial_cmp(&a.2).unwrap_or(Ordering::Equal))
                .then_with(|| a.3.cmp(&b.3))
        });
    } else {
        let use_priority_preorder = graph.edges.len() >= 10;
        route_order.sort_by(|a, b| {
            let len_a = a.1 * a.1 + a.2 * a.2;
            let len_b = b.1 * b.1 + b.2 * b.2;
            let by_length = len_b.partial_cmp(&len_a).unwrap_or(Ordering::Equal);
            let dense_by_length = len_a.partial_cmp(&len_b).unwrap_or(Ordering::Equal);
            if use_priority_preorder {
                a.0.cmp(&b.0)
                    .then_with(|| {
                        if dense_flowchart_routing {
                            dense_by_length
                        } else {
                            by_length
                        }
                    })
                    .then_with(|| a.3.cmp(&b.3))
            } else {
                by_length.then_with(|| a.3.cmp(&b.3))
            }
        });
    }

    let mut routed_points: Vec<Vec<(f32, f32)>> = vec![Vec::new(); graph.edges.len()];
    let use_occupancy = !tiny_graph && graph.edges.len() > 2;
    let mut edge_occupancy = if use_occupancy {
        Some(EdgeOccupancy::new(
            config.node_spacing.max(MIN_NODE_SPACING_FLOOR) * EDGE_OCCUPANCY_CELL_RATIO,
        ))
    } else {
        None
    };
    let route_labels_via = route_labels::should_route_labels_via(graph, nodes);
    let (edge_label_pad_x, edge_label_pad_y) =
        label_placement::edge_label_padding(graph.kind, config);
    let (mut route_label_plans, mut route_label_obstacles) =
        route_labels::initialize_route_label_plans(
            graph,
            nodes,
            subgraphs,
            &edge_ports,
            &pair_index,
            &lane_offsets,
            edge_route_labels,
            label_obstacles,
            config,
        );
    let mut existing_segments: Vec<Segment> = Vec::new();
    let mut label_anchors: Vec<Option<(f32, f32)>> = vec![None; graph.edges.len()];
    for (_, _, _, idx) in &route_order {
        let edge = &graph.edges[*idx];
        let key = edge_pair_key(edge);
        let total = *pair_counts.get(&key).unwrap_or(&1) as f32;
        let idx_in_pair = pair_index[*idx] as f32;
        let base_offset = if graph.kind == DiagramKind::Flowchart {
            lane_offsets.get(*idx).copied().unwrap_or_default()
        } else if total > 1.0 {
            (idx_in_pair - (total - 1.0) / 2.0) * (config.node_spacing * MULTI_EDGE_OFFSET_RATIO)
                + cross_edge_offsets[*idx]
        } else {
            cross_edge_offsets[*idx]
        };
        let (Some(from_layout), Some(to_layout)) = (nodes.get(&edge.from), nodes.get(&edge.to))
        else {
            continue;
        };
        let temp_from = from_layout.anchor_subgraph.and_then(|idx| {
            subgraphs
                .get(idx)
                .map(|sub| anchor_layout_for_edge(from_layout, sub, graph.direction, true))
        });
        let temp_to = to_layout.anchor_subgraph.and_then(|idx| {
            subgraphs
                .get(idx)
                .map(|sub| anchor_layout_for_edge(to_layout, sub, graph.direction, false))
        });
        let from = temp_from.as_ref().unwrap_or(from_layout);
        let to = temp_to.as_ref().unwrap_or(to_layout);
        let port_info = edge_ports.get(*idx).copied().unwrap_or(EdgePortInfo {
            start_side: EdgeSide::Right,
            end_side: EdgeSide::Left,
            start_offset: 0.0,
            end_offset: 0.0,
        });
        let default_stub = port_stub_length(config, from, to);
        let stub_len = match graph.kind {
            DiagramKind::Class | DiagramKind::Er | DiagramKind::Requirement => 0.0,
            _ => default_stub,
        };
        let max_edge_label_chars = [
            edge.label.as_deref(),
            edge.start_label.as_deref(),
            edge.end_label.as_deref(),
        ]
        .into_iter()
        .flatten()
        .map(|label| label.chars().count())
        .max()
        .unwrap_or(0);
        let has_endpoint_label = edge.start_label.is_some() || edge.end_label.is_some();
        let avoid_short_tie = graph.kind == DiagramKind::Flowchart
            && (has_endpoint_label
                || max_edge_label_chars >= FLOWCHART_EDGE_LABEL_WRAP_TRIGGER_CHARS);
        let preferred_label_plan = route_label_plans.get(*idx).and_then(|plan| plan.as_ref());
        let preferred_label_id = preferred_label_plan.map(|plan| plan.obstacle_id.as_str());
        let preferred_label_obstacle =
            preferred_label_plan.and_then(|plan| route_label_obstacles.get(plan.obstacle_index));
        let preferred_label_clearance = if graph.kind == DiagramKind::Flowchart {
            (edge_label_pad_x.max(edge_label_pad_y) + config.node_spacing * 0.25).max(8.0)
        } else {
            0.0
        };
        let preferred_label_center = if matches!(graph.kind, DiagramKind::State | DiagramKind::Er)
            || graph.kind == DiagramKind::Flowchart
        {
            None
        } else {
            preferred_label_plan.map(|plan| plan.center)
        };
        let start_inset = if edge.arrow_start {
            crate::edge_geometry::arrowhead_inset(graph.kind, edge.arrow_start_kind)
        } else {
            0.0
        };
        let end_inset = if edge.arrow_end {
            crate::edge_geometry::arrowhead_inset(graph.kind, edge.arrow_end_kind)
        } else {
            0.0
        };
        let route_ctx = RouteContext {
            from_id: &edge.from,
            to_id: &edge.to,
            from,
            to,
            direction: graph.direction,
            config,
            obstacles: &obstacles,
            label_obstacles: &route_label_obstacles,
            fast_route: tiny_graph,
            base_offset,
            start_side: port_info.start_side,
            end_side: port_info.end_side,
            start_offset: port_info.start_offset,
            end_offset: port_info.end_offset,
            stub_len,
            start_inset,
            end_inset,
            prefer_shorter_ties: !avoid_short_tie,
            preferred_label_id,
            preferred_label_center,
            preferred_label_obstacle,
            preferred_label_clearance,
            force_preferred_label_via: graph.kind != DiagramKind::Flowchart,
            coarse_grid_retry: graph.kind == DiagramKind::Flowchart,
        };
        let use_existing_for_edge = !(matches!(graph.kind, DiagramKind::Class | DiagramKind::Er)
            && edge.style == crate::ir::EdgeStyle::Dotted);
        let existing_for_edge = if use_existing_for_edge {
            Some(existing_segments.as_slice())
        } else {
            None
        };
        let mut points = route_edge_with_avoidance(
            &route_ctx,
            edge_occupancy.as_ref(),
            routing_grid.as_ref(),
            existing_for_edge,
        );
        if matches!(graph.kind, DiagramKind::Class | DiagramKind::Er) {
            let fast_ctx = RouteContext {
                from_id: route_ctx.from_id,
                to_id: route_ctx.to_id,
                from: route_ctx.from,
                to: route_ctx.to,
                direction: route_ctx.direction,
                config: route_ctx.config,
                obstacles: route_ctx.obstacles,
                label_obstacles: route_ctx.label_obstacles,
                fast_route: true,
                base_offset: route_ctx.base_offset,
                start_side: route_ctx.start_side,
                end_side: route_ctx.end_side,
                start_offset: route_ctx.start_offset,
                end_offset: route_ctx.end_offset,
                stub_len: route_ctx.stub_len,
                start_inset: route_ctx.start_inset,
                end_inset: route_ctx.end_inset,
                prefer_shorter_ties: route_ctx.prefer_shorter_ties,
                preferred_label_id: route_ctx.preferred_label_id,
                preferred_label_center: route_ctx.preferred_label_center,
                preferred_label_obstacle: route_ctx.preferred_label_obstacle,
                preferred_label_clearance: route_ctx.preferred_label_clearance,
                force_preferred_label_via: route_ctx.force_preferred_label_via,
                coarse_grid_retry: route_ctx.coarse_grid_retry,
            };
            let fast_points = route_edge_with_avoidance(&fast_ctx, None, None, existing_for_edge);
            let fast_hits = path_obstacle_intersections(
                &fast_points,
                route_ctx.obstacles,
                route_ctx.from_id,
                route_ctx.to_id,
            );
            let fast_label_hits = path_label_intersections(
                &fast_points,
                route_ctx.label_obstacles,
                route_ctx.preferred_label_id,
            );
            if fast_hits == 0 && fast_label_hits == 0 {
                let (fast_cross, fast_overlap) =
                    edge_crossings_with_existing(&fast_points, &existing_segments);
                let (cur_cross, cur_overlap) =
                    edge_crossings_with_existing(&points, &existing_segments);
                if fast_cross < cur_cross
                    || (fast_cross == cur_cross && fast_overlap + 0.25 < cur_overlap)
                {
                    points = fast_points;
                }
            }
        }
        if route_labels_via {
            let mut sync_ctx = route_labels::RouteLabelSyncContext {
                direction: graph.direction,
                kind: graph.kind,
                route_label_plans: &mut route_label_plans,
                label_anchors: &mut label_anchors,
                edge_route_labels,
                route_label_obstacles: &mut route_label_obstacles,
                edge_label_pad_x,
                edge_label_pad_y,
                update_obstacle: true,
            };
            route_labels::sync_route_label_plan_with_points(*idx, &mut points, &mut sync_ctx);
        }
        if let Some(occ) = edge_occupancy.as_mut() {
            occ.add_path(&points);
        }
        if points.len() >= 2 {
            for segment in points.windows(2) {
                existing_segments.push((segment[0], segment[1]));
            }
        }
        routed_points[*idx] = points;
    }

    post_route::apply_edge_path_cleanup(graph, nodes, &mut routed_points, config);

    if route_labels_via {
        for idx in 0..routed_points.len() {
            let mut sync_ctx = route_labels::RouteLabelSyncContext {
                direction: graph.direction,
                kind: graph.kind,
                route_label_plans: &mut route_label_plans,
                label_anchors: &mut label_anchors,
                edge_route_labels,
                route_label_obstacles: &mut route_label_obstacles,
                edge_label_pad_x,
                edge_label_pad_y,
                update_obstacle: false,
            };
            route_labels::sync_route_label_plan_with_points(
                idx,
                &mut routed_points[idx],
                &mut sync_ctx,
            );
        }
    }
    if graph.kind == DiagramKind::Flowchart {
        path_cleanup::detour_flowchart_paths_around_non_endpoint_nodes(
            graph,
            nodes,
            &mut routed_points,
            config,
        );
        path_cleanup::simplify_flowchart_axis_oscillations(&mut routed_points);
        path_cleanup::detour_flowchart_paths_around_non_endpoint_nodes(
            graph,
            nodes,
            &mut routed_points,
            config,
        );
    }

    route_labels::apply_label_dummy_anchors(
        nodes,
        label_dummy_ids,
        &mut routed_points,
        &mut label_anchors,
        graph.direction,
        graph.kind,
    );
    if graph.kind == DiagramKind::Flowchart {
        path_cleanup::detour_flowchart_paths_around_non_endpoint_nodes(
            graph,
            nodes,
            &mut routed_points,
            config,
        );
        enforce_flowchart_endpoint_ports(graph, nodes, &edge_ports, &mut routed_points, config);
        path_cleanup::repair_flowchart_endpoint_reentries(graph, nodes, &mut routed_points, config);
        repair_flowchart_endpoint_reentries_by_rerouting(
            graph,
            nodes,
            subgraphs,
            &obstacles,
            &route_label_obstacles,
            routing_grid.as_ref(),
            &mut edge_ports,
            &lane_offsets,
            &mut routed_points,
            config,
        );
        path_cleanup::repair_flowchart_endpoint_reentries(graph, nodes, &mut routed_points, config);
    }
    if graph.kind == DiagramKind::Flowchart {
        let route_label_centers = route_labels::route_label_centers(&route_label_plans);
        let plan_snapshot = plan::FlowchartLayoutPlan::from_current_pipeline(
            graph,
            nodes,
            subgraphs,
            &edge_ports,
            &pair_counts,
            &pair_index,
            &cross_edge_offsets,
            &routed_points,
            &label_anchors,
            &route_label_centers,
            edge_route_labels,
            config,
        );
        debug_assert!(plan_snapshot.is_consistent());
    }
    if let Some(metrics) = stage_metrics {
        metrics.edge_routing_us = metrics
            .edge_routing_us
            .saturating_add(edge_routing_start.elapsed().as_micros());
    }

    post_route::build_edge_layouts(
        graph,
        &routed_points,
        edge_route_labels,
        edge_start_labels,
        edge_end_labels,
        &label_anchors,
        config,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Edge, EdgeStyle, NodeShape, NodeStyle, Subgraph};
    use crate::layout::TextBlock;

    fn edge(from: &str, to: &str) -> Edge {
        Edge {
            from: from.to_string(),
            to: to.to_string(),
            label: None,
            start_label: None,
            end_label: None,
            directed: true,
            arrow_start: false,
            arrow_end: true,
            arrow_start_kind: None,
            arrow_end_kind: None,
            start_decoration: None,
            end_decoration: None,
            style: EdgeStyle::Solid,
        }
    }

    fn node(shape: NodeShape) -> NodeLayout {
        NodeLayout {
            id: "n".to_string(),
            x: 0.0,
            y: 0.0,
            width: 120.0,
            height: 80.0,
            label: TextBlock {
                lines: Vec::new(),
                width: 0.0,
                height: 0.0,
            },
            shape,
            style: NodeStyle::default(),
            link: None,
            anchor_subgraph: None,
            hidden: false,
            icon: None,
        }
    }

    #[test]
    fn axis_wide_track_skips_simple_pass_through_axis() {
        let counts = [1, 0, 1, 1];
        assert!(!use_axis_wide_port_track(
            &node(NodeShape::Diamond),
            EdgeSide::Top,
            3,
            counts,
        ));
        assert_eq!(
            port_track_for_assignment(&node(NodeShape::Diamond), EdgeSide::Top, 3, counts),
            PortTrack::Side(EdgeSide::Top)
        );
    }

    #[test]
    fn axis_wide_track_engages_for_real_same_axis_contention() {
        let counts = [0, 0, 2, 1];
        assert!(use_axis_wide_port_track(
            &node(NodeShape::Diamond),
            EdgeSide::Top,
            3,
            counts,
        ));
        assert_eq!(
            port_track_for_assignment(&node(NodeShape::Diamond), EdgeSide::Top, 3, counts),
            PortTrack::Axis(PortAxis::X)
        );
    }

    #[test]
    fn side_search_profile_includes_compound_flowcharts() {
        let mut graph = Graph::new();
        graph.edges = (0..40).map(|_| edge("a", "b")).collect();
        graph.subgraphs.push(Subgraph {
            id: Some("cluster".to_string()),
            label: "cluster".to_string(),
            nodes: vec!["a".to_string()],
            direction: None,
            icon: None,
        });

        let profile = flowchart_side_search_profile(&graph, 12, false)
            .expect("compound flowcharts should use route-scored side search");
        assert_eq!(profile.max_candidates, 5);
        assert!(!profile.fast_route);
        assert!(profile.use_grid);
    }

    #[test]
    fn side_search_profile_bounds_large_flowcharts() {
        let mut graph = Graph::new();
        graph.edges = (0..200).map(|_| edge("a", "b")).collect();

        let profile = flowchart_side_search_profile(&graph, 200, false)
            .expect("large flowcharts should still get bounded side search");
        assert_eq!(profile.max_candidates, 3);
        assert!(profile.fast_route);
        assert!(!profile.use_grid);
        assert!(!profile.use_existing_segments);
    }

    #[test]
    fn endpoint_port_enforcement_repairs_inward_segments() {
        let mut graph = Graph::new();
        graph.edges.push(edge("a", "b"));

        let mut nodes = BTreeMap::new();
        let mut a = node(NodeShape::Rectangle);
        a.id = "a".to_string();
        let mut b = node(NodeShape::Rectangle);
        b.id = "b".to_string();
        b.x = 240.0;
        nodes.insert("a".to_string(), a);
        nodes.insert("b".to_string(), b);

        let ports = vec![EdgePortInfo {
            start_side: EdgeSide::Right,
            end_side: EdgeSide::Left,
            start_offset: 0.0,
            end_offset: 0.0,
        }];
        let mut routed_points = vec![vec![
            (120.0, 40.0),
            (100.0, 40.0),
            (260.0, 40.0),
            (240.0, 40.0),
        ]];

        enforce_flowchart_endpoint_ports(
            &graph,
            &nodes,
            &ports,
            &mut routed_points,
            &LayoutConfig::default(),
        );

        let points = &routed_points[0];
        assert!(side_points_outward(EdgeSide::Right, points[0], points[1]));
        assert!(side_points_outward(
            EdgeSide::Left,
            *points.last().unwrap(),
            points[points.len() - 2]
        ));
    }

    #[test]
    fn endpoint_port_enforcement_repairs_self_loop_final_leg() {
        let mut graph = Graph::new();
        graph.edges.push(edge("a", "a"));

        let mut nodes = BTreeMap::new();
        let mut a = node(NodeShape::Rectangle);
        a.id = "a".to_string();
        nodes.insert("a".to_string(), a);

        let mut routed_points = vec![vec![
            (120.0, 40.0),
            (140.0, 40.0),
            (140.0, 100.0),
            (10.0, 100.0),
            (10.0, 40.0),
            (0.0, 40.0),
        ]];

        enforce_flowchart_endpoint_ports(
            &graph,
            &nodes,
            &[],
            &mut routed_points,
            &LayoutConfig::default(),
        );

        let points = &routed_points[0];
        assert!(side_points_outward(EdgeSide::Right, points[0], points[1]));
        assert!(side_points_outward(
            EdgeSide::Left,
            *points.last().unwrap(),
            points[points.len() - 2]
        ));
    }
}
