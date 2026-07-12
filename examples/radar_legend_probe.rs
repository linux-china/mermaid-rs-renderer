//! Verification probe for radar legend overflow/clipping/overlap at scale.
//! Renders synthetic fixtures and checks legend bboxes against the viewBox
//! and against curve polygon geometry. Prints exact measurements.

use mermaid_rs_renderer::render;

#[derive(Debug, Clone, Copy)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let pat = format!("{}=\"", name);
    let start = tag.find(&pat)? + pat.len();
    let end = tag[start..].find('"')? + start;
    Some(tag[start..end].to_string())
}

fn attr_f(tag: &str, name: &str) -> Option<f32> {
    attr(tag, name)?.parse().ok()
}

/// Extract all tags of a given element name (self-closing or with content) as raw strings.
fn tags<'a>(svg: &'a str, elem: &str) -> Vec<&'a str> {
    let open = format!("<{}", elem);
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(i) = svg[pos..].find(&open) {
        let s = pos + i;
        // ensure next char is space or '>'
        let after = &svg[s + open.len()..];
        if !after.starts_with(' ') && !after.starts_with('>') {
            pos = s + open.len();
            continue;
        }
        let e = svg[s..].find('>').map(|j| s + j + 1).unwrap_or(svg.len());
        out.push(&svg[s..e]);
        pos = e;
    }
    out
}

fn viewbox(svg: &str) -> (f32, f32, f32, f32) {
    let vb = attr(svg, "viewBox").expect("viewBox");
    let v: Vec<f32> = vb.split_whitespace().map(|t| t.parse().unwrap()).collect();
    (v[0], v[1], v[2], v[3])
}

/// Translate offset applied to the radar group.
fn radar_translate(svg: &str) -> (f32, f32) {
    let pat = "transform=\"translate(";
    let s = svg.find(pat).expect("translate") + pat.len();
    let e = svg[s..].find(')').unwrap() + s;
    let parts: Vec<f32> = svg[s..e]
        .split(',')
        .map(|t| t.trim().parse().unwrap())
        .collect();
    (parts[0], parts[1])
}

/// Estimate text width: font-size 12 legend text, average glyph ~0.6em
/// (matches the crate's fallback width heuristic for latin text).
fn est_text_width(text: &str, font_size: f32) -> f32 {
    text.chars().count() as f32 * font_size * 0.6
}

struct LegendRow {
    name: String,
    color: String,
    box_rect: Rect,  // absolute coords
    text_bbox: Rect, // absolute coords, estimated width
}

fn parse_legend(svg: &str) -> Vec<LegendRow> {
    let (tx, ty) = radar_translate(svg);
    let rects: Vec<&str> = tags(svg, "rect")
        .into_iter()
        .filter(|t| {
            attr(t, "fill-opacity").as_deref() == Some("0.5")
                && attr(t, "width").as_deref() == Some("12")
        })
        .collect();
    let texts: Vec<&str> = tags(svg, "text")
        .into_iter()
        .filter(|t| {
            attr(t, "dominant-baseline").as_deref() == Some("hanging")
                && attr(t, "text-anchor").as_deref() == Some("start")
        })
        .collect();
    let mut rows = Vec::new();
    for (r, t) in rects.iter().zip(texts.iter()) {
        let bx = attr_f(r, "x").unwrap() + tx;
        let by = attr_f(r, "y").unwrap() + ty;
        let bw = attr_f(r, "width").unwrap();
        let bh = attr_f(r, "height").unwrap();
        let tx0 = attr_f(t, "x").unwrap() + tx;
        let ty0 = attr_f(t, "y").unwrap() + ty;
        // text content sits after '>' of open tag in full svg; find it
        let open_end = svg.find(t).unwrap() + t.len();
        let close = svg[open_end..].find("</text>").unwrap() + open_end;
        let name = svg[open_end..close].to_string();
        let fs: f32 = attr_f(t, "font-size").unwrap_or(12.0);
        rows.push(LegendRow {
            color: attr(r, "fill").unwrap_or_default(),
            box_rect: Rect {
                x: bx,
                y: by,
                w: bw,
                h: bh,
            },
            text_bbox: Rect {
                x: tx0,
                y: ty0,
                w: est_text_width(&name, fs),
                h: fs,
            },
            name,
        });
    }
    rows
}

