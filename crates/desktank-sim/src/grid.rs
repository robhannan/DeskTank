//! Cells and rectangles (SPEC §2.1).

use crate::material::MaterialId;

/// One grid cell: 4 bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Cell {
    /// The material in this cell.
    pub material: MaterialId,
    /// Variant or colour-noise byte, used by the renderer.
    pub variant: u8,
    /// Per-material state: for floating materials, ticks spent floating.
    pub state: u16,
}

impl Cell {
    /// A fresh cell of `material` with zero state.
    #[must_use]
    pub fn new(material: MaterialId, variant: u8) -> Self {
        Self {
            material,
            variant,
            state: 0,
        }
    }
}

/// A half-open cell rectangle `[x0, x1) × [y0, y1)`. Empty when it has no
/// area.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rect {
    /// Left edge (inclusive).
    pub x0: usize,
    /// Top edge (inclusive).
    pub y0: usize,
    /// Right edge (exclusive).
    pub x1: usize,
    /// Bottom edge (exclusive).
    pub y1: usize,
}

impl Rect {
    /// The empty rectangle.
    pub const EMPTY: Self = Self {
        x0: 0,
        y0: 0,
        x1: 0,
        y1: 0,
    };

    /// Whether the rectangle has no area.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }

    /// Whether the cell `(x, y)` lies inside.
    #[must_use]
    pub fn contains(&self, x: usize, y: usize) -> bool {
        (self.x0..self.x1).contains(&x) && (self.y0..self.y1).contains(&y)
    }

    /// The smallest rectangle covering both.
    #[must_use]
    pub fn union(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        Self {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }

    /// The overlap of both (empty if they don't overlap).
    #[must_use]
    pub fn intersect(self, other: Self) -> Self {
        let r = Self {
            x0: self.x0.max(other.x0),
            y0: self.y0.max(other.y0),
            x1: self.x1.min(other.x1),
            y1: self.y1.min(other.y1),
        };
        if r.is_empty() { Self::EMPTY } else { r }
    }

    /// Number of cells covered.
    #[must_use]
    pub fn area(&self) -> usize {
        if self.is_empty() {
            0
        } else {
            (self.x1 - self.x0) * (self.y1 - self.y0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_ops() {
        let a = Rect {
            x0: 0,
            y0: 0,
            x1: 4,
            y1: 4,
        };
        let b = Rect {
            x0: 2,
            y0: 3,
            x1: 8,
            y1: 5,
        };
        assert_eq!(
            a.union(b),
            Rect {
                x0: 0,
                y0: 0,
                x1: 8,
                y1: 5
            }
        );
        assert_eq!(a.intersect(b).area(), 2);
        assert!(a.intersect(Rect::EMPTY).is_empty());
        assert_eq!(Rect::EMPTY.union(b), b);
        assert!(b.contains(2, 4) && !b.contains(8, 4));
    }
}
