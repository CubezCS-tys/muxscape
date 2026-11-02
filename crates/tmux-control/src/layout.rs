//! tmux window layouts.
//!
//! A layout string looks like `b25d,80x24,0,0{40x24,0,0,1,39x24,41,0,2}`:
//! a 16-bit checksum followed by a cell tree. A cell is `WxH,X,Y` followed
//! by either `,<pane-number>` (a leaf), `{...}` (children side by side,
//! left→right) or `[...]` (children stacked, top→bottom). Sibling cells are
//! separated by a one-cell border, which is exactly what the GUI draws as a
//! draggable divider.

use crate::ids::PaneId;
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutCell {
    pub width: u32,
    pub height: u32,
    pub x: u32,
    pub y: u32,
    #[serde(flatten)]
    pub kind: CellKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "lowercase")]
pub enum CellKind {
    Pane(PaneId),
    /// `{...}`: children laid out left to right.
    Horizontal(Vec<LayoutCell>),
    /// `[...]`: children laid out top to bottom.
    Vertical(Vec<LayoutCell>),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LayoutError {
    #[error("layout parse error at byte {0}")]
    Syntax(usize),
    #[error("layout checksum mismatch (expected {expected:04x}, got {actual:04x})")]
    Checksum { expected: u16, actual: u16 },
}

/// The checksum algorithm from tmux layout-custom.c.
pub fn checksum(body: &str) -> u16 {
    let mut csum: u16 = 0;
    for b in body.bytes() {
        csum = (csum >> 1) | ((csum & 1) << 15);
        csum = csum.wrapping_add(b as u16);
    }
    csum
}

impl LayoutCell {
    /// Parse a full layout string including the checksum prefix.
    pub fn parse(s: &str) -> Result<LayoutCell, LayoutError> {
        let (sum, body) = s.split_once(',').ok_or(LayoutError::Syntax(0))?;
        if sum.len() != 4 {
            return Err(LayoutError::Syntax(0));
        }
        let expected = u16::from_str_radix(sum, 16).map_err(|_| LayoutError::Syntax(0))?;
        let actual = checksum(body);
        if expected != actual {
            return Err(LayoutError::Checksum { expected, actual });
        }
        let mut p = Cursor { s: body.as_bytes(), pos: 0, base: sum.len() + 1 };
        let cell = p.cell()?;
        if p.pos != p.s.len() {
            return Err(LayoutError::Syntax(p.base + p.pos));
        }
        Ok(cell)
    }

    /// Serialize the tree body (no checksum).
    pub fn body(&self) -> String {
        let mut s = String::new();
        self.write_body(&mut s);
        s
    }

    fn write_body(&self, s: &mut String) {
        let _ = write!(s, "{}x{},{},{}", self.width, self.height, self.x, self.y);
        match &self.kind {
            CellKind::Pane(p) => {
                let _ = write!(s, ",{}", p.0);
            }
            CellKind::Horizontal(c) | CellKind::Vertical(c) => {
                let (open, close) = if matches!(self.kind, CellKind::Horizontal(_)) {
                    ('{', '}')
                } else {
                    ('[', ']')
                };
                s.push(open);
                for (i, child) in c.iter().enumerate() {
                    if i > 0 {
                        s.push(',');
                    }
                    child.write_body(s);
                }
                s.push(close);
            }
        }
    }

    /// Serialize as a complete layout string accepted by `select-layout`.
    pub fn to_layout_string(&self) -> String {
        let body = self.body();
        format!("{:04x},{}", checksum(&body), body)
    }

    /// Leaf panes in layout order.
    pub fn panes(&self) -> Vec<PaneId> {
        let mut v = Vec::new();
        self.walk(&mut |c| {
            if let CellKind::Pane(p) = c.kind {
                v.push(p);
            }
        });
        v
    }

    pub fn find_pane(&self, pane: PaneId) -> Option<&LayoutCell> {
        match &self.kind {
            CellKind::Pane(p) if *p == pane => Some(self),
            CellKind::Pane(_) => None,
            CellKind::Horizontal(c) | CellKind::Vertical(c) => {
                c.iter().find_map(|ch| ch.find_pane(pane))
            }
        }
    }

    fn walk(&self, f: &mut impl FnMut(&LayoutCell)) {
        f(self);
        if let CellKind::Horizontal(c) | CellKind::Vertical(c) = &self.kind {
            for ch in c {
                ch.walk(f);
            }
        }
    }

    pub fn children(&self) -> &[LayoutCell] {
        match &self.kind {
            CellKind::Pane(_) => &[],
            CellKind::Horizontal(c) | CellKind::Vertical(c) => c,
        }
    }

