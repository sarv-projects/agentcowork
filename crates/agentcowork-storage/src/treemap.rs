//! Squarified treemap layout (Bruls–van Wijk) + per-directory aggregation +
//! stable extension-hash colors (eDirStat/WinDirStat treemap, doc 49 §3).
//!
//! The squarified algorithm keeps every cell's aspect ratio as close to 1:1
//! as possible (much more readable than the slice-and-dice alternative).

use std::cmp::Ordering;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::walk::Arena;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct TreemapRect {
    pub id: u32,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, Copy)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

/// Lay out `items` (`(id, weight)`) inside `(x, y, w, h)`.
pub fn squarify(items: &[(u32, f64)], x: f64, y: f64, w: f64, h: f64) -> Vec<TreemapRect> {
    let mut items: Vec<(u32, f64)> = items
        .iter()
        .copied()
        .filter(|(_, s)| *s > 0.0 && s.is_finite())
        .collect();
    items.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));

    let mut out = Vec::new();
    squarify_recurse(&items, Rect { x, y, w, h }, &mut out);
    out
}

fn squarify_recurse(items: &[(u32, f64)], rect: Rect, out: &mut Vec<TreemapRect>) {
    if items.is_empty() || rect.w <= 0.0 || rect.h <= 0.0 {
        return;
    }
    let total: f64 = items.iter().map(|(_, a)| a).sum();
    if total <= 0.0 {
        return;
    }

    // Geometry constant for this rectangle (see `worst`): the row spans the
    // long side L and has thickness `S * row_sum/total` on the short side S.
    let long = rect.w.max(rect.h);
    let short = rect.w.min(rect.h);
    let k = long * total / short;

    let mut row: Vec<(u32, f64)> = vec![items[0]];
    let mut i = 1;

    while i < items.len() {
        let mut row_with = row.clone();
        row_with.push(items[i]);
        let cur = worst(&row.iter().map(|(_, a)| *a).collect::<Vec<_>>(), k);
        let nxt = worst(&row_with.iter().map(|(_, a)| *a).collect::<Vec<_>>(), k);
        if nxt <= cur {
            row.push(items[i]);
            i += 1;
        } else {
            break;
        }
    }

    let row_sum: f64 = row.iter().map(|(_, a)| a).sum();
    let remaining = layout_row(&row, row_sum, total, rect, out);
    squarify_recurse(&items[i..], remaining, out);
}

/// Worst aspect ratio of a candidate row (lower is better). The item with
/// area `a` in a row of total `s` laid over a strip of geometry constant `k`
/// has aspect ratio `max(k·a/s², s²/(k·a))`.
fn worst(row: &[f64], k: f64) -> f64 {
    let s: f64 = row.iter().sum();
    if s <= 0.0 {
        return f64::INFINITY;
    }
    let s2 = s * s;
    let mut worst = f64::NEG_INFINITY;
    for &a in row {
        let r = k * a / s2;
        let r = if r > 0.0 {
            r.max(1.0 / r)
        } else {
            f64::INFINITY
        };
        worst = worst.max(r);
    }
    worst
}

fn layout_row(
    row: &[(u32, f64)],
    row_sum: f64,
    total: f64,
    rect: Rect,
    out: &mut Vec<TreemapRect>,
) -> Rect {
    if rect.w >= rect.h {
        // Horizontal strip spanning full width.
        let th = rect.h * (row_sum / total);
        let mut cx = rect.x;
        for (id, a) in row {
            let cw = rect.w * (a / row_sum);
            out.push(TreemapRect {
                id: *id,
                x: cx,
                y: rect.y,
                w: cw,
                h: th,
            });
            cx += cw;
        }
        Rect {
            x: rect.x,
            y: rect.y + th,
            w: rect.w,
            h: rect.h - th,
        }
    } else {
        // Vertical strip spanning full height.
        let tw = rect.w * (row_sum / total);
        let mut cy = rect.y;
        for (id, a) in row {
            let ch = rect.h * (a / row_sum);
            out.push(TreemapRect {
                id: *id,
                x: rect.x,
                y: cy,
                w: tw,
                h: ch,
            });
            cy += ch;
        }
        Rect {
            x: rect.x + tw,
            y: rect.y,
            w: rect.w - tw,
            h: rect.h,
        }
    }
}

/// Lay out a directory's children as a unit-square treemap.
pub fn treemap_for_dir(arena: &Arena, dir_id: u32) -> Vec<TreemapRect> {
    let children = arena.children(dir_id);
    let items: Vec<(u32, f64)> = children
        .iter()
        .filter_map(|&id| arena.get(id).map(|n| (id, n.size as f64)))
        .collect();
    squarify(&items, 0.0, 0.0, 1.0, 1.0)
}

/// Stable per-extension color (xxHash3 → hue; identical across runs).
pub fn color_for(name: &str) -> [u8; 3] {
    let ext = Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let h = crate::xxh3(ext.to_lowercase().as_bytes());
    let hue = (h % 360) as f32 / 360.0;
    hsl_to_rgb(hue, 0.5, 0.55)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [u8; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h * 6.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h * 6.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squarify_conserves_and_allocates_area() {
        let items = vec![(1u32, 6.0), (2, 3.0), (3, 1.0)];
        let rects = squarify(&items, 0.0, 0.0, 10.0, 10.0);
        assert_eq!(rects.len(), 3);

        let total_area: f64 = rects.iter().map(|r| r.w * r.h).sum();
        assert!((total_area - 100.0).abs() < 1e-6);

        // Proportional: id=1 → 60, id=2 → 30, id=3 → 10.
        let area = |id: u32| {
            rects
                .iter()
                .find(|r| r.id == id)
                .map(|r| r.w * r.h)
                .unwrap()
        };
        assert!((area(1) - 60.0).abs() < 1e-6);
        assert!((area(2) - 30.0).abs() < 1e-6);
        assert!((area(3) - 10.0).abs() < 1e-6);

        // No cell has a degenerate aspect ratio.
        for r in &rects {
            let ar = r.w.max(r.h) / r.w.min(r.h);
            assert!(ar < 4.0, "aspect ratio {ar} too extreme");
        }
    }

    #[test]
    fn colors_are_stable_by_extension() {
        assert_eq!(color_for("a.txt"), color_for("b.txt"));
        assert_eq!(color_for("a.txt"), color_for("A.TXT"));
        assert_ne!(color_for("a.txt"), color_for("a.rs"));
    }
}
