// Radar edge-case regression suite.
//
// Codifies the verified current behavior for radar curves with non-numeric
// values, empty tokens, curve/axis count mismatches, and named
// (`axis: value`) entries, so future refactors cannot silently change it.
//
// Reference semantics (mermaid.js, packages/parser radar.langium +
// packages/mermaid/src/diagrams/radar):
// - Non-numeric or empty values are a PARSE ERROR upstream. We are more
//   lenient and drop/zero-fill instead. Deviations are documented per test.
// - Named entries are mapped by axis name in declared-axis order upstream.
//   After the parser fix (radar_named_entry in src/parser.rs), our named
//   entries survive to the renderer and map by name too.
// - Upstream skips curves whose entry count != axis count; we truncate
//   extras and pad missing values with 0 (center).

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

/// Extract curve polygon path data (curves render with fill-opacity="0.5").
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

fn assert_point_near(actual: (f32, f32), expected: (f32, f32), what: &str) {
    assert!(
        (actual.0 - expected.0).abs() < 0.01 && (actual.1 - expected.1).abs() < 0.01,
        "{what}: expected ({:.3},{:.3}), got ({:.3},{:.3})",
        expected.0,
        expected.1,
        actual.0,
        actual.1
    );
}

fn assert_at_center(actual: (f32, f32), what: &str) {
    assert!(
        actual.0.abs() < 0.001 && actual.1.abs() < 0.001,
        "{what}: expected center, got ({:.3},{:.3})",
        actual.0,
        actual.1
    );
}

/// Whether the radar renders an axis label with this exact text.
fn has_axis_label(svg: &str, axis: &str) -> bool {
    svg.split("<text ").skip(1).any(|chunk| {
        chunk.contains("dominant-baseline=\"middle\"")
            && chunk
                .split_once('>')
                .map(|(_, rest)| rest.starts_with(&format!("{axis}</text>")))
                .unwrap_or(false)
    })
}

// ====================================================================
// Non-numeric values (upstream: parse error; ours: lenient drop/zero)
// ====================================================================

/// FIXED (was deviation D1): the declared axis list is now authoritative
/// (DiagramData::Radar carries it structurally), so a non-numeric value in
/// the FIRST curve no longer deletes that axis chart-wide. The bad token
/// zero-fills at the center and every other value keeps its correct axis.
#[test]
fn nonnumeric_value_in_first_curve_keeps_axis_chart_wide() {
    let input = r#"radar-beta
  axis A, B, C
  curve Alpha {3, abc, 5}
  curve Beta {1, 2, 3}
"#;
    let (graph, svg) = render(input);
    assert_eq!(graph.kind, DiagramKind::Radar);
    let paths = curve_paths(&svg);
    assert_eq!(paths.len(), 2, "both curves should still render");
    for d in &paths {
        assert_eq!(
            path_points(d).len(),
            3,
            "all declared axes keep a vertex despite the bad token"
        );
    }
    assert!(has_axis_label(&svg, "A"), "axis A should render");
    assert!(
        has_axis_label(&svg, "B"),
        "axis B must survive Alpha's bad value"
    );
    assert!(has_axis_label(&svg, "C"), "axis C should render");
    // Alpha's bad B token sits at the center; Beta's valid B=2 survives.
    // max=5 => scale 60. Alpha B at center; Beta A=1 -> r=60 at -90deg.
    let alpha = path_points(paths[0]);
    assert_at_center(alpha[1], "Alpha's non-numeric B vertex zero-fills");
    let beta = path_points(paths[1]);
    assert_point_near(beta[0], (0.0, -60.0), "Beta A vertex");
    let (cos30, sin30) = (3.0f32.sqrt() / 2.0, 0.5f32);
    assert_point_near(
        beta[1],
        (120.0 * cos30, 120.0 * sin30),
        "Beta keeps its valid B=2 value",
    );
}