    fn node_at_mut(&mut self, path: &[usize]) -> Option<&mut LayoutCell> {
        let mut cur = self;
        for &i in path {
            cur = match &mut cur.kind {
                CellKind::Pane(_) => return None,
                CellKind::Horizontal(c) | CellKind::Vertical(c) => c.get_mut(i)?,
            };
        }
        Some(cur)
    }

    /// Smallest size this subtree can take along `axis`.
    fn min_size(&self, axis: Axis) -> u32 {
        match (&self.kind, axis) {
            (CellKind::Pane(_), _) => 1,
            (CellKind::Horizontal(c), Axis::X) | (CellKind::Vertical(c), Axis::Y) => {
                c.iter().map(|ch| ch.min_size(axis)).sum::<u32>() + (c.len() as u32 - 1)
            }
            (CellKind::Horizontal(c), Axis::Y) | (CellKind::Vertical(c), Axis::X) => {
                c.iter().map(|ch| ch.min_size(axis)).max().unwrap_or(1)
            }
        }
    }

    fn size(&self, axis: Axis) -> u32 {
        match axis {
            Axis::X => self.width,
            Axis::Y => self.height,
        }
    }

    fn set_size(&mut self, axis: Axis, v: u32) {
        match axis {
            Axis::X => self.width = v,
            Axis::Y => self.height = v,
        }
    }

    /// Resize this subtree along `axis` to `new`. When the subtree is split
    /// along the same axis, the change is absorbed by the children nearest
    /// the moving edge (the end edge if `at_end`, else the start edge),
    /// spilling over to further children only once those hit their minimum —
    /// this mirrors how dragging a divider feels in any desktop app.
    fn resize_along(&mut self, axis: Axis, new: u32, at_end: bool) {
        let old = self.size(axis);
        self.set_size(axis, new);
        let same_axis = matches!(
            (&self.kind, axis),
            (CellKind::Horizontal(_), Axis::X) | (CellKind::Vertical(_), Axis::Y)
        );
        let children = match &mut self.kind {
            CellKind::Pane(_) => return,
            CellKind::Horizontal(c) | CellKind::Vertical(c) => c,
        };
        if !same_axis {
            for ch in children.iter_mut() {
                ch.resize_along(axis, new, at_end);
            }
            return;
        }
        let order: Vec<usize> = if at_end {
            (0..children.len()).rev().collect()
        } else {
            (0..children.len()).collect()
        };
        if new >= old {
            let i = order[0];
            let s = children[i].size(axis);
            children[i].resize_along(axis, s + (new - old), at_end);
        } else {
            let mut remaining = old - new;
            for i in order {
                if remaining == 0 {
                    break;
                }
                let s = children[i].size(axis);
                let can = s - children[i].min_size(axis);
                let take = can.min(remaining);
                if take > 0 {
                    children[i].resize_along(axis, s - take, at_end);
                    remaining -= take;
                }
            }
        }
    }

    /// Recompute all x/y offsets from sizes (borders are one cell wide).
    fn reposition(&mut self, x: u32, y: u32) {
        self.x = x;
        self.y = y;
        match &mut self.kind {
            CellKind::Pane(_) => {}
            CellKind::Horizontal(c) => {
                let mut cx = x;
                for ch in c {
                    ch.reposition(cx, y);
                    cx += ch.width + 1;
                }
            }
            CellKind::Vertical(c) => {
                let mut cy = y;
                for ch in c {
                    ch.reposition(x, cy);
                    cy += ch.height + 1;
                }
            }
        }
    }

    /// Move the divider between children `index` and `index + 1` of the
    /// container at `path` by `delta` cells (positive = right/down).
    /// The delta is clamped so no pane shrinks below one cell. Returns the
    /// new layout and the delta actually applied.
    pub fn move_divider(&self, path: &[usize], index: usize, delta: i32) -> Option<(LayoutCell, i32)> {
        let mut root = self.clone();
        let node = root.node_at_mut(path)?;
        let axis = match node.kind {
            CellKind::Horizontal(_) => Axis::X,
            CellKind::Vertical(_) => Axis::Y,
            CellKind::Pane(_) => return None,
        };
        let children = match &mut node.kind {
            CellKind::Horizontal(c) | CellKind::Vertical(c) => c,
            CellKind::Pane(_) => unreachable!(),
        };
        if index + 1 >= children.len() {
            return None;
        }
        let (a_size, a_min) = (children[index].size(axis), children[index].min_size(axis));
        let (b_size, b_min) = (children[index + 1].size(axis), children[index + 1].min_size(axis));
        let lo = -((a_size - a_min) as i64);
        let hi = (b_size - b_min) as i64;
        let d = (delta as i64).clamp(lo, hi);
        if d == 0 {
            return Some((self.clone(), 0));
        }
        children[index].resize_along(axis, (a_size as i64 + d) as u32, true);
        children[index + 1].resize_along(axis, (b_size as i64 - d) as u32, false);
        let (x, y) = (root.x, root.y);
        root.reposition(x, y);
        Some((root, d as i32))
    }

