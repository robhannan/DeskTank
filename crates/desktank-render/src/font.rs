//! A tiny pixel font for text drawn into CPU-built images (the species card,
//! and later the in-tank inspect card).
//!
//! Glyphs are 3 pixels wide and 6 tall: capitals and digits fill rows 0–4
//! (the baseline is row 4), lower case has a 4-row body (rows 1–4) with
//! ascenders reaching row 0 and descenders row 5. Letters are one pixel
//! apart ([`ADVANCE`] per glyph) and lines [`LINE`] apart. Hand-drawn as
//! const string bitmaps, like the HUD's tool icons (`#` = ink). Characters
//! without a glyph draw a hollow box ([`glyph`] returns `None` for them, so
//! tests can assert nothing a shipped string uses falls back).
//!
//! Text goes into a tightly packed RGBA image through [`draw`]; [`width`]
//! measures a string and [`wrap`] breaks one at spaces to a pixel width.

/// Glyph width in pixels.
pub const GLYPH_W: u32 = 3;
/// Pixels from one glyph's left edge to the next's: the glyph and a gap.
pub const ADVANCE: u32 = GLYPH_W + 1;
/// Pixels from one line's top to the next's (a 6-row glyph and 2 gap rows).
pub const LINE: u32 = 8;

type Glyph = [&'static str; 6];

/// The hollow box drawn for a character without a glyph.
const FALLBACK: Glyph = ["###", "#.#", "#.#", "#.#", "###", "..."];

/// ASCII 32 (space) to 126 (`~`), in order.
const ASCII: [Glyph; 95] = [
    ["...", "...", "...", "...", "...", "..."], // space
    [".#.", ".#.", ".#.", "...", ".#.", "..."], // !
    ["#.#", "#.#", "...", "...", "...", "..."], // "
    ["#.#", "###", "#.#", "###", "#.#", "..."], // #
    [".##", "##.", ".#.", ".##", "##.", "..."], // $
    ["#.#", "..#", ".#.", "#..", "#.#", "..."], // %
    ["##.", "##.", ".##", "#.#", ".##", "..."], // &
    [".#.", ".#.", "...", "...", "...", "..."], // '
    [".#.", "#..", "#..", "#..", ".#.", "..."], // (
    [".#.", "..#", "..#", "..#", ".#.", "..."], // )
    ["#.#", ".#.", "###", ".#.", "#.#", "..."], // *
    ["...", ".#.", "###", ".#.", "...", "..."], // +
    ["...", "...", "...", "...", ".#.", "#.."], // ,
    ["...", "...", "###", "...", "...", "..."], // -
    ["...", "...", "...", "...", ".#.", "..."], // .
    ["..#", "..#", ".#.", "#..", "#..", "..."], // /
    ["###", "#.#", "#.#", "#.#", "###", "..."], // 0
    [".#.", "##.", ".#.", ".#.", "###", "..."], // 1
    ["###", "..#", "###", "#..", "###", "..."], // 2
    ["###", "..#", "###", "..#", "###", "..."], // 3
    ["#.#", "#.#", "###", "..#", "..#", "..."], // 4
    ["###", "#..", "###", "..#", "###", "..."], // 5
    ["###", "#..", "###", "#.#", "###", "..."], // 6
    ["###", "..#", "..#", ".#.", ".#.", "..."], // 7
    ["###", "#.#", "###", "#.#", "###", "..."], // 8
    ["###", "#.#", "###", "..#", "###", "..."], // 9
    ["...", ".#.", "...", ".#.", "...", "..."], // :
    ["...", ".#.", "...", "...", ".#.", "#.."], // ;
    ["..#", ".#.", "#..", ".#.", "..#", "..."], // <
    ["...", "###", "...", "###", "...", "..."], // =
    ["#..", ".#.", "..#", ".#.", "#..", "..."], // >
    ["###", "..#", ".##", "...", ".#.", "..."], // ?
    ["###", "#.#", "###", "#..", "###", "..."], // @
    [".#.", "#.#", "###", "#.#", "#.#", "..."], // A
    ["##.", "#.#", "##.", "#.#", "##.", "..."], // B
    [".##", "#..", "#..", "#..", ".##", "..."], // C
    ["##.", "#.#", "#.#", "#.#", "##.", "..."], // D
    ["###", "#..", "##.", "#..", "###", "..."], // E
    ["###", "#..", "##.", "#..", "#..", "..."], // F
    [".##", "#..", "#.#", "#.#", ".##", "..."], // G
    ["#.#", "#.#", "###", "#.#", "#.#", "..."], // H
    ["###", ".#.", ".#.", ".#.", "###", "..."], // I
    ["..#", "..#", "..#", "#.#", ".#.", "..."], // J
    ["#.#", "#.#", "##.", "#.#", "#.#", "..."], // K
    ["#..", "#..", "#..", "#..", "###", "..."], // L
    ["#.#", "###", "###", "#.#", "#.#", "..."], // M
    ["##.", "#.#", "#.#", "#.#", "#.#", "..."], // N
    [".#.", "#.#", "#.#", "#.#", ".#.", "..."], // O
    ["##.", "#.#", "##.", "#..", "#..", "..."], // P
    [".#.", "#.#", "#.#", "###", ".##", "..."], // Q
    ["##.", "#.#", "##.", "#.#", "#.#", "..."], // R
    [".##", "#..", ".#.", "..#", "##.", "..."], // S
    ["###", ".#.", ".#.", ".#.", ".#.", "..."], // T
    ["#.#", "#.#", "#.#", "#.#", "###", "..."], // U
    ["#.#", "#.#", "#.#", "#.#", ".#.", "..."], // V
    ["#.#", "#.#", "###", "###", "#.#", "..."], // W
    ["#.#", "#.#", ".#.", "#.#", "#.#", "..."], // X
    ["#.#", "#.#", ".#.", ".#.", ".#.", "..."], // Y
    ["###", "..#", ".#.", "#..", "###", "..."], // Z
    ["##.", "#..", "#..", "#..", "##.", "..."], // [
    ["#..", "#..", ".#.", "..#", "..#", "..."], // \
    [".##", "..#", "..#", "..#", ".##", "..."], // ]
    [".#.", "#.#", "...", "...", "...", "..."], // ^
    ["...", "...", "...", "...", "###", "..."], // _
    ["#..", ".#.", "...", "...", "...", "..."], // `
    ["...", ".##", "#.#", "#.#", ".##", "..."], // a
    ["#..", "##.", "#.#", "#.#", "##.", "..."], // b
    ["...", ".##", "#..", "#..", ".##", "..."], // c
    ["..#", ".##", "#.#", "#.#", ".##", "..."], // d
    ["...", ".#.", "#.#", "##.", ".##", "..."], // e
    [".##", "#..", "##.", "#..", "#..", "..."], // f
    ["...", ".##", "#.#", ".##", "..#", "##."], // g
    ["#..", "#..", "##.", "#.#", "#.#", "..."], // h
    [".#.", "...", ".#.", ".#.", ".#.", "..."], // i
    ["..#", "...", "..#", "..#", "..#", "##."], // j
    ["#..", "#.#", "##.", "#.#", "#.#", "..."], // k
    ["##.", ".#.", ".#.", ".#.", ".##", "..."], // l
    ["...", "...", "###", "###", "#.#", "..."], // m
    ["...", "##.", "#.#", "#.#", "#.#", "..."], // n
    ["...", ".#.", "#.#", "#.#", ".#.", "..."], // o
    ["...", "##.", "#.#", "##.", "#..", "#.."], // p
    ["...", ".##", "#.#", ".##", "..#", "..#"], // q
    ["...", "##.", "#.#", "#..", "#..", "..."], // r
    ["...", ".##", "#..", "..#", "##.", "..."], // s
    [".#.", "###", ".#.", ".#.", ".##", "..."], // t
    ["...", "#.#", "#.#", "#.#", "###", "..."], // u
    ["...", "#.#", "#.#", "#.#", ".#.", "..."], // v
    ["...", "#.#", "#.#", "###", "###", "..."], // w
    ["...", "#.#", ".#.", "#.#", "#.#", "..."], // x
    ["...", "#.#", "#.#", ".##", "..#", "##."], // y
    ["...", "###", "..#", "#..", "###", "..."], // z
    [".##", ".#.", "#..", ".#.", ".##", "..."], // {
    [".#.", ".#.", ".#.", ".#.", ".#.", "..."], // |
    ["##.", ".#.", "..#", ".#.", "##.", "..."], // }
    ["...", ".##", "##.", "...", "...", "..."], // ~
];

/// Characters beyond ASCII that shipped species text uses (and a few
/// near neighbours that a data file may reasonably want).
const EXTRA: [(char, Glyph); 6] = [
    // A subscript two, as in CO₂.
    ('₂', ["...", "...", "...", "##.", ".#.", "###"]),
    ('–', ["...", "...", "###", "...", "...", "..."]),
    ('’', [".#.", "#..", "...", "...", "...", "..."]),
    ('é', [".#.", "...", ".##", "###", ".##", "..."]),
    ('°', ["##.", "##.", "...", "...", "...", "..."]),
    ('×', ["...", "#.#", ".#.", "#.#", "...", "..."]),
];

/// The glyph for `c`, or `None` if the font has none.
#[must_use]
pub fn glyph(c: char) -> Option<&'static Glyph> {
    if (' '..='~').contains(&c) {
        ASCII.get(usize::try_from(u32::from(c) - 32).ok()?)
    } else {
        EXTRA.iter().find(|(e, _)| *e == c).map(|(_, g)| g)
    }
}

