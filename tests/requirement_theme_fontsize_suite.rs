//! Requirement rendering under non-default themes, large theme font sizes,
//! and requirement layout overrides (header_band_height / header_line_gap /
//! label_padding_y).
//!
//! For every combination this renders a requirement fixture whose nodes have
//! a 2-line header (`<<Requirement>>` + name) plus body lines, then asserts
//! numerically from the emitted SVG:
//!   * both header lines' glyph boxes (ascent = font_size * 0.8, box height
//!     = font_size) fit inside the header band above the divider,
//!   * the first body line clears the divider,
//!   * the last body line fits above the box bottom,
//!   * the configured fill/stroke/divider/label colors are actually applied.

use mermaid_rs_renderer::{LayoutConfig, Theme, compute_layout, parse_mermaid, render_svg};

const FIXTURE: &str = r#"requirementDiagram
  requirement req1 {
    id: 1
    text: Login
    risk: medium
    verifymethod: test
  }
  requirement req2 {
    id: 2
    text: Session
    risk: low
    verifymethod: inspection
  }
  req1 - satisfies -> req2
"#;

/// Extract the value of `attr="..."` from an SVG element string.
fn attr<'a>(element: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=\"");
    let start = element.find(&needle)? + needle.len();
    let end = element[start..].find('"')? + start;
    Some(&element[start..end])
}

fn attr_f32(element: &str, name: &str) -> Option<f32> {
    attr(element, name)?.parse().ok()
}

/// All elements with the given tag, excluding anything inside `<defs>`
/// (markers also contain `<line>`/`<circle>` elements).
fn elements<'a>(svg: &'a str, tag: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut rest = svg;
    let mut depth = 0usize;
    let open = "<defs>";
    let close = "</defs>";
    loop {
        let next_defs_open = rest.find(open);
        let next_defs_close = rest.find(close);
        let next_tag = rest.find(&format!("<{tag}"));
        let Some(tag_at) = next_tag else { break };
        // Process whichever boundary (defs open/close or the tag) comes first.
        let open_at = next_defs_open.filter(|&at| at < tag_at);
        let close_at = next_defs_close.filter(|&at| at < tag_at);
        match (open_at, close_at) {
            (Some(o), Some(c)) if o < c => {
                depth += 1;
                rest = &rest[o + open.len()..];
                continue;
            }
            (Some(_), Some(c)) => {
                depth = depth.saturating_sub(1);
                rest = &rest[c + close.len()..];
                continue;
            }
            (Some(o), None) => {
                depth += 1;
                rest = &rest[o + open.len()..];
                continue;
            }
            (None, Some(c)) => {
                depth = depth.saturating_sub(1);
                rest = &rest[c + close.len()..];
                continue;
            }
            (None, None) => {}
        }
        let end = rest[tag_at..].find('>').map(|e| tag_at + e + 1).unwrap();
        if depth == 0 {
            out.push(&rest[tag_at..end]);
        }
        rest = &rest[end..];
    }
    out
}