    /// Structural sanity check (what tmux's layout_check enforces).
    pub fn is_consistent(&self) -> bool {
        match &self.kind {
            CellKind::Pane(_) => self.width >= 1 && self.height >= 1,
            CellKind::Horizontal(c) => {
                !c.is_empty()
                    && c.iter().all(|ch| ch.height == self.height && ch.y == self.y && ch.is_consistent())
                    && c.iter().map(|ch| ch.width).sum::<u32>() + c.len() as u32 - 1 == self.width
            }
            CellKind::Vertical(c) => {
                !c.is_empty()
                    && c.iter().all(|ch| ch.width == self.width && ch.x == self.x && ch.is_consistent())
                    && c.iter().map(|ch| ch.height).sum::<u32>() + c.len() as u32 - 1 == self.height
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    X,
    Y,
}

struct Cursor<'a> {
    s: &'a [u8],
    pos: usize,
    base: usize,
}

impl Cursor<'_> {
    fn err(&self) -> LayoutError {
        LayoutError::Syntax(self.base + self.pos)
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }

    fn expect(&mut self, c: u8) -> Result<(), LayoutError> {
        if self.peek() == Some(c) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.err())
        }
    }

    fn num(&mut self) -> Result<u32, LayoutError> {
        let start = self.pos;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        if start == self.pos {
            return Err(self.err());
        }
        std::str::from_utf8(&self.s[start..self.pos])
            .unwrap()
            .parse()
            .map_err(|_| self.err())
    }

