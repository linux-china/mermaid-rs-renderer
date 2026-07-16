use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::FlowchartLayoutEngine;
use crate::ir::{Direction, Edge};

use super::super::NodeLayout;
use super::manual_layout::ManualLayoutRanks;

/// Deterministic snapshot of the layered node-placement boundary.
///
/// This is captured immediately after rank assignment, within-rank ordering,
/// and coordinate assignment, before aspect folding, subgraph cleanup, routing,
/// label placement, or rendering can alter the geometry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayeredLayoutSnapshot {
    pub engine: String,
    pub direction: String,
    pub ranks: Vec<LayeredRankSnapshot>,
    pub nodes: Vec<LayeredNodeSnapshot>,
    pub edges: Vec<LayeredEdgeSnapshot>,
    pub feedback_edges: Vec<LayeredFeedbackEdgeSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayeredRankSnapshot {
    pub index: usize,
    pub nodes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayeredNodeSnapshot {
    pub id: String,
    pub rank: usize,
    pub order: usize,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub hidden: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayeredEdgeSnapshot {
    pub from: String,
    pub to: String,
    pub from_rank: usize,
    pub to_rank: usize,
    pub rank_span: usize,
    pub feedback: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayeredFeedbackEdgeSnapshot {
    pub from: String,
    pub to: String,
}

impl LayeredLayoutSnapshot {
    pub(in crate::layout) fn from_stage(
        engine: FlowchartLayoutEngine,
        direction: Direction,
        nodes: &BTreeMap<String, NodeLayout>,
        edges: &[Edge],
        ranks: &ManualLayoutRanks,
    ) -> Self {
        let mut position_by_id: HashMap<&str, (usize, usize)> = HashMap::new();
        let rank_snapshots = ranks
            .rank_nodes
            .iter()
            .enumerate()
            .map(|(rank, bucket)| {
                for (order, id) in bucket.iter().enumerate() {
                    position_by_id.insert(id.as_str(), (rank, order));
                }
                LayeredRankSnapshot {
                    index: rank,
                    nodes: bucket.clone(),
                }
            })
            .collect::<Vec<_>>();

        let node_snapshots = nodes
            .values()
            .filter_map(|node| {
                let (rank, order) = position_by_id.get(node.id.as_str()).copied()?;
                Some(LayeredNodeSnapshot {
                    id: node.id.clone(),
                    rank,
                    order,
                    x: node.x,
                    y: node.y,
                    width: node.width,
                    height: node.height,
                    hidden: node.hidden,
                })
            })
            .collect::<Vec<_>>();

        let feedback_set = ranks
            .feedback_edges
            .iter()
            .map(|(from, to)| (from.as_str(), to.as_str()))
            .collect::<HashSet<_>>();
        let edge_snapshots = edges
            .iter()
            .filter_map(|edge| {
                let (from_rank, _) = position_by_id.get(edge.from.as_str()).copied()?;
                let (to_rank, _) = position_by_id.get(edge.to.as_str()).copied()?;
                let feedback = feedback_set.contains(&(edge.from.as_str(), edge.to.as_str()))
                    || to_rank <= from_rank;
                Some(LayeredEdgeSnapshot {
                    from: edge.from.clone(),
                    to: edge.to.clone(),
                    from_rank,
                    to_rank,
                    rank_span: from_rank.abs_diff(to_rank),
                    feedback,
                })
            })
            .collect::<Vec<_>>();

        let mut feedback_edges = edge_snapshots
            .iter()
            .filter(|edge| edge.feedback)
            .map(|edge| LayeredFeedbackEdgeSnapshot {
                from: edge.from.clone(),
                to: edge.to.clone(),
            })
            .collect::<Vec<_>>();
        feedback_edges.sort_by(|a, b| (&a.from, &a.to).cmp(&(&b.from, &b.to)));
        feedback_edges.dedup();

        Self {
            engine: engine.as_str().to_string(),
            direction: direction_name(direction).to_string(),
            ranks: rank_snapshots,
            nodes: node_snapshots,
            edges: edge_snapshots,
            feedback_edges,
        }
    }
}

fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::TopDown => "TD",
        Direction::BottomTop => "BT",
        Direction::LeftRight => "LR",
        Direction::RightLeft => "RL",
    }
}

pub fn write_layered_layout_dump(
    path: &Path,
    snapshot: &LayeredLayoutSnapshot,
) -> anyhow::Result<()> {
    let file = File::create(path)?;
    let writer = BufWriter::new(file);
    serde_json::to_writer_pretty(writer, snapshot)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::NodeShape;
    use crate::layout::TextBlock;

    #[test]
    fn snapshot_is_ordered_by_rank_and_node_map() {
        let mut nodes = BTreeMap::new();
        for (id, x) in [("A", 0.0), ("B", 100.0)] {
            nodes.insert(
                id.to_string(),
                NodeLayout {
                    id: id.to_string(),
                    x,
                    y: 0.0,
                    width: 40.0,
                    height: 20.0,
                    label: TextBlock {
                        lines: vec![id.to_string()],
                        width: 10.0,
                        height: 10.0,
                    },
                    shape: NodeShape::Rectangle,
                    style: Default::default(),
                    link: None,
                    anchor_subgraph: None,
                    hidden: false,
                    icon: None,
                },
            );
        }
        let ranks = ManualLayoutRanks {
            rank_nodes: vec![vec!["A".into()], vec!["B".into()]],
            feedback_edges: Vec::new(),
        };
        let edges = Vec::new();
        let snapshot = LayeredLayoutSnapshot::from_stage(
            FlowchartLayoutEngine::Current,
            Direction::TopDown,
            &nodes,
            &edges,
            &ranks,
        );
        assert_eq!(snapshot.engine, "current");
        assert_eq!(snapshot.nodes[0].id, "A");
        assert_eq!(snapshot.nodes[1].rank, 1);
    }
}
