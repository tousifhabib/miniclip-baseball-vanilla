//! 2D transforms and colour transforms.

use bb_format as f;

/// A 2D affine transform: `x' = a*x + c*y + tx`, `y' = b*x + d*y + ty`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub tx: f32,
    pub ty: f32,
}

impl Matrix {
    pub const IDENTITY: Matrix = Matrix {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    pub fn translate(tx: f32, ty: f32) -> Matrix {
        Matrix {
            tx,
            ty,
            ..Matrix::IDENTITY
        }
    }

    pub fn scale(sx: f32, sy: f32) -> Matrix {
        Matrix {
            a: sx,
            d: sy,
            ..Matrix::IDENTITY
        }
    }

    /// The transform that applies `inner` first and then `self`.
    pub fn then_inner(self, inner: Matrix) -> Matrix {
        Matrix {
            a: self.a * inner.a + self.c * inner.b,
            b: self.b * inner.a + self.d * inner.b,
            c: self.a * inner.c + self.c * inner.d,
            d: self.b * inner.c + self.d * inner.d,
            tx: self.a * inner.tx + self.c * inner.ty + self.tx,
            ty: self.b * inner.tx + self.d * inner.ty + self.ty,
        }
    }

    pub fn apply(self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.tx,
            self.b * x + self.d * y + self.ty,
        )
    }

    /// `None` when the transform squashes everything onto a line or point.
    pub fn inverse(self) -> Option<Matrix> {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 1e-12 {
            return None;
        }
        let (a, b, c, d) = (self.d / det, -self.b / det, -self.c / det, self.a / det);
        Some(Matrix {
            a,
            b,
            c,
            d,
            tx: -(a * self.tx + c * self.ty),
            ty: -(b * self.tx + d * self.ty),
        })
    }
}

impl From<f::Matrix> for Matrix {
    fn from(m: f::Matrix) -> Matrix {
        Matrix {
            a: m[0] as f32,
            b: m[1] as f32,
            c: m[2] as f32,
            d: m[3] as f32,
            tx: m[4] as f32,
            ty: m[5] as f32,
        }
    }
}

/// Each channel becomes `channel * mult + add`, with channels from 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorTransform {
    pub mult: [f32; 4],
    pub add: [f32; 4],
}

impl ColorTransform {
    pub const IDENTITY: ColorTransform = ColorTransform {
        mult: [1.0; 4],
        add: [0.0; 4],
    };

    /// The transform that applies `inner` first and then `self`.
    pub fn then_inner(self, inner: ColorTransform) -> ColorTransform {
        let mut out = ColorTransform::IDENTITY;
        for i in 0..4 {
            out.mult[i] = self.mult[i] * inner.mult[i];
            out.add[i] = self.mult[i] * inner.add[i] + self.add[i];
        }
        out
    }
}

impl From<f::ColorTransform> for ColorTransform {
    fn from(c: f::ColorTransform) -> ColorTransform {
        ColorTransform {
            mult: c.mult.map(|m| m as f32),
            add: c.add.map(|a| f32::from(a) / 255.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 1e-4 && (a.1 - b.1).abs() < 1e-4
    }

    #[test]
    fn then_inner_applies_the_inner_transform_first() {
        let outer = Matrix::translate(10.0, 0.0);
        let inner = Matrix::scale(2.0, 2.0);
        // Scale (1, 1) to (2, 2), then move it right by 10.
        assert!(close(outer.then_inner(inner).apply(1.0, 1.0), (12.0, 2.0)));
        // The other way round: move to (11, 1), then scale.
        assert!(close(inner.then_inner(outer).apply(1.0, 1.0), (22.0, 2.0)));
    }

    #[test]
    fn inverse_undoes_a_transform() {
        let m = Matrix {
            a: 2.0,
            b: 0.5,
            c: -1.0,
            d: 3.0,
            tx: 7.0,
            ty: -4.0,
        };
        let (x, y) = m.apply(3.0, 5.0);
        assert!(close(m.inverse().unwrap().apply(x, y), (3.0, 5.0)));
    }

    #[test]
    fn a_flat_transform_has_no_inverse() {
        assert!(Matrix::scale(0.0, 1.0).inverse().is_none());
    }

    #[test]
    fn color_transforms_compose_inner_first() {
        let halve = ColorTransform {
            mult: [0.5; 4],
            add: [0.0; 4],
        };
        let brighten = ColorTransform {
            mult: [1.0; 4],
            add: [0.2; 4],
        };
        // Brighten then halve: (c + 0.2) * 0.5.
        let both = halve.then_inner(brighten);
        assert_eq!(both.mult, [0.5; 4]);
        assert_eq!(both.add, [0.1; 4]);
    }
}
