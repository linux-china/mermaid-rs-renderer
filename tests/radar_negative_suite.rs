// Radar diagrams with negative curve values.
//
// Upstream mermaid.js radar clips values to [min, max] with `min` defaulting
// to 0 (packages/mermaid/src/diagrams/radar/db.ts `min: 0`; renderer.ts
// `relativeRadius`: clippedValue = Math.min(Math.max(value, minValue),
// maxValue)). Our renderer clamps via `value.max(0.0)` in `parse_series`
// (src/render.rs, render_radar), so negative entries must land at the chart
// center (radius 0) with no panic and no negative radii.

use mermaid_rs_renderer::{
    DiagramKind, LayoutConfig, Theme, compute_layout, parse_mermaid, render_svg,
};

fn render(input: &str) -> (mermaid_rs_renderer::Graph, String) {
    let parsed = parse_mermaid(input).expect("diagram should parse");
    let theme = Theme::modern();
    let config = LayoutConfig::default();
    let layout = compute_layout(&parsed.graph, &theme, &config);
    let svg = render_svg(&layout, &theme, &config);
    (parsed.graph, svg)
}

fn curve_paths(svg: &str) -> Vec<&str> {
    svg.split("<path d=\"")
        .skip(1)
        .filter(|chunk| chunk.contains("fill-opacity=\"0.5\""))
        .map(|chunk| chunk.split('"').next().unwrap())
        .collect()
}

fn path_points(d: &str) -> Vec<(f32, f32)> {
    d.trim_start_matches('M')
        .trim_end_matches(" Z")
        .split(" L")
        .map(|point| {
            let (x, y) = point.split_once(',').expect("point format x,y");
            (x.parse().unwrap(), y.parse().unwrap())
        })
        .collect()
}

#[test]
fn radar_negative_values_clamp_to_center_matching_upstream_min_default() {
    let input = r#"radar-beta
  axis A, B, C
  curve Neg {-3, 5, -1}
  curve Pos {2, 4, 6}
"#;
    let (graph, svg) = render(input);
    assert_eq!(graph.kind, DiagramKind::Radar);
    let paths = curve_paths(&svg);
    assert_eq!(paths.len(), 2, "both curves should render");

    // Chart max is 6 and MAX_RADIUS is 300, so B on the Neg curve (value 5)
    // sits at r=250 => (250*cos30, 250*sin30) = (216.506, 125). A (-3) and
    // C (-1) clamp to the center.
    let neg = path_points(paths[0]);
    assert_eq!(neg.len(), 3, "one point per axis");
    assert!(
        neg[0].0.abs() < 0.001 && neg[0].1.abs() < 0.001,
        "negative A value should clamp to center, got {:?}",
        neg[0]
    );
    assert!(
        (neg[1].0 - 216.506).abs() < 0.01 && (neg[1].1 - 125.0).abs() < 0.01,
        "positive B value should scale normally, got {:?}",
        neg[1]
    );
    assert!(
        neg[2].0.abs() < 0.001 && neg[2].1.abs() < 0.001,
        "negative C value should clamp to center, got {:?}",
        neg[2]
    );

    for d in &paths {
        for (x, y) in path_points(d) {
            let r = (x * x + y * y).sqrt();
            assert!(x.is_finite() && y.is_finite(), "coordinates must be finite");
            assert!(
                r <= 300.01,
                "curve point ({x},{y}) escapes the radar radius"
            );
        }
    }
}

#[test]
fn radar_all_negative_values_render_degenerate_center_polygon_without_panic() {
    let input = r#"radar-beta
  axis A, B, C
  curve AllNeg {-3, -5, -1}
"#;
    let (graph, svg) = render(input);
    assert_eq!(graph.kind, DiagramKind::Radar);
    let paths = curve_paths(&svg);
    assert_eq!(paths.len(), 1, "curve should still render");
    for (x, y) in path_points(paths[0]) {
        assert!(x.is_finite() && y.is_finite(), "coordinates must be finite");
        assert!(
            x.abs() < 0.001 && y.abs() < 0.001,
            "all-negative curve should collapse to the center, got ({x},{y})"
        );
    }
}
