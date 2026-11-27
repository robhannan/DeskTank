//! Image comparison for the render tests: CIEDE2000 colour difference
//! (Sharma, Wu and Dalal 2005) on sRGB pixels.
//!
//! Variable names follow the paper's notation.
#![allow(
    clippy::many_single_char_names,
    clippy::similar_names,
    clippy::manual_midpoint,
    clippy::cast_precision_loss
)]

/// CIELAB (D65) of an 8-bit sRGB colour.
pub fn srgb_to_lab([r, g, b]: [u8; 3]) -> [f64; 3] {
    let lin = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (lin(r), lin(g), lin(b));
    // Linear sRGB to XYZ, normalised by the D65 white point.
    let x = (0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b) / 0.950_47;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175 * b;
    let z = (0.019_333_9 * r + 0.119_192 * g + 0.950_304_1 * b) / 1.088_83;
    let f = |t: f64| {
        if t > 216.0 / 24389.0 {
            t.cbrt()
        } else {
            (24389.0 / 27.0 * t + 16.0) / 116.0
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIEDE2000 ΔE between two CIELAB colours (kL = kC = kH = 1).
pub fn delta_e2000([l1, a1, b1]: [f64; 3], [l2, a2, b2]: [f64; 3]) -> f64 {
    let deg = |r: f64| r.to_degrees();
    let rad = |d: f64| d.to_radians();
    let c1 = a1.hypot(b1);
    let c2 = a2.hypot(b2);
    let c_bar7 = ((c1 + c2) / 2.0).powi(7);
    let g = 0.5 * (1.0 - (c_bar7 / (c_bar7 + 25f64.powi(7))).sqrt());
    let (a1p, a2p) = ((1.0 + g) * a1, (1.0 + g) * a2);
    let (c1p, c2p) = (a1p.hypot(b1), a2p.hypot(b2));
    let hue = |b: f64, a: f64| {
        if a == 0.0 && b == 0.0 {
            0.0
        } else {
            deg(b.atan2(a)).rem_euclid(360.0)
        }
    };
    let (h1p, h2p) = (hue(b1, a1p), hue(b2, a2p));

    let dl = l2 - l1;
    let dc = c2p - c1p;
    let dh = if c1p * c2p == 0.0 {
        0.0
    } else if (h2p - h1p).abs() <= 180.0 {
        h2p - h1p
    } else if h2p <= h1p {
        h2p - h1p + 360.0
    } else {
        h2p - h1p - 360.0
    };
    let dhh = 2.0 * (c1p * c2p).sqrt() * (rad(dh) / 2.0).sin();

    let lbar = (l1 + l2) / 2.0;
    let cbar = (c1p + c2p) / 2.0;
    let hbar = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };
    let t = 1.0 - 0.17 * (rad(hbar - 30.0)).cos()
        + 0.24 * (rad(2.0 * hbar)).cos()
        + 0.32 * (rad(3.0 * hbar + 6.0)).cos()
        - 0.20 * (rad(4.0 * hbar - 63.0)).cos();
    let d_theta = 30.0 * (-((hbar - 275.0) / 25.0).powi(2)).exp();
    let cbar7 = cbar.powi(7);
    let rc = 2.0 * (cbar7 / (cbar7 + 25f64.powi(7))).sqrt();
    let l50 = (lbar - 50.0).powi(2);
    let sl = 1.0 + 0.015 * l50 / (20.0 + l50).sqrt();
    let sc = 1.0 + 0.045 * cbar;
    let sh = 1.0 + 0.015 * cbar * t;
    let rt = -(2.0 * rad(d_theta)).sin() * rc;
    ((dl / sl).powi(2) + (dc / sc).powi(2) + (dhh / sh).powi(2) + rt * (dc / sc) * (dhh / sh))
        .sqrt()
}

/// How two same-sized RGBA images differ.
#[derive(Debug)]
pub struct Diff {
    /// Pixels whose ΔE2000 exceeds the threshold.
    pub over: usize,
    /// All pixels.
    pub total: usize,
    /// Largest ΔE2000 seen.
    pub max: f64,
}

impl Diff {
    /// Fraction of pixels over the threshold.
    pub fn fraction(&self) -> f64 {
        self.over as f64 / self.total as f64
    }
}

/// Compares RGBA8 images pixel by pixel (alpha ignored).
pub fn compare(a: &[u8], b: &[u8], threshold: f64) -> Diff {
    assert_eq!(a.len(), b.len(), "image sizes differ");
    let mut diff = Diff {
        over: 0,
        total: a.len() / 4,
        max: 0.0,
    };
    for (p, q) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0) {
        if p[..3] == q[..3] {
            continue;
        }
        let de = delta_e2000(
            srgb_to_lab([p[0], p[1], p[2]]),
            srgb_to_lab([q[0], q[1], q[2]]),
        );
        diff.max = diff.max.max(de);
        if de > threshold {
            diff.over += 1;
        }
    }
    diff
}

#[test]
fn delta_e2000_matches_sharma_reference_pairs() {
    // From Sharma, Wu and Dalal's test data (pairs 1, 2, 3, 7, 13, 17, 19, 25).
    let pairs = [
        ([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485], 2.0425),
        ([50.0, 3.1571, -77.2803], [50.0, 0.0, -82.7485], 2.8615),
        ([50.0, 2.8361, -74.0200], [50.0, 0.0, -82.7485], 3.4412),
        ([50.0, 0.0, 0.0], [50.0, -1.0, 2.0], 2.3669),
        ([50.0, 2.4900, -0.0010], [50.0, -2.4900, 0.0009], 7.1792),
        ([50.0, 2.5, 0.0], [73.0, 25.0, -18.0], 27.1492),
        ([50.0, 2.5, 0.0], [58.0, 24.0, 15.0], 19.4535),
        (
            [60.2574, -34.0099, 36.2677],
            [60.4626, -34.1751, 39.4387],
            1.2644,
        ),
    ];
    for (a, b, want) in pairs {
        for (x, y) in [(a, b), (b, a)] {
            let got = delta_e2000(x, y);
            assert!((got - want).abs() < 1e-4, "{x:?} vs {y:?}: {got} != {want}");
        }
    }
}

#[test]
fn srgb_to_lab_known_colours() {
    let close =
        |got: [f64; 3], want: [f64; 3]| got.iter().zip(want).all(|(g, w)| (g - w).abs() < 0.05);
    assert!(close(srgb_to_lab([255, 255, 255]), [100.0, 0.0, 0.0]));
    assert!(close(srgb_to_lab([0, 0, 0]), [0.0, 0.0, 0.0]));
    // sRGB red: L* 53.24, a* 80.09, b* 67.20.
    assert!(close(srgb_to_lab([255, 0, 0]), [53.24, 80.09, 67.20]));
}
