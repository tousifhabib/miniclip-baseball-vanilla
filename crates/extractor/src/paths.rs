//! Turns Flash's edge lists into outlines.
//!
//! Flash does not store a shape as a list of outlines. It stores edges, and
//! each edge names the fill on its left, the fill on its right and its stroke.
//! This module regroups those edges into paths: one set per fill and one per
//! stroke, which is what SVG and every other vector format expects.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;

/// A point in twips (twentieths of a pixel).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Pt {
    pub x: i32,
    pub y: i32,
}

impl Pt {
    pub const ZERO: Pt = Pt { x: 0, y: 0 };

    pub fn offset(self, dx: i32, dy: i32) -> Pt {
        Pt {
            x: self.x + dx,
            y: self.y + dy,
        }
    }
}

/// A straight line, or a quadratic curve when `ctrl` is set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub from: Pt,
    pub ctrl: Option<Pt>,
    pub to: Pt,
}

impl Edge {
    fn reversed(self) -> Edge {
        Edge {
            from: self.to,
            ctrl: self.ctrl,
            to: self.from,
        }
    }
}

/// One edge of a shape. Morph shapes carry the edge at both ends of the blend;
/// plain shapes use the same edge for `start` and `end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seg {
    pub start: Edge,
    pub end: Edge,
}

impl Seg {
    pub fn plain(edge: Edge) -> Seg {
        Seg {
            start: edge,
            end: edge,
        }
    }

    fn reversed(self) -> Seg {
        Seg {
            start: self.start.reversed(),
            end: self.end.reversed(),
        }
    }
}

/// A run of connected edges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contour {
    pub segs: Vec<Seg>,
    pub closed: bool,
}

/// Everything drawn with one set of styles. Later layers draw on top of
/// earlier ones; within a layer, strokes draw on top of fills.
///
/// Style numbers start at 1, as in the file: style `n` is entry `n - 1` of the
/// layer's style list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layer {
    pub fills: Vec<(u32, Vec<Contour>)>,
    pub strokes: Vec<(u32, Vec<Contour>)>,
}

#[derive(Default)]
struct LayerEdges {
    fills: BTreeMap<u32, Vec<Seg>>,
    strokes: BTreeMap<u32, Vec<Seg>>,
}

/// Collects edges as a shape is read and groups them into [`Layer`]s.
#[derive(Default)]
pub struct Outliner {
    /// Fill on the left of the edges that follow; 0 for none.
    pub fill0: u32,
    /// Fill on the right of the edges that follow; 0 for none.
    pub fill1: u32,
    /// Stroke along the edges that follow; 0 for none.
    pub line: u32,
    current: LayerEdges,
    done: Vec<Layer>,
}

impl Outliner {
    pub fn new() -> Outliner {
        Outliner::default()
    }

    pub fn edge(&mut self, seg: Seg) {
        // An edge with the same fill on both sides is inside that fill, not on
        // its boundary.
        if self.fill0 != self.fill1 {
            if self.fill1 != 0 {
                self.current.fills.entry(self.fill1).or_default().push(seg);
            }
            if self.fill0 != 0 {
                self.current
                    .fills
                    .entry(self.fill0)
                    .or_default()
                    .push(seg.reversed());
            }
        }
        if self.line != 0 {
            self.current.strokes.entry(self.line).or_default().push(seg);
        }
    }

    /// Ends the current layer. Call when the shape switches to a new style
    /// list.
    pub fn new_layer(&mut self) {
        let edges = std::mem::take(&mut self.current);
        self.done.push(Layer {
            fills: edges
                .fills
                .into_iter()
                .map(|(style, segs)| (style, chain(segs, false)))
                .collect(),
            strokes: edges
                .strokes
                .into_iter()
                .map(|(style, segs)| (style, chain(segs, true)))
                .collect(),
        });
    }

    pub fn finish(mut self) -> Vec<Layer> {
        self.new_layer();
        self.done
    }
}