/// CURRENT BEHAVIOR (documented deviation D2, medium severity): a
/// non-numeric value in a LATER curve is zero-filled, putting that vertex
/// at the chart center. Upstream errors instead of substituting 0.
#[test]
fn nonnumeric_value_in_later_curve_is_zero_filled_at_center() {
    let input = r#"radar-beta
  axis A, B, C
  curve Alpha {1, 2, 3}
  curve Beta {3, abc, 5}
"#;
    let (_, svg) = render(input);
    let paths = curve_paths(&svg);
    assert_eq!(paths.len(), 2);
    assert!(has_axis_label(&svg, "B"), "axes stay intact");
    let beta = path_points(paths[1]);
    assert_eq!(beta.len(), 3, "one vertex per axis");
    assert_at_center(beta[1], "Beta's non-numeric B vertex is treated as 0");
    // A=3 and C=5 survive; max=5 => scale 60. A: r=180 at -90deg.
    assert_point_near(beta[0], (0.0, -180.0), "Beta A vertex");
}

/// FIXED (was deviation D4): a chart whose only curve is entirely
/// non-numeric now keeps the declared axes and renders the curve as a
/// degenerate center polygon (every value zero-fills), with no panic.
/// Upstream emits a parse error; we stay lenient but no longer blank.
#[test]
fn all_nonnumeric_only_curve_renders_axes_and_center_polygon() {
    let input = r#"radar-beta
  axis A, B, C
  curve Alpha {foo, bar, baz}
"#;
    let (graph, svg) = render(input);
    assert_eq!(graph.kind, DiagramKind::Radar);
    let paths = curve_paths(&svg);
    assert_eq!(paths.len(), 1, "curve renders as a degenerate polygon");
    for point in path_points(paths[0]) {
        assert_at_center(point, "non-numeric values zero-fill to the center");
    }
    assert!(
        has_axis_label(&svg, "A") && has_axis_label(&svg, "B") && has_axis_label(&svg, "C"),
        "declared axes render regardless of curve values"
    );
}

// ====================================================================
// Empty token: `{3, , 5}` (KNOWN BUG: positional shift)
// ====================================================================

/// FIXED (was KNOWN BUG D3): the parser now uses a position-preserving
/// comma split (split_args_keep_empty in src/parser.rs), so `{3, , 5}`
/// keeps value 5 bound to axis C. The empty slot zero-fills at the center
/// (upstream treats it as a parse error; this is the lenient equivalent).
#[test]
fn empty_token_does_not_shift_later_values() {
    let input = r#"radar-beta
  axis A, B, C
  curve Alpha {3, , 5}
"#;
    let (_, svg) = render(input);
    assert!(
        has_axis_label(&svg, "C"),
        "axis C must survive an empty token in another slot"
    );
    let paths = curve_paths(&svg);
    assert_eq!(paths.len(), 1);
    let points = path_points(paths[0]);
    assert_eq!(points.len(), 3, "one vertex per declared axis");
    assert_at_center(points[1], "empty B slot zero-fills at the center");
    // Value 5 lands on axis C: max=5 => r=300 at 150deg.
    let (cos30, sin30) = (3.0f32.sqrt() / 2.0, 0.5f32);
    assert_point_near(
        points[2],
        (-300.0 * cos30, 300.0 * sin30),
        "value 5 stays bound to axis C",
    );
}

// ====================================================================
// Named (`axis: value`) entries: fixed to bind by name
// ====================================================================

/// Named entries in any order now map to the correct axes by NAME. The
/// parser previously corrupted `C: 9` into `A: C: 9`, silently dropping
/// every named entry (verified 2026-07: whole curve vanished).
#[test]
fn named_entries_reordered_map_to_correct_axes_by_name() {
    let input = r#"radar-beta
  axis A, B, C
  curve Alpha {1, 2, 3}
  curve Beta { C: 9, A: 3, B: 6 }
"#;
    let (_, svg) = render(input);
    let paths = curve_paths(&svg);
    assert_eq!(
        paths.len(),
        2,
        "named curve must render, not silently vanish"
    );
    let beta = path_points(paths[1]);
    assert_eq!(beta.len(), 3);
    // max=9 => scale 300/9. Beta by declared axis order: A=3, B=6, C=9.
    assert_point_near(beta[0], (0.0, -100.0), "Beta A=3 at -90deg r=100");
    let (cos30, sin30) = (3.0f32.sqrt() / 2.0, 0.5f32);
    assert_point_near(
        beta[1],
        (200.0 * cos30, 200.0 * sin30),
        "Beta B=6 at 30deg r=200",
    );
    assert_point_near(
        beta[2],
        (-300.0 * cos30, 300.0 * sin30),
        "Beta C=9 at 150deg r=300",
    );
}