/// Parse curve polygon points (absolute coords) from <path d="M... L... Z"> with fill-opacity 0.5.
fn parse_curves(svg: &str) -> Vec<Vec<(f32, f32)>> {
    let (tx, ty) = radar_translate(svg);
    tags(svg, "path")
        .into_iter()
        .filter(|t| attr(t, "fill-opacity").as_deref() == Some("0.5"))
        .filter_map(|t| attr(t, "d"))
        .map(|d| {
            d.trim_end_matches(" Z")
                .replace(['M', 'L'], "")
                .split_whitespace()
                .map(|p| {
                    let mut it = p.split(',');
                    let x: f32 = it.next().unwrap().parse().unwrap();
                    let y: f32 = it.next().unwrap().parse().unwrap();
                    (x + tx, y + ty)
                })
                .collect()
        })
        .collect()
}

fn seg_intersects_rect(a: (f32, f32), b: (f32, f32), r: &Rect) -> bool {
    // Trivial accept if either endpoint inside
    let inside = |p: (f32, f32)| p.0 >= r.x && p.0 <= r.x + r.w && p.1 >= r.y && p.1 <= r.y + r.h;
    if inside(a) || inside(b) {
        return true;
    }
    // Check segment against each rect edge
    let edges = [
        ((r.x, r.y), (r.x + r.w, r.y)),
        ((r.x + r.w, r.y), (r.x + r.w, r.y + r.h)),
        ((r.x + r.w, r.y + r.h), (r.x, r.y + r.h)),
        ((r.x, r.y + r.h), (r.x, r.y)),
    ];
    edges.iter().any(|&(c, d)| segs_cross(a, b, c, d))
}