    fn cell(&mut self) -> Result<LayoutCell, LayoutError> {
        let width = self.num()?;
        self.expect(b'x')?;
        let height = self.num()?;
        self.expect(b',')?;
        let x = self.num()?;
        self.expect(b',')?;
        let y = self.num()?;
        let kind = match self.peek() {
            Some(b',') => {
                // Either a pane id, or (inside a container) the next sibling.
                // A pane id is a number NOT followed by 'x'.
                let save = self.pos;
                self.pos += 1;
                let n = self.num()?;
                if self.peek() == Some(b'x') {
                    // Old-style leaf without pane id; rewind.
                    self.pos = save;
                    return Err(self.err());
                }
                CellKind::Pane(PaneId(n))
            }
            Some(open @ (b'{' | b'[')) => {
                self.pos += 1;
                let close = if open == b'{' { b'}' } else { b']' };
                let mut children = vec![self.cell()?];
                loop {
                    match self.peek() {
                        Some(b',') => {
                            self.pos += 1;
                            children.push(self.cell()?);
                        }
                        Some(c) if c == close => {
                            self.pos += 1;
                            break;
                        }
                        _ => return Err(self.err()),
                    }
                }
                if open == b'{' {
                    CellKind::Horizontal(children)
                } else {
                    CellKind::Vertical(children)
                }
            }
            _ => return Err(self.err()),
        };
        Ok(LayoutCell { width, height, x, y, kind })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real layouts produced by tmux 3.4.
    const SINGLE: &str = "a87d,100x30,0,0,0";
    const NESTED: &str = "b25d,80x24,0,0{40x24,0,0,1,39x24,41,0[39x12,41,0,2,39x11,41,13,3]}";

    fn fix(body: &str) -> String {
        format!("{:04x},{}", checksum(body), body)
    }

    #[test]
    fn checksum_matches_tmux() {
        assert_eq!(checksum("100x30,0,0,0"), 0xa87d);
    }

    #[test]
    fn parse_single() {
        let l = LayoutCell::parse(SINGLE).unwrap();
        assert_eq!(l, LayoutCell { width: 100, height: 30, x: 0, y: 0, kind: CellKind::Pane(PaneId(0)) });
        assert_eq!(l.to_layout_string(), SINGLE);
    }

    #[test]
    fn parse_nested_roundtrip() {
        let s = fix("80x24,0,0{40x24,0,0,1,39x24,41,0[39x12,41,0,2,39x11,41,13,3]}");
        let l = LayoutCell::parse(&s).unwrap();
        assert_eq!(l.panes(), vec![PaneId(1), PaneId(2), PaneId(3)]);
        assert!(l.is_consistent());
        assert_eq!(l.to_layout_string(), s);
        assert_eq!(l.find_pane(PaneId(3)).unwrap().y, 13);
        let _ = NESTED;
    }

    #[test]
    fn rejects_bad_checksum_and_syntax() {
        assert!(matches!(LayoutCell::parse("0000,100x30,0,0,0"), Err(LayoutError::Checksum { .. })));
        for body in ["100x30,0,0", "100x30,0,0{", "100x30,0,0{10x30,0,0,1", "100x30,0,0,1}", "x30,0,0,1", "100x30,0,0[]"] {
            assert!(LayoutCell::parse(&fix(body)).is_err(), "{body}");
        }
        assert!(LayoutCell::parse("").is_err());
    }

    #[test]
    fn move_simple_vertical_divider() {
        let l = LayoutCell::parse(&fix("80x24,0,0{40x24,0,0,1,39x24,41,0,2}")).unwrap();
        let (n, d) = l.move_divider(&[], 0, 5).unwrap();
        assert_eq!(d, 5);
        assert_eq!(n.body(), "80x24,0,0{45x24,0,0,1,34x24,46,0,2}");
        assert!(n.is_consistent());
        // Clamped at the far edge: right pane keeps 1 column.
        let (n, d) = l.move_divider(&[], 0, 1000).unwrap();
        assert_eq!(d, 38);
        assert_eq!(n.body(), "80x24,0,0{78x24,0,0,1,1x24,79,0,2}");
        let (n, d) = l.move_divider(&[], 0, -1000).unwrap();
        assert_eq!(d, -39);
        assert_eq!(n.body(), "80x24,0,0{1x24,0,0,1,78x24,2,0,2}");
    }

    #[test]
    fn move_divider_with_nested_children() {
        // Left: a vertical stack; right: side-by-side pair (same axis as root).
        let body = "81x24,0,0{40x24,0,0[40x12,0,0,1,40x11,0,13,2],40x24,41,0{20x24,41,0,3,19x24,62,0,4}}";
        let l = LayoutCell::parse(&fix(body)).unwrap();
        assert!(l.is_consistent());
        let (n, d) = l.move_divider(&[], 0, 10).unwrap();
        assert_eq!(d, 10);
        // Left stack panes both widen; on the right the change is taken from
        // the pane adjacent to the divider (pane 3) only.
        assert_eq!(n.body(), "81x24,0,0{50x24,0,0[50x12,0,0,1,50x11,0,13,2],30x24,51,0{10x24,51,0,3,19x24,62,0,4}}");
        assert!(n.is_consistent());
        // Spill-over: pane 3 hits its minimum, pane 4 gives the rest.
        let (n, d) = l.move_divider(&[], 0, 25).unwrap();
        assert_eq!(d, 25);
        assert_eq!(n.body(), "81x24,0,0{65x24,0,0[65x12,0,0,1,65x11,0,13,2],15x24,66,0{1x24,66,0,3,13x24,68,0,4}}");
        assert!(n.is_consistent());
        // Max: right side min is 1+1+1 = 3.
        let (_, d) = l.move_divider(&[], 0, 1000).unwrap();
        assert_eq!(d, 37);
    }

    #[test]
    fn move_divider_inside_nested_node() {
        let body = "80x24,0,0{40x24,0,0[40x12,0,0,1,40x11,0,13,2],39x24,41,0,3}";
        let l = LayoutCell::parse(&fix(body)).unwrap();
        let (n, d) = l.move_divider(&[0], 0, -4).unwrap();
        assert_eq!(d, -4);
        assert_eq!(n.body(), "80x24,0,0{40x24,0,0[40x8,0,0,1,40x15,0,9,2],39x24,41,0,3}");
        assert!(n.is_consistent());
        assert!(l.move_divider(&[0], 1, 1).is_none());
        assert!(l.move_divider(&[1], 0, 1).is_none());
        assert!(l.move_divider(&[5], 0, 1).is_none());
    }

    #[test]
    fn serde_shape_for_frontend() {
        let l = LayoutCell::parse(&fix("80x24,0,0{40x24,0,0,1,39x24,41,0,2}")).unwrap();
        let v = serde_json::to_value(&l).unwrap();
        assert_eq!(v["type"], "horizontal");
        assert_eq!(v["value"][1]["type"], "pane");
        assert_eq!(v["value"][1]["value"], "%2");
        assert_eq!(v["value"][1]["x"], 41);
    }
}