/// A diagram whose ONLY curve uses the named form now renders instead of
/// producing a blank SVG.
#[test]
fn named_only_curve_renders_instead_of_blank() {
    let input = r#"radar-beta
  axis A, B, C
  curve Alpha { A: 1, B: 2, C: 3 }
"#;
    let (_, svg) = render(input);
    let paths = curve_paths(&svg);
    assert_eq!(paths.len(), 1, "named-only diagram must not be blank");
    let alpha = path_points(paths[0]);
    assert_eq!(alpha.len(), 3);
    // max=3 => scale 100. A=1 -> r=100 at -90deg.
    assert_point_near(alpha[0], (0.0, -100.0), "Alpha A vertex");
    assert!(has_axis_label(&svg, "A") && has_axis_label(&svg, "B") && has_axis_label(&svg, "C"));
}

/// CURRENT BEHAVIOR (documented deviation): a named curve missing an axis
/// zero-fills that axis at the center. Upstream mermaid.js throws
/// "Missing entry for axis C" instead. Zero-fill is our deliberate lenient
/// policy (consistent with short positional curves).
#[test]
fn named_subset_curve_zero_fills_missing_axes() {
    let input = r#"radar-beta
  axis A, B, C
  curve Alpha {1, 2, 3}
  curve Beta { A: 8, B: 4 }
"#;
    let (_, svg) = render(input);
    let paths = curve_paths(&svg);
    assert_eq!(paths.len(), 2, "subset named curve must render");
    let beta = path_points(paths[1]);
    assert_eq!(beta.len(), 3);
    // max=8 => scale 37.5. A=8 -> r=300 at -90deg.
    assert_point_near(beta[0], (0.0, -300.0), "Beta A vertex");
    assert_at_center(beta[2], "missing C entry zero-fills to center");
}

/// Named entries referencing an unknown axis are ignored (matches upstream,
/// which never looks up unknown extras), and the known axes keep their
/// correct values. Previously this rendered a degenerate all-zero polygon.
#[test]
fn named_entry_with_unknown_axis_is_ignored() {
    let input = r#"radar-beta
  axis A, B, C
  curve Alpha {1, 2, 3}
  curve Beta { C: 9, A: 1, B: 2, D: 7 }
"#;
    let (_, svg) = render(input);
    let paths = curve_paths(&svg);
    assert_eq!(paths.len(), 2);
    let beta = path_points(paths[1]);
    assert_eq!(beta.len(), 3, "unknown axis D adds no vertex");
    // max=9 => C=9 at full radius 300 (150deg); A=1 and B=2 small but nonzero.
    let (cos30, sin30) = (3.0f32.sqrt() / 2.0, 0.5f32);
    assert_point_near(
        beta[2],
        (-300.0 * cos30, 300.0 * sin30),
        "Beta C vertex keeps its named value",
    );
    let r0 = (beta[0].0.powi(2) + beta[0].1.powi(2)).sqrt();
    assert!(
        (r0 - 300.0 / 9.0).abs() < 0.01,
        "Beta A=1 keeps its named value, got r={r0}"
    );
    assert!(!has_axis_label(&svg, "D"), "unknown axis D never renders");
}

// ====================================================================
// Positional count mismatches
// ====================================================================