fn segs_cross(p1: (f32, f32), p2: (f32, f32), p3: (f32, f32), p4: (f32, f32)) -> bool {
    let d = |a: (f32, f32), b: (f32, f32), c: (f32, f32)| {
        (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
    };
    let d1 = d(p3, p4, p1);
    let d2 = d(p3, p4, p2);
    let d3 = d(p1, p2, p3);
    let d4 = d(p1, p2, p4);
    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}

fn point_in_poly(p: (f32, f32), poly: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if ((yi > p.1) != (yj > p.1)) && (p.0 < (xj - xi) * (p.1 - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn analyze(label: &str, source: &str) {
    println!("\n=== {} ===", label);
    let svg = render(source).expect("render ok");
    let (vx, vy, vw, vh) = viewbox(&svg);
    println!("viewBox: {} {} {} {}", vx, vy, vw, vh);
    let legend = parse_legend(&svg);
    let curves = parse_curves(&svg);
    println!(
        "legend rows: {}, curve polys: {}",
        legend.len(),
        curves.len()
    );

    // 1. Vertical overflow
    let mut overflow_rows = 0;
    for (i, row) in legend.iter().enumerate() {
        let bottom = row.box_rect.y + row.box_rect.h;
        let tbottom = row.text_bbox.y + row.text_bbox.h;
        let over = bottom.max(tbottom) - (vy + vh);
        if over > 0.0 {
            overflow_rows += 1;
            println!(
                "  ROW {} '{}' overflows bottom by {:.1}px (row bottom {:.1} > viewBox bottom {:.1})",
                i,
                row.name,
                over,
                bottom.max(tbottom),
                vy + vh
            );
        }
    }
    if overflow_rows == 0 && !legend.is_empty() {
        let last = legend.last().unwrap();
        println!(
            "  no vertical overflow; last row bottom {:.1} vs viewBox bottom {:.1} (headroom {:.1}px, {:.0} more rows fit)",
            last.box_rect.y + last.box_rect.h,
            vy + vh,
            (vy + vh) - (last.box_rect.y + last.box_rect.h),
            ((vy + vh) - (last.box_rect.y + last.box_rect.h)) / (last.text_bbox.h + 10.0).max(1.0)
        );
    }

    // 2. Horizontal clipping of names (estimated width)
    for (i, row) in legend.iter().enumerate() {
        let right = row.text_bbox.x + row.text_bbox.w;
        let over = right - (vx + vw);
        if over > 0.0 {
            println!(
                "  ROW {} '{}' est. clips right edge by {:.1}px (text {:.1}..{:.1} vs viewBox right {:.1})",
                i,
                row.name,
                over,
                row.text_bbox.x,
                right,
                vx + vw
            );
        }
    }
    if !legend.is_empty() {
        let room = (vx + vw) - legend[0].text_bbox.x;
        println!(
            "  text room: {:.1}px from text-x {:.1} to right edge (~{:.0} chars at 12px)",
            room,
            legend[0].text_bbox.x,
            room / 7.2
        );
    }

    // 3. Duplicate colors
    let mut seen: Vec<&str> = Vec::new();
    let mut dups = 0;
    for row in &legend {
        if seen.contains(&row.color.as_str()) {
            dups += 1;
        } else {
            seen.push(&row.color);
        }
    }
    println!(
        "  distinct colors: {} of {} rows ({} duplicates)",
        seen.len(),
        legend.len(),
        dups
    );

    // 4. Curve geometry overlapping legend rows
    for (ci, poly) in curves.iter().enumerate() {
        for (ri, row) in legend.iter().enumerate() {
            // union bbox of box + estimated text
            let bb = Rect {
                x: row.box_rect.x,
                y: row.box_rect.y.min(row.text_bbox.y),
                w: (row.text_bbox.x + row.text_bbox.w) - row.box_rect.x,
                h: row.box_rect.h.max(row.text_bbox.h),
            };
            let mut hit = false;
            let n = poly.len();
            for i in 0..n {
                if seg_intersects_rect(poly[i], poly[(i + 1) % n], &bb) {
                    hit = true;
                    break;
                }
            }
            // fill overlap: legend bbox center inside polygon
            let center = (bb.x + bb.w / 2.0, bb.y + bb.h / 2.0);
            let fill_hit = point_in_poly(center, poly);
            if hit || fill_hit {
                println!(
                    "  OVERLAP curve {} vs legend row {} '{}' (stroke_cross={}, under_fill={}) legend bbox x={:.0} y={:.0} w={:.0} h={:.0}",
                    ci, ri, row.name, hit, fill_hit, bb.x, bb.y, bb.w, bb.h
                );
            }
        }
    }
    // max curve extent
    let (mut maxx, mut miny) = (f32::MIN, f32::MAX);
    for poly in &curves {
        for &(x, y) in poly {
            maxx = maxx.max(x);
            miny = miny.min(y);
        }
    }
    if !curves.is_empty() {
        println!(
            "  curve extent: max x {:.1}, min y {:.1} (viewBox right {:.1}, top {:.1})",
            maxx,
            miny,
            vx + vw,
            vy
        );
    }
}

fn many_curves(n: usize) -> String {
    let mut s = String::from("radar-beta\n  axis A, B, C, D, E\n");
    for i in 0..n {
        s.push_str(&format!(
            "  curve Curve{:02} {{{},{},{},{},{}}}\n",
            i,
            1 + i % 5,
            2 + i % 4,
            3 + i % 3,
            1 + i % 4,
            2 + i % 5
        ));
    }
    s
}

fn main() {
    analyze(
        "baseline 3 curves",
        "radar-beta\n  axis Speed, Quality, Cost, Support, Docs\n  curve Alpha {4,3,4,2,5}\n  curve Beta {3,4,3,4,3}\n  curve Gamma {5,2,5,3,4}\n",
    );

    analyze("14 curves", &many_curves(14));
    analyze("26 curves", &many_curves(26));
    analyze("27 curves", &many_curves(27));
    analyze("40 curves", &many_curves(40));

    analyze(
        "long names",
        "radar-beta\n  axis A, B, C\n  curve Short {1,2,3}\n  curve Enterprise Platform Engineering Team {3,2,1}\n  curve Customer Success And Support Organization {2,3,2}\n",
    );

    // Upper-right pressure: 8 axes so one axis points at -45deg (NE), max values.
    analyze(
        "upper-right overlap (8 axes max values)",
        "radar-beta\n  axis N, NE, E, SE, S, SW, W, NW\n  curve Full {5,5,5,5,5,5,5,5}\n  curve Big {4,5,4,3,3,3,3,4}\n",
    );
}
