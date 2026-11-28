//! A small pixel-art sprite of a mature plant, for its info panels (SPEC
//! §2.5): the species card and the plant's page of the inspect card.
//!
//! Built on the CPU from the species data alone, with no state: its growth
//! form and the form's parameters, its drawn shapes ([`Look`]), its colour
//! ramps and its colouration. Each of the seven forms, and each leaf, stem and
//! root shape within it, has its own drawing, so every shipped species has a
//! distinct, recognisable silhouette. The sprite is drawn at one pixel per
//! card pixel, with the plant's base on the bottom row of what is above the
//! substrate (roots hang below it), and is cropped to what it draws.
//!
//! The same data drive the tank's plants ([`crate::plants`]), but the tank
//! grows them from their cells; this draws a fully grown example directly,
//! the colours coming only from the species' ramps (the colouration being a
//! tint of the leaf ramp, as in the tank, that keeps each shade's lightness).

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::many_single_char_names,
    clippy::similar_names,
    clippy::too_many_arguments,
    clippy::too_many_lines
)]

use desktank_sim::plants::{
    Arrangement, EpiphyteForm, FloaterForm, Form, LeafShape, Look, MatForm, PlantSpecies,
    RootShape, RosetteForm, StemForm, StemShape,
};

/// A cropped sprite: its size in pixels and its RGBA rows, sRGB, alpha 0 or
/// 255.
pub(crate) type Sprite = ((usize, usize), Vec<[u8; 4]>);

/// The tallest a sprite may be, in pixels.
pub(crate) const MAX_HEIGHT: usize = 38;
/// The widest a sprite may be, in pixels.
pub(crate) const MAX_WIDTH: usize = 88;

type Rgb = [u8; 3];

/// A shade for a ramp a species left empty (none of the shipped ones do).
const FALLBACK: Rgb = [0x40, 0x90, 0x40];

/// Working canvas: `x` runs from `-OX`, `y` from `-OY` (up) to `H - OY`
/// (down); `y = 0` is the base row, where the plant meets the substrate, the
/// water's surface or the rock.
const W: i32 = 128;
const H: i32 = 96;
const OX: i32 = 64;
const OY: i32 = 60;

/// The sprite of a mature `sp`.
pub(crate) fn build(sp: &PlantSpecies) -> Sprite {
    let mut pic = Pic::new();
    let c = Ctx::new(sp);
    match &sp.form {
        Form::Stem(f) => stem(&mut pic, &c, f),
        Form::Mat(f) => mat(&mut pic, &c, f),
        Form::Rosette(f) => rosette(&mut pic, &c, f),
        Form::Carpet(f) => carpet(&mut pic, &c, f),
        Form::Floater(f) => floater(&mut pic, &c, f),
        Form::Epiphyte(f) => epiphyte(&mut pic, &c, f),
        Form::Ball(_) => ball(&mut pic, &c),
    }
    pic.finish()
}

/// Shade `i` of `ramp`, clamped to its ends.
fn at(ramp: &[Rgb], i: i32) -> Rgb {
    let last = ramp.len().saturating_sub(1) as i32;
    ramp.get(i.clamp(0, last) as usize)
        .copied()
        .unwrap_or(FALLBACK)
}

/// The shade at fraction `t` (0 dark, 1 light) along `ramp`.
fn pick(ramp: &[Rgb], t: f32) -> Rgb {
    let last = ramp.len().saturating_sub(1) as f32;
    at(ramp, (t.clamp(0.0, 1.0) * last).round() as i32)
}

fn luma(c: [f32; 3]) -> f32 {
    0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]
}

/// `shade` turned towards `to`, keeping its lightness (as the tank's
/// colouration does).
pub(crate) fn tint(shade: Rgb, to: Rgb) -> Rgb {
    const WEIGHT: f32 = 0.6;
    let from = shade.map(f32::from);
    let to = to.map(f32::from);
    let mixed = [0, 1, 2].map(|i| from[i] * (1.0 - WEIGHT) + to[i] * WEIGHT);
    let (l0, l1) = (luma(from), luma(mixed));
    // Most of its lightness, so a pale shade still turns a clear colour.
    let k = if l1 > 0.0 { (l0 / l1).powf(0.6) } else { 1.0 };
    mixed.map(|v| (v * k).round().clamp(0.0, 255.0) as u8)
}

/// The leaf ramp's shades turned towards the species' colouration.
pub(crate) fn tinted_leaf(sp: &PlantSpecies) -> Option<Vec<Rgb>> {
    sp.colouration
        .map(|c| sp.colours.leaf.iter().map(|&s| tint(s, c.colour)).collect())
}

/// A hash of two coordinates and a seed.
fn hash(seed: u32, a: i32, b: i32) -> u32 {
    let mut h = seed ^ (a as u32).wrapping_mul(0x9e37_79b1) ^ (b as u32).wrapping_mul(0x85eb_ca6b);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^ (h >> 15)
}

/// What a species is drawn from.
struct Ctx {
    root: Vec<Rgb>,
    stem: Vec<Rgb>,
    leaf: Vec<Rgb>,
    /// The leaf ramp turned towards the colouration, or the leaf ramp itself.
    tip: Vec<Rgb>,
    look: Look,
    seed: u32,
}

impl Ctx {
    fn new(sp: &PlantSpecies) -> Self {
        let seed = sp.name.bytes().fold(0x811c_9dc5_u32, |h, b| {
            (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
        });
        Self {
            root: sp.colours.root.clone(),
            stem: sp.colours.stem.clone(),
            leaf: sp.colours.leaf.clone(),
            tip: tinted_leaf(sp).unwrap_or_else(|| sp.colours.leaf.clone()),
            look: sp.look,
            seed,
        }
    }

    /// A number in `0..n` from the seed and two coordinates.
    fn roll(&self, a: i32, b: i32, n: u32) -> i32 {
        (hash(self.seed, a, b) % n.max(1)) as i32
    }

    /// A fraction in `0..1`.
    fn unit(&self, a: i32, b: i32) -> f32 {
        (hash(self.seed, a, b) >> 8) as f32 / 16_777_216.0
    }
}

/// The picture being drawn.
struct Pic {
    px: Vec<Option<Rgb>>,
}

impl Pic {
    fn new() -> Self {
        Self {
            px: vec![None; (W * H) as usize],
        }
    }

    fn index(x: i32, y: i32) -> Option<usize> {
        let (ix, iy) = (x + OX, y + OY);
        ((0..W).contains(&ix) && (0..H).contains(&iy)).then_some((iy * W + ix) as usize)
    }

    fn put(&mut self, x: i32, y: i32, c: Rgb) {
        if let Some(i) = Self::index(x, y) {
            self.px[i] = Some(c);
        }
    }

    fn hline(&mut self, x0: i32, x1: i32, y: i32, c: Rgb) {
        for x in x0..=x1 {
            self.put(x, y, c);
        }
    }