/// CURRENT BEHAVIOR (documented deviation, low severity): extra positional
/// values beyond the axis count are silently truncated. Upstream skips
/// drawing mismatched curves entirely while keeping their legend entry.
#[test]
fn positional_curve_with_extra_values_truncates_extras() {
    let input = r#"radar-beta
  axis A, B, C
  curve Alpha {1, 2, 3}
  curve Beta {4, 5, 6, 7}
"#;
    let (_, svg) = render(input);
    let paths = curve_paths(&svg);
    assert_eq!(paths.len(), 2);
    let beta = path_points(paths[1]);
    assert_eq!(beta.len(), 3, "extra 4th value is truncated");
    // max=6 => scale 50. Beta C=6 -> r=300 at 150deg.
    let (cos30, sin30) = (3.0f32.sqrt() / 2.0, 0.5f32);
    assert_point_near(beta[2], (-300.0 * cos30, 300.0 * sin30), "Beta C vertex");
}

/// FIXED (was a medium-severity deviation): the declared axis list is
/// authoritative, so a short first curve no longer shrinks the whole chart.
/// Its missing axes zero-fill at the center and later curves keep their
/// values on every declared axis (matching upstream's axis handling).
#[test]
fn short_first_curve_keeps_all_declared_axes() {
    let input = r#"radar-beta
  axis A, B, C
  curve Alpha {1, 2}
  curve Beta {4, 5, 6}
"#;
    let (_, svg) = render(input);
    let paths = curve_paths(&svg);
    assert_eq!(paths.len(), 2);
    for d in &paths {
        assert_eq!(path_points(d).len(), 3, "one vertex per declared axis");
    }
    assert!(
        has_axis_label(&svg, "A") && has_axis_label(&svg, "B") && has_axis_label(&svg, "C"),
        "all declared axes render"
    );
    let alpha = path_points(paths[0]);
    assert_at_center(alpha[2], "Alpha's missing C zero-fills at the center");
    // Beta keeps C=6: max=6 => r=300 at 150deg.
    let (cos30, sin30) = (3.0f32.sqrt() / 2.0, 0.5f32);
    let beta = path_points(paths[1]);
    assert_point_near(
        beta[2],
        (-300.0 * cos30, 300.0 * sin30),
        "Beta keeps its C value on the declared third axis",
    );
}

// ====================================================================
// Structural pipeline regressions (DiagramData::Radar)
// ====================================================================

/// Extract every rendered axis label text, in document order. Axis labels
/// are the only radar text elements with dominant-baseline="middle".
fn axis_labels(svg: &str) -> Vec<String> {
    svg.split("<text ")
        .skip(1)
        .filter(|chunk| chunk.contains("dominant-baseline=\"middle\""))
        .filter_map(|chunk| {
            let (_, rest) = chunk.split_once('>')?;
            Some(rest.split('<').next()?.to_string())
        })
        .collect()
}