/// Number of characters in `text`, saturated into `u32`.
fn count(text: &str) -> u32 {
    u32::try_from(text.chars().count()).unwrap_or(u32::MAX)
}

/// Width of `text` in pixels, with no gap after its last glyph.
#[must_use]
pub fn width(text: &str) -> u32 {
    match count(text) {
        0 => 0,
        n => n.saturating_mul(ADVANCE) - (ADVANCE - GLYPH_W),
    }
}

/// How many characters fit in `pixels` (a text of that many is at most
/// `pixels` wide).
#[must_use]
pub fn columns(pixels: u32) -> u32 {
    (pixels + (ADVANCE - GLYPH_W)) / ADVANCE
}

/// Breaks `text` into lines at most `max_width` pixels wide, at spaces; a
/// word wider than a line is split between characters. Runs of spaces
/// collapse to one and lines never start or end with one. Empty text gives
/// no lines.
#[must_use]
pub fn wrap(text: &str, max_width: u32) -> Vec<String> {
    let limit = usize::try_from(columns(max_width))
        .unwrap_or(usize::MAX)
        .max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    let mut len = 0;
    for word in text.split_whitespace() {
        let mut word_len = word.chars().count();
        let mut word = word;
        // Start a new line when the word doesn't fit after the current text.
        if len > 0 && len + 1 + word_len > limit {
            lines.push(core::mem::take(&mut line));
            len = 0;
        }
        // A word longer than a whole line is split.
        while word_len > limit {
            let cut = word
                .char_indices()
                .nth(limit)
                .map_or(word.len(), |(i, _)| i);
            lines.push(word[..cut].to_owned());
            word = &word[cut..];
            word_len -= limit;
            len = 0;
        }
        if len > 0 {
            line.push(' ');
            len += 1;
        }
        line.push_str(word);
        len += word_len;
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Draws `text` with its top-left at `(x, y)` in `colour` into `image`, a
/// tightly packed `size.0` × `size.1` RGBA image, fully opaque where drawn.
/// Pixels outside the image are clipped. Returns the x just past the text.
pub fn draw(
    image: &mut [u8],
    size: (u32, u32),
    (x, y): (u32, u32),
    text: &str,
    colour: [u8; 3],
) -> u32 {
    let mut pen = x;
    for c in text.chars() {
        let rows = glyph(c).unwrap_or(&FALLBACK);
        for (dy, row) in (0..).zip(rows) {
            for (dx, ink) in (0..).zip(row.bytes()) {
                let (px, py) = (pen + dx, y + dy);
                if ink == b'#' && px < size.0 && py < size.1 {
                    let i = ((py * size.0 + px) * 4) as usize;
                    if let Some(p) = image.get_mut(i..i + 4) {
                        p.copy_from_slice(&[colour[0], colour[1], colour[2], 255]);
                    }
                }
            }
        }
        pen += ADVANCE;
    }
    pen.saturating_sub(ADVANCE - GLYPH_W)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_glyph_is_three_wide_and_six_tall() {
        for (i, g) in ASCII.iter().chain(EXTRA.iter().map(|(_, g)| g)).enumerate() {
            for row in g {
                assert_eq!(row.len(), 3, "glyph {i}: {g:?}");
                assert!(row.bytes().all(|b| b == b'#' || b == b'.'), "glyph {i}");
            }
        }
    }

    #[test]
    fn distinct_characters_have_distinct_glyphs() {
        // The en dash is as wide as the hyphen at three pixels, so it
        // shares its glyph.
        let chars: Vec<char> = (' '..='~')
            .chain(EXTRA.iter().map(|(c, _)| *c).filter(|&c| c != '–'))
            .collect();
        for (i, a) in chars.iter().enumerate() {
            for b in &chars[i + 1..] {
                assert_ne!(glyph(*a), glyph(*b), "{a:?} and {b:?} look alike");
            }
        }
    }

    #[test]
    fn covers_ascii_and_the_extras_but_not_other_characters() {
        assert!((' '..='~').all(|c| glyph(c).is_some()));
        assert!(EXTRA.iter().all(|(c, _)| glyph(*c).is_some()));
        assert!(glyph('€').is_none());
        assert!(glyph('\n').is_none());
        // An unknown character draws the box, so it is still visible.
        let mut img = vec![0; 4 * 4 * 6 * 4];
        draw(&mut img, (4, 6), (0, 0), "€", [1, 2, 3]);
        assert_eq!(&img[..4], &[1, 2, 3, 255]);
    }

    #[test]
    fn measures_and_draws_at_the_pen() {
        assert_eq!(width(""), 0);
        assert_eq!(width("a"), 3);
        assert_eq!(width("ab"), 7);
        assert_eq!(columns(7), 2);
        assert_eq!(columns(6), 1);
        let mut img = vec![0; 16 * 8 * 4];
        let end = draw(&mut img, (16, 8), (2, 1), "T", [9, 9, 9]);
        assert_eq!(end, 5);
        let at = |x: u32, y: u32| img[((y * 16 + x) * 4 + 3) as usize];
        // The T's bar is its top row, its stem the middle column.
        assert_eq!((at(2, 1), at(3, 1), at(4, 1)), (255, 255, 255));
        assert_eq!((at(2, 2), at(3, 2), at(4, 2)), (0, 255, 0));
        assert_eq!(at(1, 1), 0);
        // Clipped, not wrapped or panicking.
        draw(&mut img, (16, 8), (14, 6), "HH", [1, 1, 1]);
    }

    #[test]
    fn wraps_at_spaces_to_the_width() {
        // Twelve columns: 47 pixels.
        let lines = wrap(
            "a small livebearing fish whose males sport bright fan tails.",
            47,
        );
        assert_eq!(
            lines,
            [
                "a small",
                "livebearing",
                "fish whose",
                "males sport",
                "bright fan",
                "tails."
            ]
            .map(str::to_owned)
        );
        assert!(lines.iter().all(|l| width(l) <= 47));
        // A word wider than the line is split; spaces collapse.
        assert_eq!(
            wrap("abcdefghijkl  mn", 19),
            ["abcde", "fghij", "kl mn"].map(str::to_owned)
        );
        assert_eq!(wrap("", 40), Vec::<String>::new());
        assert_eq!(wrap("   ", 40), Vec::<String>::new());
        assert_eq!(wrap("a b", 0), ["a", "b"].map(str::to_owned));
    }
}