    /// Crops to what is drawn.
    fn finish(self) -> Sprite {
        let mut bounds: Option<(i32, i32, i32, i32)> = None;
        for (i, p) in self.px.iter().enumerate() {
            if p.is_some() {
                let (x, y) = (i as i32 % W, i as i32 / W);
                bounds = Some(bounds.map_or((x, y, x, y), |(a, b, c, d)| {
                    (a.min(x), b.min(y), c.max(x), d.max(y))
                }));
            }
        }
        let Some((x0, y0, x1, y1)) = bounds else {
            return ((0, 0), Vec::new());
        };
        let mut out = Vec::new();
        for y in y0..=y1 {
            for x in x0..=x1 {
                out.push(match self.px[(y * W + x) as usize] {
                    Some(c) => [c[0], c[1], c[2], 255],
                    None => [0, 0, 0, 0],
                });
            }
        }
        (((x1 - x0 + 1) as usize, (y1 - y0 + 1) as usize), out)
    }
}

/// Roots hanging from `(x0, y0)`: `count` strands `len` long, fanning out
/// by `fan`, paler towards their tips. `thick` roots are two pixels wide.
fn roots(pic: &mut Pic, c: &Ctx, x0: i32, y0: i32, count: i32, len: i32, fan: f32, thick: bool) {
    for i in 0..count {
        let u = if count > 1 {
            (i as f32 / (count - 1) as f32) * 2.0 - 1.0
        } else {
            0.0
        };
        let n = (len - c.roll(i, 5, 2)).max(1);
        let wobble = c.unit(i, 9) * 6.0;
        for k in 1..=n {
            let t = k as f32 / n as f32;
            let x = x0
                + (u * fan * t.sqrt()).round() as i32
                + ((k as f32 * 0.9 + wobble).sin() * 0.6).round() as i32;
            let shade = pick(&c.root, 0.2 + 0.8 * t);
            pic.put(x, y0 + k, shade);
            if thick && k < n {
                pic.put(x + 1, y0 + k, pick(&c.root, 0.1 + 0.6 * t));
            }
        }
    }
}

// ---------------------------------------------------------------- stems

/// An upright stem with leaves at nodes, roots below and, where the species
/// branches, short side shoots.
fn stem(pic: &mut Pic, c: &Ctx, f: &StemForm) {
    let h = ((f32::from(f.height) * 0.6).round() as i32).clamp(16, 30);
    let thick = c.look.stem == StemShape::Thick;
    let phase = c.unit(1, 1) * 6.0;
    let sway = move |k: i32| ((k as f32 / 8.0 + phase).sin() * 1.1).round() as i32;
    let rd = (i32::from(f.root_depth) / 2).clamp(3, 6);
    roots(
        pic,
        c,
        sway(0),
        0,
        3,
        rd,
        3.0,
        c.look.root == RootShape::Thick,
    );
    let main = Run {
        x0: 0,
        len: h,
        thick,
        leaf_scale: 1.0,
        height: h,
        base: 0,
    };
    run(pic, c, f, &main, &sway);
    if f.branching > 0.0 {
        let shoots = if f.branching >= 0.18 { 2 } else { 1 };
        for s in 0..shoots {
            let dir = if s == 0 { 1 } else { -1 };
            let k0 = if s == 0 { h * 36 / 100 } else { h * 58 / 100 };
            let len = (h * 4 / 10).max(8);
            let shoot = Run {
                x0: sway(k0) + dir,
                len,
                thick: false,
                leaf_scale: 0.75,
                height: h,
                base: k0,
            };
            run(pic, c, f, &shoot, &move |k| dir * ((k + 1) / 2).min(4));
        }
    }
}

/// One stalk: the main stem or a side shoot.
struct Run {
    /// Its first pixel's column.
    x0: i32,
    /// Its length in rows.
    len: i32,
    thick: bool,
    /// Leaf size next to the main stem's.
    leaf_scale: f32,
    /// The whole plant's height in rows, for shading by height.
    height: i32,
    /// Row of its first pixel.
    base: i32,
}

fn run(pic: &mut Pic, c: &Ctx, f: &StemForm, r: &Run, offset: &dyn Fn(i32) -> i32) {
    let step = (i32::from(f.internode) * 2 + 1).max(4);
    let first = if r.base == 0 { 3 } else { 2 };
    for k in 0..=r.len {
        let ht = (r.base + k) as f32 / r.height as f32;
        let x = r.x0 + offset(k);
        let y = -(r.base + k);
        pic.put(x, y, pick(&c.stem, 0.15 + 0.6 * ht));
        if r.thick {
            pic.put(x + 1, y, pick(&c.stem, 0.4 + 0.55 * ht));
        }
    }
    let mut k = first;
    let mut node = 0;
    while k < r.len - 1 {
        node += 1;
        let ht = (r.base + k) as f32 / r.height as f32;
        let x = r.x0 + offset(k);
        let y = -(r.base + k);
        let len = (f32::from(f.leaf_length) * 2.0 + 1.0) * (1.0 - 0.35 * ht) * r.leaf_scale;
        let len = (len.round() as i32).max(3);
        // Leaves at the upper nodes carry the colouration, if the species
        // has one.
        let ramp: &[Rgb] = if ht > 0.5 { &c.tip } else { &c.leaf };
        if c.look.arrangement == Arrangement::Whorl {
            // A whorl: a disc of fine segments round the node.
            whorl(pic, ramp, (x, y), len, ht);
        } else {
            // Opposite pairs, turned a quarter at alternate nodes: there the
            // side leaves are seen at an angle (shorter) and a front leaf is
            // drawn over the stem.
            let turned = node % 2 == 0;
            let side = if turned { (len - 2).max(2) } else { len };
            for dir in [-1, 1] {
                let sx = if dir > 0 && r.thick { x + 1 } else { x };
                stem_leaf(pic, ramp, c.look.leaf, (sx, y), dir, side, ht);
            }
            if turned {
                front_leaf(pic, ramp, c.look.leaf, (x + i32::from(r.thick), y), ht);
            }
        }
        k += step;
    }
    // The growing tip: a small bud of the brightest leaf shade.
    let ht = (r.base + r.len) as f32 / r.height as f32;
    let (x, y) = (r.x0 + offset(r.len), -(r.base + r.len));
    let ramp: &[Rgb] = if ht > 0.5 { &c.tip } else { &c.leaf };
    pic.put(x, y - 1, pick(ramp, 0.9));
    if c.look.leaf != LeafShape::Needle {
        pic.put(x - 1, y, pick(ramp, 0.75));
        pic.put(x + 1, y, pick(ramp, 0.75));
    }
}

/// A leaf turned to the viewer, drawn over the stem at its node `at`: an oval
/// (a needle's a small tuft) lit above and darker beneath, a pale rib down it.
fn front_leaf(pic: &mut Pic, ramp: &[Rgb], shape: LeafShape, at: (i32, i32), ht: f32) {
    let (x, y) = at;
    let base = 0.35 + 0.4 * ht;
    if shape == LeafShape::Feather {
        // A fan seen face-on: filaments radiating up from the stem.
        for i in 0..5 {
            let th = 0.5 + i as f32 * 0.55;
            for r in 1..=3 {
                let (dx, dy) = (
                    (r as f32 * th.cos()).round() as i32,
                    (r as f32 * th.sin()).round() as i32,
                );
                pic.put(x + dx, y - 1 - dy, pick(ramp, base + 0.1 + 0.05 * r as f32));
            }
        }
        return;
    }
    let rows: &[(i32, i32)] = if shape == LeafShape::Needle {
        &[(-1, 0), (0, 1), (1, 0)]
    } else {
        &[(-2, 0), (-1, 1), (0, 1), (1, 1), (2, 0)]
    };
    for &(dy, w) in rows {
        for dx in -w..=w {
            let lit = 0.15 * (1 - dy).clamp(-1, 2) as f32 / 2.0 + if dx == 0 { 0.1 } else { 0.0 };
            pic.put(x + dx, y + dy - 1, pick(ramp, base + lit));
        }
    }
}

/// A whorl (cabomba): a disc of fine, lacy leaf segments round the node `at`,
/// `len` pixels across each way and about a third as tall, with a solid rib through
/// it, lighter towards its upper rim.
fn whorl(pic: &mut Pic, ramp: &[Rgb], at: (i32, i32), len: i32, ht: f32) {
    let (x, y) = at;
    let rx = len as f32;
    let ry = (len as f32 * 0.38).max(2.0);
    let base = 0.25 + 0.4 * ht;
    for dy in -(ry as i32)..=(ry as i32) {
        for dx in -len..=len {
            if dx == 0 {
                continue;
            }
            let inside = (dx as f32 / rx).powi(2) + (dy as f32 / ry).powi(2) <= 1.0;
            let lace = (dx + dy).rem_euclid(2) == 0;
            if inside && (dy == 0 || lace) {
                let lit = 0.3 * (-dy as f32 / ry + 1.0) / 2.0;
                pic.put(x + dx, y + dy - 1, pick(ramp, base + lit));
            }
        }
    }
}

/// A profile `0..=max` along a leaf's `t` (0 base, 1 tip), widest at `peak`.
fn width(t: f32, max: f32, peak: f32) -> i32 {
    let s = if t < peak {
        (t / peak).powf(0.6)
    } else {
        ((1.0 - t) / (1.0 - peak)).powf(0.9)
    };
    ((max * s).round() as i32).max(1)
}

/// One leaf of a stem node, off the stem at `from`, `dir` towards `+x` or
/// `-x`, `len` pixels long.
fn stem_leaf(
    pic: &mut Pic,
    ramp: &[Rgb],
    shape: LeafShape,
    from: (i32, i32),
    dir: i32,
    len: i32,
    ht: f32,
) {
    let (sx, sy) = from;
    let base = 0.2 + 0.5 * ht;
    match shape {
        LeafShape::Needle => {
            // A fine blade rising from base to tip.
            for k in 1..=len {
                let t = k as f32 / len as f32;
                pic.put(sx + dir * k, sy - (k * 2) / 3, pick(ramp, base + 0.4 * t));
            }
        }
        LeafShape::Round => {
            // A fuller, blunt oval.
            let max = (len as f32 * 0.55).round().clamp(2.0, 4.0);
            for k in 1..=len {
                let s = (k as f32 - 0.5) / len as f32;
                let w = ((max * (1.0 - (2.0 * s - 1.0).powi(2)).sqrt()).round() as i32).max(1);
                let cy = sy - i32::from(k * 2 >= len);
                let top = cy - w / 2;
                for r in 0..w {
                    let lit = if w > 1 {
                        1.0 - r as f32 / (w - 1) as f32
                    } else {
                        0.5
                    };
                    let t = base + 0.3 * lit + 0.1 * s;
                    pic.put(sx + dir * k, top + r, pick(ramp, t));
                }
            }
        }
        LeafShape::Feather => {
            // A short petiole rising out from the stem to a fan: thin filaments
            // radiating up and out from its end, lighter towards their tips,
            // water between them.
            let apex = (sx + dir * 2, sy - 2);
            for k in 1..=2 {
                pic.put(sx + dir * k, sy - k, pick(ramp, base));
            }
            let reach = (len - 4).clamp(3, 4);
            for i in 0..6 {
                let th = -0.2 + i as f32 * 0.5;
                for r in 1..=reach {
                    let (dx, dy) = (
                        (r as f32 * th.cos()).round() as i32,
                        (r as f32 * th.sin()).round() as i32,
                    );
                    let tip = r as f32 / reach as f32;
                    pic.put(
                        apex.0 + dir * dx,
                        apex.1 - dy,
                        pick(ramp, base + 0.1 + 0.3 * tip),
                    );
                }
            }
        }
        _ => {
            // Lanceolate: a broad blade rising to a point.
            for k in 1..=len {
                let t = k as f32 / len as f32;
                let w = width(t, 3.0, 0.42);
                let cy = sy - (k - 1) / 2;
                let top = cy - w / 2;
                for r in 0..w {
                    let lit = if w > 1 {
                        1.0 - r as f32 / (w - 1) as f32
                    } else {
                        0.5
                    };
                    let tip = if k == len { 0.2 } else { 0.0 };
                    pic.put(
                        sx + dir * k,
                        top + r,
                        pick(ramp, base + 0.28 * lit + 0.15 * t + tip),
                    );
                }
            }
        }
    }
}

// ------------------------------------------------------------ rosettes

/// How wide a blade is at `t` along it (0 base, 1 tip).
#[derive(Clone, Copy)]
enum Blade {
    /// A narrow petiole flaring into a broad pointed blade, with a pale
    /// midrib.
    Sword { max: f32 },
    /// A twisting one- or two-pixel strap.
    Ribbon,
}

/// A blade of `len` rows rising from `(x0, 0)` and arching `drift` pixels
/// outwards at its tip.
fn draw_blade(
    pic: &mut Pic,
    c: &Ctx,
    x0: i32,
    y0: i32,
    len: i32,
    drift: f32,
    kind: Blade,
    id: i32,
) {
    let phase = c.unit(id, 3) * 6.0;
    for k in 0..len {
        let t = (k as f32 + 0.5) / len as f32;
        let arch = drift * t.powf(1.7);
        let wave = match kind {
            Blade::Ribbon => ((t * 6.0 + phase).sin() * 0.9).round(),
            Blade::Sword { .. } => 0.0,
        };
        let xc = x0 + (arch + wave).round() as i32;
        let y = y0 - k;
        match kind {
            Blade::Sword { max } => {
                let w = if t < 0.28 {
                    1
                } else {
                    let s = (t - 0.28) / 0.72;
                    (((max * (std::f32::consts::PI * s.powf(0.8)).sin().powf(0.8)).round()) as i32)
                        .max(1)
                };
                let lo = xc - (w - 1) / 2;
                for i in 0..w {
                    let mid = (w - 1) / 2;
                    let mut shade = 0.2 + 0.6 * t;
                    if w >= 3 && (i == 0 || i == w - 1) {
                        shade -= 0.15;
                    } else if w >= 3 && i == mid {
                        shade += 0.22;
                    } else if i > mid {
                        shade += 0.05;
                    }
                    pic.put(lo + i, y, pick(&c.leaf, shade));
                }
            }
            Blade::Ribbon => {
                let twist = ((t * 8.7 + phase).sin() > 0.3) && t < 0.92 && t > 0.08;
                let shade = 0.3 + 0.6 * t;
                if twist {
                    pic.put(xc, y, pick(&c.leaf, shade - 0.12));
                    pic.put(xc + 1, y, pick(&c.leaf, shade + 0.1));
                } else {
                    pic.put(xc, y, pick(&c.leaf, shade));
                }
            }
        }
    }
}

/// A crown with blades fanning up from it and roots below.
fn rosette(pic: &mut Pic, c: &Ctx, f: &RosetteForm) {
    let crown_w = i32::from(f.crown) * 2 + 2;
    let lean = f32::from(f.lean);
    let blades = i32::from(f.blades).max(1);
    let root_len = (i32::from(f.root_depth) * 2 / 5).clamp(3, 6);
    let lotus = c.look.leaf == LeafShape::Lotus && f.pad > 0;
    // Roots under the crown, then the crown itself.
    roots(
        pic,
        c,
        0,
        0,
        if lotus { 4 } else { 3 },
        root_len,
        crown_w as f32 * 0.6,
        c.look.root == RootShape::Thick,
    );
    if lotus {
        bulb(pic, c);
    } else {
        for y in [-1, -2] {
            for x in -crown_w / 2..crown_w / 2 {
                let edge = x == -crown_w / 2 || x == crown_w / 2 - 1;
                let t = if y == -2 { 0.8 } else { 0.35 } - if edge { 0.2 } else { 0.0 };
                pic.put(x, y, pick(&c.stem, t));
            }
        }
    }
    let longest = if lotus {
        26
    } else {
        ((f32::from(f.blade_length) * 0.8).round() as i32).clamp(12, 30)
    };
    let spread = longest as f32 * 6.0 / (lean + 6.0);
    // Draw the outer blades first so the middle ones sit in front.
    let mut order: Vec<i32> = (0..blades).collect();
    order.sort_by_key(|&i| {
        let u = unit_of(i, blades);
        -((u.abs() * 100.0) as i32)
    });
    for i in order {
        let u = unit_of(i, blades);
        let height = (longest as f32 * (0.8 + 0.2 * (1.0 - u.abs())) - c.roll(i, 2, 3) as f32)
            .round() as i32;
        let x0 = (u * crown_w as f32 * 0.4).round() as i32;
        if lotus {
            let frac = [0.62, 1.0, 0.78, 0.9][i as usize % 4];
            lotus_leaf(
                pic,
                c,
                f,
                x0,
                (longest as f32 * frac).round() as i32,
                u * spread * 0.55,
                i,
            );
        } else if c.look.leaf == LeafShape::Ribbon {
            draw_blade(
                pic,
                c,
                x0,
                -2,
                (height + 2).min(30),
                u * spread * 0.9,
                Blade::Ribbon,
                i,
            );
        } else {
            let max = if f.blade_length < 20 { 4.0 } else { 5.0 };
            draw_blade(pic, c, x0, -2, height, u * spread, Blade::Sword { max }, i);
        }
    }
}

/// Where blade `i` of `n` leans: `-1` (left) to `1` (right).
fn unit_of(i: i32, n: i32) -> f32 {
    if n > 1 {
        (i as f32 / (n - 1) as f32) * 2.0 - 1.0
    } else {
        0.0
    }
}

/// The tiger lotus's brown bulb.
fn bulb(pic: &mut Pic, c: &Ctx) {
    let rows: [(i32, i32, f32); 4] = [(-4, 2, 0.8), (-3, 3, 0.65), (-2, 4, 0.45), (-1, 3, 0.2)];
    for (y, half, t) in rows {
        for x in -half..half {
            let edge = x == -half;
            pic.put(x, y, pick(&c.stem, t - if edge { 0.2 } else { 0.0 }));
        }
    }
}

/// A petiole of `height` rows leaning `drift` pixels, with a broad mottled
/// pad on its tip.
fn lotus_leaf(pic: &mut Pic, c: &Ctx, f: &RosetteForm, x0: i32, height: i32, drift: f32, id: i32) {
    let top = -4 - height;
    let mut tip = x0;
    for k in 0..height {
        let t = (k as f32 + 0.5) / height as f32;
        tip = x0 + (drift * t.powf(1.5)).round() as i32;
        pic.put(tip, -4 - k, pick(&c.stem, 0.3 + 0.7 * t));
    }
    let half = i32::from(f.pad) * 3 / 2;
    // Row half-widths of the pad, top to bottom, rounded at its ends.
    for (r, scale) in [0.72_f32, 1.0, 0.82].into_iter().enumerate() {
        let hw = ((half as f32) * scale).round() as i32;
        for dx in -hw..=hw {
            let x = tip + dx;
            let y = top + r as i32;
            let noise = c.unit(x + id * 17, y);
            let shade = match r {
                0 => {
                    if noise < 0.3 {
                        3
                    } else {
                        5 + i32::from(noise > 0.65) + i32::from(noise > 0.9)
                    }
                }
                1 => {
                    if noise < 0.45 {
                        2 + i32::from(noise < 0.2)
                    } else {
                        4 + i32::from(noise > 0.75)
                    }
                }
                _ => i32::from(noise < 0.5) + 1 + i32::from(noise > 0.85),
            };
            pic.put(x, y, at(&c.leaf, shade));
        }
    }
}

// ------------------------------------------------------------- carpets

/// Three low tufts joined by a runner, with tiny roots below.
fn carpet(pic: &mut Pic, c: &Ctx, f: &RosetteForm) {
    let tufts = [-14, 0, 14];
    // The runner just under the surface, and roots from each tuft.
    pic.hline(-15, 15, 1, pick(&c.root, 0.8));
    for (i, tx) in tufts.into_iter().enumerate() {
        roots(pic, c, tx, 1, 2, 3 + i32::from(i == 1), 2.0, false);
    }
    if c.look.leaf == LeafShape::Pad {
        for (i, tx) in tufts.into_iter().enumerate() {
            pads(pic, c, tx, i as i32, i32::from(f.blade_length));
        }
    } else {
        for (i, tx) in tufts.into_iter().enumerate() {
            grass(pic, c, tx, i as i32, i32::from(f.blade_length));
        }
    }
}

/// A tuft of fine blades fanning from `tx`.
fn grass(pic: &mut Pic, c: &Ctx, tx: i32, id: i32, blade_length: i32) {
    let blades = 6;
    let h = blade_length * 4 + 3;
    for i in 0..blades {
        let u = unit_of(i, blades);
        let height = h - (u.abs() * 3.0) as i32 + c.roll(id * 9 + i, 4, 4) - 1;
        for y in 0..height {
            let t = y as f32 / height as f32;
            let x = tx + (u * (0.5 + 8.5 * t.powf(0.9))).round() as i32;
            let shade = pick(
                &c.leaf,
                0.2 + 0.65 * t + if y == height - 1 { 0.15 } else { 0.0 },
            );
            pic.put(x, -y - 1, shade);
        }
    }
}

/// A tuft of short stalks tipped with round pads.
fn pads(pic: &mut Pic, c: &Ctx, tx: i32, id: i32, blade_length: i32) {
    let stalks = 5;
    for i in 0..stalks {
        let u = unit_of(i, stalks);
        let h = (blade_length * 3 + 2 + c.roll(id * 9 + i, 6, 4) - (u.abs() * 2.0) as i32).max(1);
        let sx = |y: i32| tx + (u * 6.0 * (0.3 + 0.7 * y as f32 / h as f32)).round() as i32;
        for y in 0..h {
            pic.put(
                sx(y),
                -y - 1,
                pick(&c.stem, 0.2 + 0.7 * y as f32 / h as f32),
            );
        }
        let x = sx(h);
        let y = -h - 1;
        // A pad: three wide on top, bright, over a shaded row.
        pic.hline(x - 1, x + 1, y - 1, pick(&c.leaf, 0.95));
        pic.hline(x - 2, x + 2, y, pick(&c.leaf, 0.75));
        pic.hline(x - 1, x + 1, y + 1, pick(&c.leaf, 0.45));
        // A small side pad at mid-height, alternating sides.
        let side = if i % 2 == 0 { -1 } else { 1 };
        let my = h / 2;
        let mx = sx(my) + side * 2;
        pic.hline(
            mx.min(mx + side),
            mx.max(mx + side),
            -my - 2,
            pick(&c.leaf, 0.7),
        );
        pic.hline(
            mx.min(mx + side),
            mx.max(mx + side),
            -my - 1,
            pick(&c.leaf, 0.4),
        );
    }
}

// ------------------------------------------------------------ floaters

/// Floating leaves along the top line with roots hanging below.
fn floater(pic: &mut Pic, c: &Ctx, f: &FloaterForm) {
    let len = (i32::from(f.root_length) * 2).clamp(4, 28);
    if c.look.leaf == LeafShape::Disc {
        let pads: [(i32, i32, i32); 4] = [(-15, 0, 12), (-4, -1, 10), (6, 0, 12), (16, -1, 10)];
        // Roots first, so the pads are in front of them.
        for (i, x) in [-9, 0, 10, 14]
            .into_iter()
            .take(i32::from(f.roots).max(1) as usize + 1)
            .enumerate()
        {
            hanging_root(pic, c, x, 2, len - c.roll(i as i32, 7, 3), i as i32);
        }
        for (x, y, w) in pads {
            disc(pic, c, x, y, w);
        }
    } else {
        let lenses = [(-16, 0), (-11, 0), (-4, -1), (1, 0), (8, 0), (13, -1)];
        for (i, &(x, y)) in lenses.iter().enumerate() {
            if i % 3 != 1 {
                hanging_root(
                    pic,
                    c,
                    x + 3,
                    y + 1,
                    len + 2 + c.roll(i as i32, 7, 2),
                    i as i32,
                );
            }
        }
        for (i, &(x, y)) in lenses.iter().enumerate() {
            lens(pic, c, x, y, i as i32);
        }
    }
}

/// A pale root hanging from row `y0`, `len` long, swaying a little.
fn hanging_root(pic: &mut Pic, c: &Ctx, x0: i32, y0: i32, len: i32, id: i32) {
    let phase = c.unit(id, 11) * 6.0;
    for k in 0..len {
        let t = k as f32 / len.max(1) as f32;
        let x = x0 + ((k as f32 * 0.4 + phase).sin() * 1.1).round() as i32;
        pic.put(x, y0 + k, pick(&c.root, 0.35 + 0.65 * t));
    }
}

/// A duckweed frond: a flat oval six pixels wide, lit on top.
fn lens(pic: &mut Pic, c: &Ctx, x: i32, y: i32, id: i32) {
    let bright = i32::from(c.roll(id, 13, 2) == 0);
    pic.hline(x + 1, x + 4, y - 2, at(&c.leaf, 4 + bright));
    pic.hline(x, x + 5, y - 1, at(&c.leaf, 3 + bright));
    pic.hline(x + 1, x + 4, y, at(&c.leaf, 1));
    pic.put(x + 2, y - 2, at(&c.leaf, 5));
}

/// A frogbit pad `w` wide: a lit rim, a pale vein, a dark spongy underside.
fn disc(pic: &mut Pic, c: &Ctx, cx: i32, y: i32, w: i32) {
    let half = w / 2;
    pic.hline(cx - half + 3, cx + half - 4, y - 3, at(&c.leaf, 5));
    pic.hline(cx - half + 1, cx + half - 2, y - 2, at(&c.leaf, 4));
    pic.hline(cx - half, cx + half - 1, y - 1, at(&c.leaf, 3));
    for x in cx - half + 2..cx + half - 2 {
        pic.put(x, y, at(&c.leaf, i32::from(x % 2 == 0)));
    }
    // The vein and a glint.
    pic.hline(cx - half + 2, cx + 1, y - 2, at(&c.leaf, 5));
    pic.put(cx - half + 4, y - 3, at(&c.leaf, 5));
}

// ----------------------------------------------------------- epiphytes

/// A creeping rhizome with rootlets under it and leaves along it.
fn epiphyte(pic: &mut Pic, c: &Ctx, f: &EpiphyteForm) {
    let half = (i32::from(f.rhizome_length) * 3 / 2 / 2).max(4);
    let thick = c.look.stem == StemShape::Thick;
    // The leaves first, so the rhizome is in front of their stalks.
    let leaves = if c.look.leaf == LeafShape::Pad { 4 } else { 5 };
    let lean = f32::from(f.lean);
    for i in 0..leaves {
        let u = unit_of(i, leaves);
        let x0 = (u * half as f32 * 0.8).round() as i32;
        let long = i32::from(f.leaf_length);
        if c.look.leaf == LeafShape::Pad {
            let h = (long * 2 + 2 + c.roll(i, 3, 4) - (u.abs() * 2.0) as i32).clamp(8, 24);
            let drift = u * h as f32 * 0.6 * 6.0 / (lean + 6.0);
            anubias_leaf(pic, c, x0, h, drift);
        } else {
            let h = ((long as f32 * 1.5).round() as i32 - c.roll(i, 3, 4) - (u.abs() * 3.0) as i32)
                .clamp(8, 28);
            let drift = u * h as f32 * 0.7 * 6.0 / (lean + 6.0) + u * 2.0;
            draw_blade(pic, c, x0, -2, h, drift, Blade::Sword { max: 3.0 }, i);
        }
    }
    // The rhizome.
    for x in -half..=half {
        pic.put(x, -1, pick(&c.stem, 0.75));
        if thick {
            pic.put(x, 0, pick(&c.stem, 0.25));
            pic.put(x, -2, pick(&c.stem, 0.9));
        }
    }
    // Rootlets clinging below.
    let base = i32::from(thick);
    for x in (-half + 1..=half).step_by(3) {
        let n = 2 + c.roll(x, 6, 3);
        for k in 1..=n {
            let dx = if k > 1 { c.roll(x, 7, 3) - 1 } else { 0 };
            pic.put(
                x + dx * i32::from(k > 1),
                base + k,
                pick(&c.root, 0.3 + 0.5 * k as f32 / n as f32),
            );
        }
    }
}

/// An anubias leaf: a stalk to a broad, waxy, rounded blade with a pointed
/// tip and a midrib.
fn anubias_leaf(pic: &mut Pic, c: &Ctx, x0: i32, h: i32, drift: f32) {
    let (rx, ry) = (4.8_f32, 4.2_f32);
    let stalk = h - 8;
    let mut tip = x0;
    for k in 0..stalk {
        let t = k as f32 / stalk as f32;
        tip = x0 + (drift * t.powf(1.3)).round() as i32;
        pic.put(tip, -2 - k, pick(&c.stem, 0.3 + 0.5 * t));
    }
    let cy = -2 - stalk - 4;
    for dy in -5..=4 {
        for dx in -5..=5 {
            let d = (dx as f32 / rx).powi(2) + ((dy as f32 + 0.5) / ry).powi(2);
            let pointed = dy == -5 && dx == 0;
            if d <= 1.0 || pointed {
                let mut t = 0.55 - 0.3 * (dy as f32 / ry) - 0.1 * (dx as f32 / rx);
                if d > 0.65 {
                    t -= 0.18;
                }
                if dx == 0 {
                    t += 0.2;
                }
                pic.put(tip + dx, cy + dy, pick(&c.leaf, t));
            }
        }
    }
}

// ---------------------------------------------------------------- mats

/// A mound of moss cushioning the base: low and ragged for `Fuzz`, thicker
/// and layered for `Frond`.
fn mat(pic: &mut Pic, c: &Ctx, f: &MatForm) {
    let layers = i32::from(f.layers).max(1);
    let half = 22;
    let top = if c.look.stem == StemShape::Frond {
        layers * 7
    } else {
        10
    };
    // A mound: the height at column `x`.
    let mound = |x: i32, id: i32| -> i32 {
        let s = (x as f32 / half as f32).abs().min(1.0);
        let body = (1.0 - s * s).max(0.0).powf(0.55);
        ((top as f32 * body) + c.unit(x, id) * 2.0 - 0.5).round() as i32
    };
    if c.look.stem == StemShape::Frond {
        // The dark body under the fronds, then the layers from the lowest.
        for x in -half..=half {
            for y in 1..=(mound(x, 0) - 4).max(1) {
                pic.put(x, -y, pick(&c.stem, 0.05 + 0.15 * c.unit(x, y)));
            }
        }
        for l in 0..layers {
            let frac = (l + 1) as f32 / layers as f32;
            let mut x = -half + 2 + (l * 2) % 5;
            while x < half {
                let apex = (mound(x, l) as f32 * (0.35 + 0.65 * frac)).round() as i32;
                frond(pic, c, x, apex.max(5), l as f32 / layers as f32);
                x += 5;
            }
        }
    } else {
        for x in -half..=half {
            let h = mound(x, 0).max(1);
            let edge = (x.abs() as f32 / half as f32) > 0.78;
            for y in 1..=h {
                let t = y as f32 / h as f32;
                let noise = c.unit(x, y);
                // A ragged fringe: the outer columns and the top rows drop
                // pixels.
                if ((edge || y == h) && noise < 0.4) || (y == h - 1 && noise < 0.15) {
                    continue;
                }
                let shade = 0.1 + 0.55 * t + 0.25 * noise + if y == h { 0.15 } else { 0.0 };
                pic.put(x, -y, pick(&c.stem, shade));
            }
        }
        // Lighter growing tips poking above it.
        for x in (-half + 3..half - 2).step_by(3) {
            let h = mound(x, 0).max(1);
            if c.roll(x, 8, 3) != 0 {
                pic.put(x + c.roll(x, 9, 2), -h - 1, pick(&c.stem, 1.0));
            }
        }
    }
}

/// A downswept triangular frond with its apex at height `apex` above column
/// `x`, in the ramp's band `band` (0 darkest layer, towards 1).
fn frond(pic: &mut Pic, c: &Ctx, x: i32, apex: i32, band: f32) {
    const DEPTH: i32 = 6;
    for d in 0..=DEPTH {
        let half = (d as f32 * 0.85).round() as i32;
        for dx in -half..=half {
            let edge = (dx.abs() == half && d > 1) || d == DEPTH;
            let mut t = 0.3 * band + 0.6 * (1.0 - d as f32 / DEPTH as f32);
            if d <= 1 {
                t += 0.2;
            }
            if edge {
                t -= 0.3;
            }
            pic.put(x + dx, -apex + d, pick(&c.stem, t));
        }
    }
}

// ----------------------------------------------------------------- ball

/// A round, velvety ball, lit from above and shaded below.
fn ball(pic: &mut Pic, c: &Ctx) {
    let r = 9.4_f32;
    let cy = -9.0_f32;
    let light = [-0.35_f32, -0.8, 0.5];
    let norm = (light[0] * light[0] + light[1] * light[1] + light[2] * light[2]).sqrt();
    for y in -19..=1 {
        for x in -10..=10 {
            let (dx, dy) = (x as f32, y as f32 - cy);
            let d = (dx * dx + dy * dy).sqrt();
            let noise = c.unit(x, y);
            if d > r + 0.9 {
                continue;
            }
            if d > r {
                // A few fibres standing proud of the edge.
                if noise < 0.3 {
                    pic.put(x, y, pick(&c.stem, 0.1));
                }
                continue;
            }
            let (nx, ny) = (dx / r, dy / r);
            let nz = (1.0 - nx * nx - ny * ny).max(0.0).sqrt();
            let diffuse = (nx * light[0] + ny * light[1] + nz * light[2]) / norm;
            let mut t = 0.1 + 0.85 * (diffuse * 0.95 + 0.15).clamp(0.0, 1.0);
            t += (noise - 0.5) * 0.14;
            if d > r - 1.0 {
                t -= 0.18;
            }
            if c.roll(x, y + 40, 9) == 0 {
                t += 0.12;
            }
            pic.put(x, y, pick(&c.stem, t));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktank_sim::{MaterialTable, PlantSpeciesTable};
    use std::collections::HashSet;

    /// A crop rectangle (x, y, width, height) in cells.
    type Crop = (usize, usize, usize, usize);

    fn table() -> PlantSpeciesTable {
        let m = MaterialTable::builtin().unwrap();
        PlantSpeciesTable::builtin_for(&m).unwrap()
    }

    fn all(t: &PlantSpeciesTable) -> Vec<(&PlantSpecies, Sprite)> {
        t.iter().map(|sp| (sp, build(sp))).collect()
    }

    #[test]
    fn every_shipped_species_has_a_cropped_sprite_within_bounds() {
        let t = table();
        assert_eq!(t.len(), 18);
        for (sp, ((w, h), px)) in all(&t) {
            assert!(w >= 8 && h >= 8, "{}: {w}×{h}", sp.name);
            assert!(w <= MAX_WIDTH && h <= MAX_HEIGHT, "{}: {w}×{h}", sp.name);
            assert_eq!(px.len(), w * h, "{}", sp.name);
            assert!(px.iter().all(|p| p[3] == 0 || p[3] == 255), "{}", sp.name);
            // Cropped: something is drawn on every edge.
            let opaque = |x: usize, y: usize| px[y * w + x][3] == 255;
            assert!((0..w).any(|x| opaque(x, 0)), "{}: top row", sp.name);
            assert!((0..w).any(|x| opaque(x, h - 1)), "{}: bottom row", sp.name);
            assert!((0..h).any(|y| opaque(0, y)), "{}: left column", sp.name);
            assert!(
                (0..h).any(|y| opaque(w - 1, y)),
                "{}: right column",
                sp.name
            );
            let filled = px.iter().filter(|p| p[3] == 255).count();
            assert!(filled >= 25, "{}: {filled} pixels", sp.name);
        }
    }

    #[test]
    fn sprites_are_deterministic() {
        let t = table();
        for sp in t.iter() {
            assert_eq!(build(sp), build(sp), "{}", sp.name);
        }
    }

    /// A whorled species (the data option no shipped species uses) draws as
    /// stacked discs, an image of its own that still fits the pane.
    #[test]
    fn a_whorled_species_draws_discs_within_bounds() {
        let t = table();
        let cabomba = t.iter().find(|sp| sp.name == "cabomba").unwrap();
        let mut whorled = cabomba.clone();
        whorled.look.arrangement = Arrangement::Whorl;
        let ((w, h), px) = build(&whorled);
        assert!(w <= MAX_WIDTH && h <= MAX_HEIGHT, "{w}x{h}");
        assert!(px.iter().any(|p| p[3] == 255));
        assert_ne!(px, build(cabomba).1, "a whorl isn't a pair of fans");
    }

    #[test]
    fn every_species_is_a_distinct_image() {
        let t = table();
        let sprites = all(&t);
        for (i, (a, sa)) in sprites.iter().enumerate() {
            for (b, sb) in &sprites[i + 1..] {
                assert_ne!(sa, sb, "{} and {} draw the same sprite", a.name, b.name);
            }
        }
        // Different forms differ in shape, not only in colour: no two
        // species share an outline.
        let outlines: HashSet<_> = sprites
            .iter()
            .map(|(_, ((w, h), px))| (*w, *h, px.iter().map(|p| p[3]).collect::<Vec<_>>()))
            .collect();
        assert_eq!(
            outlines.len(),
            sprites.len(),
            "two species share an outline"
        );
    }

    #[test]
    fn sprites_use_only_the_species_colours() {
        let t = table();
        for (sp, (_, px)) in all(&t) {
            let mut allowed: HashSet<Rgb> = HashSet::new();
            allowed.extend(&sp.colours.root);
            allowed.extend(&sp.colours.stem);
            allowed.extend(&sp.colours.leaf);
            allowed.extend(tinted_leaf(sp).unwrap_or_default());
            let mut used: HashSet<Rgb> = HashSet::new();
            for p in px.iter().filter(|p| p[3] == 255) {
                let rgb = [p[0], p[1], p[2]];
                assert!(allowed.contains(&rgb), "{}: stray colour {rgb:?}", sp.name);
                used.insert(rgb);
            }
            assert!(used.len() >= 2, "{} is flat", sp.name);
        }
    }

    #[test]
    fn colouration_shows_on_the_species_that_have_it() {
        let t = table();
        let rotala = t.get(t.id("rotala").unwrap()).unwrap();
        let tip = tinted_leaf(rotala).unwrap();
        let (_, px) = build(rotala);
        let has = |shade: &Rgb| px.iter().any(|p| p[3] == 255 && p[..3] == shade[..]);
        assert!(tip.iter().any(has), "rotala's tips are tinted");
        // The tinted shades are not the plain ones, and they are pinker.
        assert!(tip.iter().all(|s| !rotala.colours.leaf.contains(s)));
        let (a, b) = (tip[tip.len() - 1], rotala.colours.leaf[tip.len() - 1]);
        assert!(a[0] > b[0] && a[0] > a[1]);
        // Species without one use their own ramps only.
        let hygro = t.get(t.id("hygrophila").unwrap()).unwrap();
        assert!(tinted_leaf(hygro).is_none());
    }

    #[test]
    fn the_forms_differ_in_size_as_their_data_do() {
        let t = table();
        let size = |name: &str| build(t.get(t.id(name).unwrap()).unwrap()).0;
        // A cryptocoryne is compact next to an amazon sword and a
        // vallisneria.
        assert!(size("cryptocoryne").1 + 8 <= size("amazon_sword").1);
        assert!(size("cryptocoryne").1 + 8 <= size("vallisneria").1);
        // Duckweed's roots are short, frogbit's long.
        assert!(size("duckweed").1 + 10 <= size("frogbit").1);
        // The marimo is round, a mat is low and wide.
        let (w, h) = size("marimo");
        assert!((17..=21).contains(&w) && (17..=21).contains(&h), "{w}×{h}");
        let (w, h) = size("java_moss");
        assert!(w > 2 * h, "{w}×{h}");
    }

    #[test]
    fn extreme_species_parameters_stay_within_bounds() {
        let t = table();
        let clone = |name: &str| t.get(t.id(name).unwrap()).unwrap().clone();

        // Floater with maximum allowed hanging roots.
        let mut frog = clone("frogbit");
        if let Form::Floater(ref mut f) = frog.form {
            f.root_length = desktank_sim::plants::MAX_HANGING_ROOT;
        }
        // Ribbon rosette with deep roots.
        let mut val = clone("vallisneria");
        if let Form::Rosette(ref mut f) = val.form {
            f.root_depth = 20;
        }
        // Epiphytes with tiny and maximum leaf lengths.
        let mut anubias_tiny = clone("anubias");
        if let Form::Epiphyte(ref mut f) = anubias_tiny.form {
            f.leaf_length = 1;
        }
        let mut anubias_huge = clone("anubias");
        if let Form::Epiphyte(ref mut f) = anubias_huge.form {
            f.leaf_length = desktank_sim::plants::MAX_POSITION;
        }
        let mut fern_huge = clone("java_fern");
        if let Form::Epiphyte(ref mut f) = fern_huge.form {
            f.leaf_length = desktank_sim::plants::MAX_POSITION;
        }
        // Carpet pad with zero blade length.
        let mut mc_zero = clone("monte_carlo");
        if let Form::Carpet(ref mut f) = mc_zero.form {
            f.blade_length = 0;
        }

        for sp in [frog, val, anubias_tiny, anubias_huge, fern_huge, mc_zero] {
            let ((w, h), px) = build(&sp);
            assert!(w > 0 && w <= MAX_WIDTH, "{}: {w}×{h}", sp.name);
            assert!(h > 0 && h <= MAX_HEIGHT, "{}: {w}×{h}", sp.name);
            assert_eq!(px.len(), w * h);
        }
    }

    /// Writes the sprites as PNGs to `target/shots/plant-*.png` when
    /// `CARD_SHOTS` is set, enlarged eight times on the card's background.
    #[test]
    fn writes_plant_shots_on_request() {
        if std::env::var_os("CARD_SHOTS").is_none() {
            return;
        }
        let t = table();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).unwrap();
        let bg = crate::hud::BACKGROUND;
        let sprites = all(&t);
        // Contact sheets of six sprites each, in a cell of the widest and
        // tallest, on the card's background.
        let (scale, cols, cell) = (6_usize, 3_usize, (92_usize, 42_usize));
        for (n, group) in sprites.chunks(6).enumerate() {
            let rows = group.len().div_ceil(cols);
            let (sw, sh) = (cols * cell.0 * scale, rows * cell.1 * scale);
            let mut big: Vec<u8> = (0..sw * sh)
                .flat_map(|_| [bg[0], bg[1], bg[2], 255])
                .collect();
            for (i, (_, ((w, h), px))) in group.iter().enumerate() {
                let (ox, oy) = (
                    (i % cols) * cell.0 + (cell.0 - w) / 2,
                    (i / cols) * cell.1 + (cell.1 - h) / 2,
                );
                for (j, p) in px.iter().enumerate().filter(|(_, p)| p[3] == 255) {
                    let (x, y) = ((ox + j % w) * scale, (oy + j / w) * scale);
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let k = ((y + dy) * sw + x + dx) * 4;
                            big[k..k + 4].copy_from_slice(p);
                        }
                    }
                }
            }
            std::fs::write(
                dir.join(format!("plants-{n}.png")),
                crate::testing::png(
                    &big,
                    (u32::try_from(sw).unwrap(), u32::try_from(sh).unwrap()),
                ),
            )
            .unwrap();
        }
    }