fn unescape_xml(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// SEVERE data-corruption regression (radar_many_axes_labels): curves used
/// to be encoded as newline-joined "axis: value" node labels, wrapped by
/// measure_label and re-parsed by splitting on ':' in render_radar, so long
/// axis names (20-28 chars) wrapped across lines and dropped/renamed axes,
/// corrupting every polygon. The structural DiagramData::Radar pipeline
/// must render EXACTLY the declared axes, byte-for-byte, in order.
#[test]
fn many_long_axis_names_render_exactly_as_declared() {
    let axes = [
        "Infrastructure Reliability", // 26 chars
        "Operational Excellence",     // 22 chars
        "Customer Satisfaction",      // 21 chars
        "Development Velocity",       // 20 chars
        "Security and Compliance",    // 23 chars
        "Documentation Coverage",     // 22 chars
        "Incident Response Time",     // 22 chars
        "Cross Team Collaboration",   // 24 chars
        "Architecture Scalability",   // 24 chars
        "Cost Efficiency Management", // 26 chars
    ];
    for axis in axes {
        assert!(
            (20..=28).contains(&axis.len()),
            "test precondition: '{axis}' must be 20-28 chars, is {}",
            axis.len()
        );
    }
    let input = format!(
        "radar-beta\n  axis {}\n  curve Alpha {{1,2,3,4,5,6,7,8,9,10}}\n  curve Beta {{10,9,8,7,6,5,4,3,2,1}}\n",
        axes.join(", ")
    );
    let (graph, svg) = render(&input);
    assert_eq!(graph.radar.axes.len(), axes.len(), "parser keeps all axes");

    let rendered: Vec<String> = axis_labels(&svg)
        .into_iter()
        .map(|label| unescape_xml(&label))
        .collect();
    assert_eq!(
        rendered.len(),
        axes.len(),
        "rendered axis count must match the source: got {rendered:?}"
    );
    for (idx, axis) in axes.iter().enumerate() {
        assert_eq!(
            rendered[idx], *axis,
            "axis {idx} must render byte-for-byte as declared"
        );
    }

    // Both polygons keep one vertex per axis.
    let paths = curve_paths(&svg);
    assert_eq!(paths.len(), 2);
    for d in &paths {
        assert_eq!(path_points(d).len(), axes.len());
    }
    // Spot-check geometry survives: Alpha's 10 on the last axis reaches the
    // outer ring (max=10 => r=300) at the last axis angle.
    let alpha = path_points(paths[0]);
    let last = alpha[axes.len() - 1];
    let r = (last.0 * last.0 + last.1 * last.1).sqrt();
    assert!(
        (r - 300.0).abs() < 0.01,
        "Alpha's max value must reach the outer ring, got r={r}"
    );
}

/// Nine axes at exactly the 20 and 28 char boundaries (regression bound of
/// radar_many_axes_labels), single curve.
#[test]
fn nine_axes_boundary_length_names_all_render() {
    let axes: Vec<String> = (0..9)
        .map(|idx| {
            let len = if idx % 2 == 0 { 20 } else { 28 };
            let base = format!("Axis Number {idx} Name Pad");
            let mut name = base;
            while name.len() < len {
                name.push('x');
            }
            name.truncate(len);
            name
        })
        .collect();
    let input = format!(
        "radar-beta\n  axis {}\n  curve Only {{9,8,7,6,5,4,3,2,1}}\n",
        axes.join(", ")
    );
    let (_, svg) = render(&input);
    let rendered = axis_labels(&svg);
    assert_eq!(rendered.len(), 9, "all 9 axes must render: {rendered:?}");
    for axis in &axes {
        assert!(
            rendered.iter().any(|label| label == axis),
            "axis '{axis}' missing from render"
        );
    }
    let paths = curve_paths(&svg);
    assert_eq!(paths.len(), 1);
    assert_eq!(path_points(paths[0]).len(), 9);
}

/// Axis and curve names containing ':' used to corrupt the label re-parse
/// (split on ':' treated "Cost: Ops" as axis "Cost" value "Ops"). The
/// structural pipeline must keep such names intact.
#[test]
fn axis_and_curve_names_containing_colon_survive() {
    let input = r#"radar-beta
  axis "Cost: Operations", "Speed: P99", Quality
  curve "Team: Alpha" {1, 2, 3}
"#;
    let (graph, svg) = render(input);
    assert_eq!(
        graph.radar.axes,
        vec!["Cost: Operations", "Speed: P99", "Quality"]
    );
    let rendered: Vec<String> = axis_labels(&svg)
        .into_iter()
        .map(|label| unescape_xml(&label))
        .collect();
    assert_eq!(rendered, vec!["Cost: Operations", "Speed: P99", "Quality"]);
    assert!(
        svg.contains(">Team: Alpha</text>"),
        "curve legend name with ':' must survive"
    );
    let paths = curve_paths(&svg);
    assert_eq!(paths.len(), 1);
    let points = path_points(paths[0]);
    assert_eq!(points.len(), 3);
    // Values bind positionally despite the colons: max=3 => scale 100.
    assert_point_near(points[0], (0.0, -100.0), "first axis keeps value 1");
}

/// Curves with more values than axes: extras used to become bare-number
/// label lines that were silently dropped (or worse, kept the polygon
/// aligned with a corrupted axis list). Extras must be truncated with all
/// declared axes intact.
#[test]
fn curve_with_more_values_than_axes_truncates_only_extras() {
    let input = r#"radar-beta
  axis A, B, C
  curve Over {1, 2, 3, 4, 5, 6}
"#;
    let (_, svg) = render(input);
    let rendered = axis_labels(&svg);
    assert_eq!(rendered, vec!["A", "B", "C"]);
    let paths = curve_paths(&svg);
    assert_eq!(paths.len(), 1);
    let points = path_points(paths[0]);
    assert_eq!(points.len(), 3, "extras beyond the axis count are dropped");
    // Scale uses only bound values: max=3 (not 6) => C=3 at r=300, 150deg.
    let (cos30, sin30) = (3.0f32.sqrt() / 2.0, 0.5f32);
    assert_point_near(
        points[2],
        (-300.0 * cos30, 300.0 * sin30),
        "C keeps value 3 and unbound extras don't distort the scale",
    );
}

/// A curve declared BEFORE the axis line must bind to the axes all the
/// same: the declared axis list is authoritative for the whole chart,
/// independent of statement order.
#[test]
fn curve_declared_before_axis_line_binds_to_axes() {
    let input = r#"radar-beta
  curve Early {1, 2, 3}
  axis A, B, C
"#;
    let (graph, svg) = render(input);
    assert_eq!(graph.radar.axes, vec!["A", "B", "C"]);
    let rendered = axis_labels(&svg);
    assert_eq!(rendered, vec!["A", "B", "C"]);
    let paths = curve_paths(&svg);
    assert_eq!(paths.len(), 1, "early curve renders");
    let points = path_points(paths[0]);
    assert_eq!(points.len(), 3);
    // max=3 => scale 100; A=1 -> r=100 at -90deg.
    assert_point_near(points[0], (0.0, -100.0), "Early A vertex");
}

/// Curves without any axis line: axes are synthesized (unlabeled) from the
/// longest positional curve so the data still renders instead of vanishing.
#[test]
fn curves_without_axis_line_render_with_synthesized_axes() {
    let input = r#"radar-beta
  curve NoAxes {2, 4, 6, 8}
"#;
    let (graph, svg) = render(input);
    assert!(graph.radar.axes.is_empty(), "no axes declared in source");
    let paths = curve_paths(&svg);
    assert_eq!(paths.len(), 1, "curve renders against synthesized axes");
    let points = path_points(paths[0]);
    assert_eq!(points.len(), 4, "one vertex per synthesized axis");
    assert!(
        axis_labels(&svg).is_empty(),
        "synthesized axes have no labels"
    );
    // max=8 => vertex 4 reaches the outer ring.
    let last = points[3];
    let r = (last.0 * last.0 + last.1 * last.1).sqrt();
    assert!((r - 300.0).abs() < 0.01, "max value reaches the ring");
}

/// min/max/ticks/graticule directives flow structurally to the renderer.
#[test]
fn radar_config_directives_flow_to_renderer() {
    let input = r#"radar-beta
  axis A, B, C
  curve Alpha {2, 5, 8}
  max 10
  min 2
  ticks 4
  graticule polygon
"#;
    let (graph, svg) = render(input);
    assert_eq!(graph.radar.max, Some(10.0));
    assert_eq!(graph.radar.min, Some(2.0));
    assert_eq!(graph.radar.ticks, Some(4));
    assert_eq!(
        graph.radar.graticule,
        mermaid_rs_renderer::RadarGraticule::Polygon
    );

    // Polygon graticule: grid rings render as paths with fill-opacity 0.3.
    let grid_rings = svg
        .split("<path d=\"")
        .skip(1)
        .filter(|chunk| chunk.contains("fill-opacity=\"0.3\""))
        .count();
    assert_eq!(grid_rings, 4, "ticks directive controls ring count");
    assert!(
        !svg.contains("<circle r="),
        "polygon graticule renders no circular rings"
    );

    // Scale [2, 10]: value 2 sits at the center, value 10 would be the ring.
    // Alpha = {2,5,8} -> A at center, C at (8-2)/(10-2)*300 = 225.
    let paths = curve_paths(&svg);
    assert_eq!(paths.len(), 1);
    let points = path_points(paths[0]);
    assert_at_center(points[0], "value at min sits at the center");
    let r2 = (points[2].0.powi(2) + points[2].1.powi(2)).sqrt();
    assert!(
        (r2 - 225.0).abs() < 0.01,
        "value 8 on a [2,10] scale sits at r=225, got {r2}"
    );
}