struct NodeBox {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

struct Case {
    theme_name: &'static str,
    theme: Theme,
    font_size: Option<f32>,
    header_band_height: Option<f32>,
    header_line_gap: Option<f32>,
    label_padding_y: Option<f32>,
}

impl Case {
    fn label(&self) -> String {
        format!(
            "theme={} font_size={:?} band={:?} gap={:?} pad_y={:?}",
            self.theme_name,
            self.font_size,
            self.header_band_height,
            self.header_line_gap,
            self.label_padding_y
        )
    }
}

fn run_case(case: &Case) {
    let mut theme = case.theme.clone();
    if let Some(fs) = case.font_size {
        theme.font_size = fs;
    }
    let mut config = LayoutConfig::default();
    if let Some(v) = case.header_band_height {
        config.requirement.header_band_height = v;
    }
    if let Some(v) = case.header_line_gap {
        config.requirement.header_line_gap = v;
    }
    if let Some(v) = case.label_padding_y {
        config.requirement.label_padding_y = v;
    }

    let parsed = parse_mermaid(FIXTURE).expect("fixture should parse");
    let layout = compute_layout(&parsed.graph, &theme, &config);
    let svg = render_svg(&layout, &theme, &config);
    let ctx = case.label();

    let req = &config.requirement;
    let font_size = theme.font_size;
    let ascent = font_size * 0.8;

    // Node boxes: rects with the requirement fill color. Node coordinates and
    // text/divider coordinates live in the same translated <g>, so they are
    // directly comparable.
    let boxes: Vec<NodeBox> = elements(&svg, "rect")
        .iter()
        .filter(|r| attr(r, "fill") == Some(req.fill.as_str()))
        .filter(|r| attr(r, "data-edge-id").is_none())
        .map(|r| NodeBox {
            x: attr_f32(r, "x").unwrap(),
            y: attr_f32(r, "y").unwrap(),
            width: attr_f32(r, "width").unwrap(),
            height: attr_f32(r, "height").unwrap(),
        })
        .collect();
    assert_eq!(boxes.len(), 2, "{ctx}: expected 2 requirement node boxes");

    // Divider lines: horizontal lines in the requirement divider color.
    let dividers: Vec<(f32, f32)> = elements(&svg, "line")
        .iter()
        .filter(|l| attr(l, "stroke") == Some(req.divider_color.as_str()))
        .map(|l| (attr_f32(l, "x1").unwrap(), attr_f32(l, "y1").unwrap()))
        .collect();
    assert_eq!(dividers.len(), 2, "{ctx}: expected 2 divider lines");

    // Node label texts: anchored at start with the requirement label color
    // (edge labels are middle-anchored and use edge_label_color).
    let texts: Vec<(f32, f32)> = elements(&svg, "text")
        .iter()
        .filter(|t| attr(t, "text-anchor") == Some("start"))
        .filter(|t| attr(t, "fill") == Some(req.label_color.as_str()))
        .map(|t| (attr_f32(t, "x").unwrap(), attr_f32(t, "y").unwrap()))
        .collect();
    // 2 header + 4 body lines per node.
    assert_eq!(texts.len(), 12, "{ctx}: expected 12 label lines");

    for node in &boxes {
        let divider_y = dividers
            .iter()
            .find(|(x1, y1)| {
                (*x1 - node.x).abs() < 0.5 && *y1 > node.y && *y1 < node.y + node.height
            })
            .map(|(_, y1)| *y1)
            .unwrap_or_else(|| panic!("{ctx}: no divider found inside node box"));

        let node_texts: Vec<f32> = texts
            .iter()
            .filter(|(x, y)| {
                *x >= node.x
                    && *x <= node.x + node.width
                    && *y >= node.y - 1.0
                    && *y <= node.y + node.height + font_size
            })
            .map(|(_, y)| *y)
            .collect();
        assert_eq!(
            node_texts.len(),
            6,
            "{ctx}: expected 6 label lines inside node"
        );

        // Baselines sorted top to bottom; first two are the header.
        let mut baselines = node_texts.clone();
        baselines.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let (header, body) = baselines.split_at(2);

        for (i, baseline) in header.iter().enumerate() {
            let glyph_top = baseline - ascent;
            let glyph_bottom = glyph_top + font_size;
            assert!(
                glyph_top >= node.y - 0.01,
                "{ctx}: header line {i} glyph top {glyph_top:.2} above box top {:.2}",
                node.y
            );
            assert!(
                glyph_bottom <= divider_y + 0.01,
                "{ctx}: header line {i} glyph bottom {glyph_bottom:.2} crosses divider {divider_y:.2}"
            );
        }

        let first_body_top = body[0] - ascent;
        assert!(
            first_body_top >= divider_y - 0.01,
            "{ctx}: first body glyph top {first_body_top:.2} above divider {divider_y:.2}"
        );

        let last_body_bottom = body[body.len() - 1] - ascent + font_size;
        assert!(
            last_body_bottom <= node.y + node.height + 0.01,
            "{ctx}: last body glyph bottom {last_body_bottom:.2} below box bottom {:.2}",
            node.y + node.height
        );
    }

    // Colors applied: box stroke and outer stroke present alongside the
    // matched fill/divider/label colors above.
    assert!(
        svg.contains(&format!("stroke=\"{}\"", req.box_stroke)),
        "{ctx}: box stroke color not applied"
    );
    assert!(
        svg.contains(&format!("stroke=\"{}\"", req.stroke)),
        "{ctx}: requirement outer stroke color not applied"
    );
}

fn themes() -> Vec<(&'static str, Theme)> {
    vec![
        ("default", Theme::mermaid_default()),
        ("dark", Theme::dark()),
        ("forest", Theme::forest()),
        ("neutral", Theme::neutral()),
        ("modern", Theme::modern()),
    ]
}

#[test]
fn requirement_header_fits_band_under_all_themes_and_font_sizes() {
    for (theme_name, theme) in themes() {
        for font_size in [None, Some(24.0), Some(32.0)] {
            run_case(&Case {
                theme_name,
                theme: theme.clone(),
                font_size,
                header_band_height: None,
                header_line_gap: None,
                label_padding_y: None,
            });
        }
    }
}

#[test]
fn requirement_header_fits_band_with_layout_overrides() {
    let overrides: Vec<(Option<f32>, Option<f32>, Option<f32>)> = vec![
        // Small explicit band must still be pushed below the header text.
        (Some(30.0), None, None),
        // Large explicit band is respected as-is.
        (Some(120.0), None, None),
        // Wide header gap grows the band.
        (None, Some(60.0), None),
        // Extra padding grows the band.
        (None, None, Some(14.0)),
        // Everything at once with a large font.
        (Some(30.0), Some(60.0), Some(14.0)),
    ];
    for (theme_name, theme) in themes() {
        for font_size in [None, Some(32.0)] {
            for (band, gap, pad) in &overrides {
                run_case(&Case {
                    theme_name,
                    theme: theme.clone(),
                    font_size,
                    header_band_height: *band,
                    header_line_gap: *gap,
                    label_padding_y: *pad,
                });
            }
        }
    }
}