/// Links edges end to start into contours.
///
/// For fills the result is a set of closed loops. Which loop an edge lands in
/// can vary where loops touch, but under the even-odd rule the filled area
/// depends only on the set of edges, so any grouping draws the same.
///
/// For strokes, `open_ends_first` starts each contour at an edge nothing leads
/// into, so an open line comes out as one piece instead of several.
fn chain(segs: Vec<Seg>, open_ends_first: bool) -> Vec<Contour> {
    let mut starting_at: HashMap<Pt, Vec<usize>> = HashMap::new();
    // Reversed, so that popping yields the earliest edge first.
    for (i, seg) in segs.iter().enumerate().rev() {
        starting_at.entry(seg.start.from).or_default().push(i);
    }

    let mut order: Vec<usize> = (0..segs.len()).collect();
    if open_ends_first {
        let mut arrivals: HashMap<Pt, usize> = HashMap::new();
        for seg in &segs {
            *arrivals.entry(seg.start.to).or_default() += 1;
        }
        // Stable, so edges keep their original order within each group.
        order.sort_by_key(|&i| arrivals.contains_key(&segs[i].start.from));
    }

    let mut used = vec![false; segs.len()];
    let mut contours = Vec::new();
    for first in order {
        if used[first] {
            continue;
        }
        used[first] = true;
        let origin = segs[first].start.from;
        let mut at = segs[first].start.to;
        let mut contour = vec![segs[first]];
        while at != origin {
            let next = starting_at.get_mut(&at).and_then(|candidates| {
                while let Some(i) = candidates.pop() {
                    if !used[i] {
                        return Some(i);
                    }
                }
                None
            });
            let Some(next) = next else { break };
            used[next] = true;
            at = segs[next].start.to;
            contour.push(segs[next]);
        }
        contours.push(Contour {
            segs: contour,
            closed: at == origin,
        });
    }
    contours
}

/// Which end of a morph to write. Plain shapes are the same at both.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Which {
    Start,
    End,
}