    /// Writes, when `STEM_SHOTS` or `ROSETTE_SHOTS` names a prefix (`before`,
    /// `after`; `SHOTS_MINUTE=0` draws at midnight), one PNG per stem (or rosette) species to
    /// `target/shots/stem-before-after/` (`rosette-before-after/`): its pane
    /// sprite, then the same species placed Mature in a tank at scale 2 and
    /// at scale 4 (quarter-cell art), all enlarged to the sprite's pixels,
    /// and a contact sheet of the species.
    #[test]
    fn writes_stem_before_after_on_request() {
        let rosettes = std::env::var_os("ROSETTE_SHOTS");
        let Some(prefix) = rosettes.clone().or_else(|| std::env::var_os("STEM_SHOTS")) else {
            return;
        };
        let rosettes = rosettes.is_some();
        let prefix = prefix.to_string_lossy().into_owned();
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let Ok(gpu) = pollster::block_on(crate::Gpu::new(
            &instance,
            None,
            crate::GpuOptions::default(),
        )) else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/shots")
            .join(if rosettes {
                "rosette-before-after"
            } else {
                "stem-before-after"
            });
        std::fs::create_dir_all(&dir).unwrap();
        let t = table();
        // A rosette's blades and a lotus's stalks reach 56 cells up and fan out.
        let (cw, ch) = if rosettes {
            (64_i64, 80_i64)
        } else {
            (48_i64, 64_i64)
        };
        let bg = crate::hud::BACKGROUND;
        // Tank pixels are shown at 4x (scale 2) or 2x (scale 4): the same size.
        let tank_shot = |name: &str,
                         scale: u32,
                         stage: desktank_sim::PlantStage,
                         tint: Option<bool>|
         -> (Vec<u8>, Option<(usize, usize)>) {
            let m = MaterialTable::builtin().unwrap();
            let mut world = desktank_sim::World::new(
                desktank_sim::WorldConfig {
                    width: cw as usize,
                    height: ch as usize,
                    seed: 7,
                    water_level: 0.95,
                },
                m,
            )
            .unwrap();
            // Shown in lean or fertilised water, the plant stands on a high bed so
            // its whole height is in strong light.
            let bed = if tint.is_some() {
                30
            } else if rosettes {
                74
            } else {
                58
            };
            let soil = world.materials().id("aquasoil").unwrap();
            world
                .fill_rect(
                    desktank_sim::Rect {
                        x0: 0,
                        y0: bed,
                        x1: cw as usize,
                        y1: ch as usize,
                    },
                    soil,
                    desktank_sim::Brush::Replace,
                )
                .unwrap();
            let id = world.plant_species().id(name).unwrap();
            world
                .place_plant(id, stage, cw / 2, bed as i64 - 1, false)
                .unwrap()
                .unwrap();
            // The lowest stem cell off the main stem: where a side shoot starts.
            let stem = world.materials().id("stem").unwrap();
            let stub = (0..ch as usize)
                .rev()
                .flat_map(|y| (0..cw as usize).map(move |x| (x, y)))
                .find(|&(x, y)| x != cw as usize / 2 && world.cell(x, y).unwrap().material == stem);
            // `Some(true)`: a lean tank, so the plant colours up fully;
            // `Some(false)`: a fertilised one, so it doesn't colour at all.
            if let Some(lean) = tint {
                // Hold the nutrient at 0.3 (colouration 1) or 1.5 (none).
                let want = if lean { 0.3 } else { 1.5 };
                for _ in 0..4800 {
                    for y in (0..ch as usize).step_by(8) {
                        for x in (0..cw as usize).step_by(8) {
                            let now = world.field(desktank_sim::FieldKind::Nutrient).at_cell(x, y);
                            world.add_to_field(desktank_sim::FieldKind::Nutrient, x, y, want - now);
                        }
                    }
                    world.tick();
                }
                let c = world.plants().next().map_or(-1.0, |p| p.colour);
                eprintln!(
                    "rotala colouration {c:.2} ({})",
                    if lean { "lean" } else { "fed" }
                );
            }
            let mut tank = crate::Tank::new(world, crate::TankConfig::default());
            // `SHOTS_MINUTE`: the minute of the day to draw at (noon by default).
            let minute = std::env::var("SHOTS_MINUTE")
                .ok()
                .and_then(|m| m.parse().ok())
                .unwrap_or(12 * 60);
            tank.set_time_of_day(minute);
            tank.step();
            (
                tank.capture(&gpu, (cw as u32 * scale, ch as u32 * scale))
                    .unwrap(),
                stub,
            )
        };
        let species: Vec<&PlantSpecies> = t
            .iter()
            .filter(|sp| {
                if rosettes {
                    matches!(sp.form, Form::Rosette(_))
                } else {
                    matches!(sp.form, Form::Stem(_))
                }
            })
            .collect();
        let mut sheet: Vec<(Vec<u8>, usize, usize)> = Vec::new();
        for sp in species {
            let ((sw, sh), px) = build(sp);
            // Enlarge `rgba` (`w` wide), cropped to `crop` (x0, y0, w, h),
            // `k` times.
            let up = |rgba: &[u8], w: usize, crop: (usize, usize, usize, usize), k: usize| {
                let (cx, cy, cw, ch) = crop;
                let mut out = vec![0u8; cw * k * ch * k * 4];
                for y in 0..ch * k {
                    for x in 0..cw * k {
                        let s = ((cy + y / k) * w + cx + x / k) * 4;
                        let o = (y * cw * k + x) * 4;
                        out[o..o + 4].copy_from_slice(&rgba[s..s + 4]);
                    }
                }
                out
            };
            let blit = |img: &mut Vec<u8>,
                        total_w: usize,
                        src: &[u8],
                        (w, h): (usize, usize),
                        (x0, y0): (usize, usize)| {
                for y in 0..h {
                    let o = ((y0 + y) * total_w + x0) * 4;
                    img[o..o + w * 4].copy_from_slice(&src[y * w * 4..(y + 1) * w * 4]);
                }
            };
            let write = |name: String, img: &[u8], (w, h): (usize, usize)| {
                std::fs::write(
                    dir.join(name),
                    crate::testing::png(
                        img,
                        (u32::try_from(w).unwrap(), u32::try_from(h).unwrap()),
                    ),
                )
                .unwrap();
            };
            let spr: Vec<u8> = px
                .iter()
                .flat_map(|p| {
                    if p[3] == 255 {
                        *p
                    } else {
                        [bg[0], bg[1], bg[2], 255]
                    }
                })
                .collect();
            let (mature, young) = (
                desktank_sim::PlantStage::Mature,
                desktank_sim::PlantStage::Juvenile,
            );
            let (two, _) = tank_shot(&sp.name, 2, mature, None);
            let (four, stub) = tank_shot(&sp.name, 4, mature, None);
            let (two_j, _) = tank_shot(&sp.name, 2, young, None);
            let (four_j, _) = tank_shot(&sp.name, 4, young, None);
            let (cwu, chu) = (cw as usize, ch as usize);
            // The whole plant: sprite and tank at the same pixel size (the
            // scale-2 tank shows a texel as a pixel, as the sprite does),
            // then the quarter-cell tank at half the zoom.
            let (x0, y0, pw, ph) = if rosettes {
                (cwu / 2 - 24, 4, 48, chu - 4)
            } else {
                (cwu / 2 - 16, 6, 32, chu - 6)
            };
            let (k, gap) = if rosettes { (4, 12) } else { (6, 16) };
            let (spr_w, spr_h) = (sw * k, sh * k);
            let tank_w = pw * 2 * k;
            let panel_h = ph * 2 * k;
            let total_w = spr_w + 4 * (gap + tank_w);
            let mut img: Vec<u8> = (0..total_w * panel_h)
                .flat_map(|_| [bg[0], bg[1], bg[2], 255])
                .collect();
            let spr_up = up(&spr, sw, (0, 0, sw, sh), k);
            blit(
                &mut img,
                total_w,
                &spr_up,
                (spr_w, spr_h),
                (0, panel_h - spr_h - 4 * k),
            );
            let a = up(&two, cwu * 2, (x0 * 2, y0 * 2, pw * 2, ph * 2), k);
            blit(&mut img, total_w, &a, (tank_w, panel_h), (spr_w + gap, 0));
            let b = up(&four, cwu * 4, (x0 * 4, y0 * 4, pw * 4, ph * 4), k / 2);
            blit(
                &mut img,
                total_w,
                &b,
                (tank_w, panel_h),
                (spr_w + gap + tank_w + gap, 0),
            );
            // The same plant placed Juvenile, to show a growing one.
            for (n, shot, c, kk) in [(2, &two_j, 2, k), (3, &four_j, 4, k / 2)] {
                let b = up(shot, cwu * c, (x0 * c, y0 * c, pw * c, ph * c), kk);
                let at = spr_w + n * (gap + tank_w) + gap;
                blit(&mut img, total_w, &b, (tank_w, panel_h), (at, 0));
            }
            write(
                format!("{prefix}-{}.png", sp.name),
                &img,
                (total_w, panel_h),
            );
            if rosettes {
                // Sprite, scale 2 and scale 4 only (not the juveniles).
                let w3 = spr_w + 2 * (gap + tank_w);
                let mut row = vec![0u8; w3 * panel_h * 4];
                for y in 0..panel_h {
                    row[y * w3 * 4..(y + 1) * w3 * 4]
                        .copy_from_slice(&img[y * total_w * 4..y * total_w * 4 + w3 * 4]);
                }
                sheet.push((row, w3, panel_h));
            }
            // A close look at a side shoot's lean, where the plant has one.
            if let (false, Some((sx, sy))) = (rosettes, stub) {
                let (bw, bh, bk) = (20, 16, 10);
                let bx = sx.saturating_sub(10).min(cwu - bw);
                let by = sy.saturating_sub(13).min(chu - bh);
                let b = up(&four, cwu * 4, (bx * 4, by * 4, bw * 4, bh * 4), bk);
                write(
                    format!("{prefix}-{}-branch.png", sp.name),
                    &b,
                    (bw * 4 * bk, bh * 4 * bk),
                );
            }
            // A close look at the plant's upper half (a rosette's, its crown
            // and its blade tips or pads).
            let crops: &[(&str, Crop)] = if rosettes {
                &[
                    ("zoom", (cwu / 2 - 14, chu - 26, 28, 22)),
                    ("top", (cwu / 2 - 20, 8, 40, 30)),
                ]
            } else {
                &[("zoom", (cwu / 2 - 14, 14, 28, 20))]
            };
            for &(label, (zx, zy, zw, zh)) in crops {
                let zk = 8;
                let a = up(&two, cwu * 2, (zx * 2, zy * 2, zw * 2, zh * 2), zk * 2);
                let b = up(&four, cwu * 4, (zx * 4, zy * 4, zw * 4, zh * 4), zk);
                let zw_px = zw * 2 * zk * 2;
                let zh_px = zh * 2 * zk * 2;
                let mut img: Vec<u8> = (0..(zw_px * 2 + gap) * zh_px)
                    .flat_map(|_| [bg[0], bg[1], bg[2], 255])
                    .collect();
                let tw = zw_px * 2 + gap;
                blit(&mut img, tw, &a, (zw_px, zh_px), (0, 0));
                blit(&mut img, tw, &b, (zw_px, zh_px), (zw_px + gap, 0));
                write(
                    format!("{prefix}-{}-{label}.png", sp.name),
                    &img,
                    (tw, zh_px),
                );
            }
        }
        if rosettes {
            let w = sheet.iter().map(|s| s.1).max().unwrap_or(0);
            let h: usize = sheet.iter().map(|s| s.2).sum();
            let mut img: Vec<u8> = (0..w * h)
                .flat_map(|_| [bg[0], bg[1], bg[2], 255])
                .collect();
            let mut y0 = 0;
            for (row, rw, rh) in &sheet {
                for y in 0..*rh {
                    let o = ((y0 + y) * w) * 4;
                    img[o..o + rw * 4].copy_from_slice(&row[y * rw * 4..(y + 1) * rw * 4]);
                }
                y0 += rh;
            }
            std::fs::write(
                dir.join(format!("{prefix}-contact-sheet.png")),
                crate::testing::png(&img, (u32::try_from(w).unwrap(), u32::try_from(h).unwrap())),
            )
            .unwrap();
            return;
        }
        // Rotala's crown at colouration 0 (fertilised) and 1 (lean water),
        // side by side, upper half, quarter-cell art.
        let (fed, _) = tank_shot("rotala", 4, desktank_sim::PlantStage::Mature, Some(false));
        let (lean, _) = tank_shot("rotala", 4, desktank_sim::PlantStage::Mature, Some(true));
        let (cwu, chu) = (cw as usize, ch as usize);
        let (zx, zy, zw, zh, zk) = (cwu / 2 - 14, 0, 28, 30, 8);
        let crop = |img: &[u8]| {
            let mut out = vec![0u8; zw * 4 * zk * zh * 4 * zk * 4];
            for y in 0..zh * 4 * zk {
                for x in 0..zw * 4 * zk {
                    let s = (((zy * 4 + y / zk) * cwu * 4) + zx * 4 + x / zk) * 4;
                    let o = (y * zw * 4 * zk + x) * 4;
                    out[o..o + 4].copy_from_slice(&img[s..s + 4]);
                }
            }
            out
        };
        let (a, b) = (crop(&fed), crop(&lean));
        let _ = chu;
        let half = zw * 4 * zk;
        let gap = 16;
        let mut img = vec![0u8; (half * 2 + gap) * zh * 4 * zk * 4];
        for y in 0..zh * 4 * zk {
            let row = (y * (half * 2 + gap)) * 4;
            img[row..row + half * 4].copy_from_slice(&a[y * half * 4..(y + 1) * half * 4]);
            let at = row + (half + gap) * 4;
            img[at..at + half * 4].copy_from_slice(&b[y * half * 4..(y + 1) * half * 4]);
        }
        std::fs::write(
            dir.join(format!("{prefix}-rotala-colour.png")),
            crate::testing::png(
                &img,
                (
                    u32::try_from(half * 2 + gap).unwrap(),
                    u32::try_from(zh * 4 * zk).unwrap(),
                ),
            ),
        )
        .unwrap();
    }
}