/// Writes contours as SVG path data, dividing coordinates by `units_per_px`.
///
/// With `curves_only`, straight edges are written as curves too, so that the
/// two ends of a morph always have matching commands.
pub fn path_data(
    contours: &[Contour],
    which: Which,
    units_per_px: i32,
    curves_only: bool,
) -> String {
    let pick = |seg: &Seg| match which {
        Which::Start => seg.start,
        Which::End => seg.end,
    };
    let num = |v: i32| f64::from(v) / f64::from(units_per_px);
    let mut d = String::new();
    for contour in contours {
        let Some(first) = contour.segs.first() else {
            continue;
        };
        if !d.is_empty() {
            d.push(' ');
        }
        let from = pick(first).from;
        write!(d, "M{} {}", num(from.x), num(from.y)).unwrap();
        for seg in &contour.segs {
            let edge = pick(seg);
            let ctrl = match edge.ctrl {
                Some(ctrl) => Some((num(ctrl.x), num(ctrl.y))),
                None if curves_only => Some((
                    (num(edge.from.x) + num(edge.to.x)) / 2.0,
                    (num(edge.from.y) + num(edge.to.y)) / 2.0,
                )),
                None => None,
            };
            match ctrl {
                Some((cx, cy)) => {
                    write!(d, " Q{cx} {cy} {} {}", num(edge.to.x), num(edge.to.y)).unwrap()
                }
                None => write!(d, " L{} {}", num(edge.to.x), num(edge.to.y)).unwrap(),
            }
        }
        if contour.closed {
            d.push_str(" Z");
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: i32, y: i32) -> Pt {
        Pt { x, y }
    }

    /// Feeds a polyline through the outliner, one straight edge per pair.
    fn draw(outliner: &mut Outliner, points: &[(i32, i32)]) {
        for pair in points.windows(2) {
            outliner.edge(Seg::plain(Edge {
                from: pt(pair[0].0, pair[0].1),
                ctrl: None,
                to: pt(pair[1].0, pair[1].1),
            }));
        }
    }

    fn fill(layer: &Layer, style: u32) -> &[Contour] {
        &layer.fills.iter().find(|(s, _)| *s == style).unwrap().1
    }

    #[test]
    fn a_square_on_the_right_fill_is_one_closed_contour() {
        let mut o = Outliner::new();
        o.fill1 = 1;
        draw(&mut o, &[(0, 0), (20, 0), (20, 20), (0, 20), (0, 0)]);
        let layers = o.finish();

        let contours = fill(&layers[0], 1);
        assert_eq!(contours.len(), 1);
        assert!(contours[0].closed);
        assert_eq!(
            path_data(contours, Which::Start, 20, false),
            "M0 0 L1 0 L1 1 L0 1 L0 0 Z"
        );
    }

    #[test]
    fn a_left_fill_gets_its_edges_reversed() {
        let mut o = Outliner::new();
        o.fill0 = 1;
        draw(&mut o, &[(0, 0), (20, 0), (20, 20), (0, 0)]);
        let layers = o.finish();

        let contours = fill(&layers[0], 1);
        assert_eq!(contours.len(), 1);
        assert!(contours[0].closed);
        // Every edge runs backwards, so each one ends where the file's began.
        assert_eq!(contours[0].segs[0].start.from, pt(20, 0));
        assert_eq!(contours[0].segs[0].start.to, pt(0, 0));
    }

    #[test]
    fn two_fills_sharing_an_edge_each_close() {
        // Two unit squares side by side. The shared edge x = 20 is drawn once,
        // with fill 1 on one side and fill 2 on the other.
        let mut o = Outliner::new();
        o.fill1 = 1;
        draw(&mut o, &[(20, 20), (0, 20), (0, 0), (20, 0)]);
        o.fill0 = 2;
        draw(&mut o, &[(20, 0), (20, 20)]);
        o.fill1 = 0;
        draw(&mut o, &[(20, 20), (40, 20), (40, 0), (20, 0)]);
        let layers = o.finish();

        for style in [1, 2] {
            let contours = fill(&layers[0], style);
            assert_eq!(contours.len(), 1, "fill {style}");
            assert!(contours[0].closed, "fill {style}");
            assert_eq!(contours[0].segs.len(), 4, "fill {style}");
        }
    }

    #[test]
    fn an_edge_with_the_same_fill_on_both_sides_is_dropped() {
        let mut o = Outliner::new();
        o.fill0 = 1;
        o.fill1 = 1;
        draw(&mut o, &[(0, 0), (20, 0)]);
        let layers = o.finish();
        assert!(layers[0].fills.is_empty());
    }

    #[test]
    fn stroke_pieces_drawn_out_of_order_join_into_one_line() {
        let mut o = Outliner::new();
        o.line = 1;
        draw(&mut o, &[(20, 0), (40, 0)]);
        draw(&mut o, &[(0, 0), (20, 0)]);
        let layers = o.finish();

        let contours = &layers[0].strokes[0].1;
        assert_eq!(contours.len(), 1);
        assert!(!contours[0].closed);
        assert_eq!(
            path_data(contours, Which::Start, 20, false),
            "M0 0 L1 0 L2 0"
        );
    }

    #[test]
    fn a_new_style_list_starts_a_new_layer() {
        let mut o = Outliner::new();
        o.fill1 = 1;
        draw(&mut o, &[(0, 0), (20, 0), (20, 20), (0, 0)]);
        o.new_layer();
        draw(&mut o, &[(0, 0), (40, 0), (40, 40), (0, 0)]);
        let layers = o.finish();

        assert_eq!(layers.len(), 2);
        assert_eq!(fill(&layers[0], 1)[0].segs[0].start.to, pt(20, 0));
        assert_eq!(fill(&layers[1], 1)[0].segs[0].start.to, pt(40, 0));
    }

    #[test]
    fn morph_paths_use_curves_at_both_ends() {
        let straight = Edge {
            from: pt(0, 0),
            ctrl: None,
            to: pt(40, 0),
        };
        let curved = Edge {
            from: pt(0, 0),
            ctrl: Some(pt(20, 20)),
            to: pt(40, 0),
        };
        let contours = [Contour {
            segs: vec![Seg {
                start: straight,
                end: curved,
            }],
            closed: false,
        }];
        assert_eq!(
            path_data(&contours, Which::Start, 20, true),
            "M0 0 Q1 0 2 0"
        );
        assert_eq!(path_data(&contours, Which::End, 20, true), "M0 0 Q1 1 2 0");
    }
}
