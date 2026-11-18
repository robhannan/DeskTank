//! Regression guard for WGSL operator-precedence rules that naga accepts
//! but Chrome's WGSL compiler (Tint) rejects.
//!
//! WGSL leaves several operator mixes without precedence, so they need
//! parentheses (naga is laxer and accepts them all):
//!
//! - bitwise `&`, `|`, `^`: operands must be unary expressions, so
//!   `a * b ^ c` is an error ("mixing '*' and '^' requires parenthesis");
//!   chaining the *same* operator (`a & b & c`) is allowed;
//! - shifts `<<`, `>>`: both operands must be unary (`a << 1u + 2u` and
//!   `a << b << c` are errors), though `a << b == c` is fine;
//! - comparisons don't chain (`a == b == c`);
//! - `&&` and `||` don't mix (`a && b || c`).
//!
//! Native tests and the WebGL2 (naga → GLSL) browser tests never see these
//! errors; only WebGPU in Chrome does, where the page shows a black canvas.
//!
//! The check is a deliberately small token scan, not a parser: it collects
//! the binary operators of each bracket group (split further at `,`, `;`,
//! `:`, braces and assignments) and applies the rules above to them. Single
//! `<` / `>` are skipped because they are indistinguishable from template
//! brackets (`vec2<f32>`) without a parser, so mixes such as `a & b < c` or
//! `a < b == c` are not caught.

use std::fs;
use std::path::Path;

/// Binary operators (other than single `<` / `>`) that matter for the rules.
const BINARY: &[&str] = &[
    "<<", ">>", "==", "!=", "<=", ">=", "&&", "||", "*", "/", "%", "+", "-", "&", "|", "^",
];
/// Tokens that end an expression segment.
const SEPARATORS: &[&str] = &[
    "<<=", ">>=", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "->", "++", "--", "=", ",", ";",
    ":", "{", "}",
];
/// Keywords that start an expression, so a following `-`/`&`/`*` is unary.
const KEYWORDS: &[&str] = &[
    "return", "let", "var", "const", "if", "else", "while", "for", "switch", "case", "loop",
];

#[derive(Debug, PartialEq)]
enum Token<'a> {
    /// Identifier, keyword or number literal.
    Word(&'a str),
    Punct(&'a str),
}

/// Strips `//` and (nested) `/* */` comments, then splits into words and
/// punctuation (longest match first, so `^=` and `&&` stay whole).
fn tokenize(src: &str) -> Vec<Token<'_>> {
    let mut puncts: Vec<&str> = BINARY.iter().chain(SEPARATORS).copied().collect();
    puncts.extend(["(", ")", "[", "]", "<", ">", "!", "~", ".", "@"]);
    puncts.sort_by_key(|p| std::cmp::Reverse(p.len()));

    let mut tokens = Vec::new();
    let mut rest = src;
    while let Some(c) = rest.chars().next() {
        if rest.starts_with("//") {
            rest = rest.find('\n').map_or("", |i| &rest[i..]);
        } else if rest.starts_with("/*") {
            rest = skip_block_comment(rest);
        } else if c.is_whitespace() {
            rest = &rest[c.len_utf8()..];
        } else if c.is_alphanumeric() || c == '_' {
            let end = word_end(rest);
            tokens.push(Token::Word(&rest[..end]));
            rest = &rest[end..];
        } else if let Some(p) = puncts.iter().find(|p| rest.starts_with(**p)) {
            tokens.push(Token::Punct(p));
            rest = &rest[p.len()..];
        } else {
            rest = &rest[c.len_utf8()..];
        }
    }
    tokens
}

/// Skips the block comment at the start of `rest`; WGSL block comments nest.
fn skip_block_comment(mut rest: &str) -> &str {
    let mut depth = 0;
    while let Some(c) = rest.chars().next() {
        if rest.starts_with("/*") {
            depth += 1;
            rest = &rest[2..];
        } else if rest.starts_with("*/") {
            depth -= 1;
            rest = &rest[2..];
            if depth == 0 {
                break;
            }
        } else {
            rest = &rest[c.len_utf8()..];
        }
    }
    rest
}

/// Length of the identifier or number at the start of `rest`. A number keeps
/// its fraction and exponent sign (`1.0e-3`, `0x1p+4`), so the sign isn't
/// read as an operator.
fn word_end(rest: &str) -> usize {
    let number = rest.starts_with(|c: char| c.is_ascii_digit());
    let exponent = if rest.starts_with("0x") || rest.starts_with("0X") {
        ['p', 'P']
    } else {
        ['e', 'E']
    };
    let mut prev = ' ';
    for (i, c) in rest.char_indices() {
        let sign = matches!(c, '+' | '-') && exponent.contains(&prev);
        if !(c.is_alphanumeric() || c == '_' || (number && (c == '.' || sign))) {
            return i;
        }
        prev = c;
    }
    rest.len()
}

/// Records an error for each operator mix in `ops` (the binary operators of
/// one segment, in order) that WGSL requires parentheses for.
fn check(ops: &[&str], errors: &mut Vec<String>) {
    let mut mix = |a: &str, b: &str| {
        errors.push(format!("mixing '{a}' and '{b}' requires parentheses"));
    };
    let logical = |op: &&str| matches!(*op, "&&" | "||");
    let relational = |op: &&str| matches!(*op, "==" | "!=" | "<=" | ">=");

    // A bitwise operator chains only with itself.
    if let Some(bit) = ops.iter().find(|op| matches!(**op, "&" | "|" | "^")) {
        if let Some(other) = ops.iter().find(|op| *op != bit) {
            mix(other, bit);
        }
        return;
    }
    if ops.contains(&"&&") && ops.contains(&"||") {
        mix("&&", "||");
    }
    for clause in ops.split(logical) {
        // At most one comparison per `&&`/`||` operand.
        if let [a, b, ..] = clause
            .iter()
            .filter(|op| relational(op))
            .collect::<Vec<_>>()[..]
        {
            mix(a, b);
        }
        // A shift's operands are unary: nothing else between comparisons.
        for operand in clause.split(relational) {
            if let Some(shift) = operand.iter().find(|op| matches!(**op, "<<" | ">>"))
                && operand.len() > 1
            {
                let other = operand.iter().find(|op| *op != shift).unwrap_or(shift);
                mix(other, shift);
            }
        }
    }
}

/// Returns a message for each operator mix at one nesting level that WGSL
/// requires parentheses for.
fn precedence_errors(src: &str) -> Vec<String> {
    let tokens = tokenize(src);
    // One entry per open bracket: the binary operators seen at that level in
    // the current segment.
    let mut levels: Vec<Vec<&str>> = vec![Vec::new()];
    let mut errors = Vec::new();
    let mut prev: Option<&Token> = None;
    for token in &tokens {
        match token {
            Token::Punct("(" | "[") => levels.push(Vec::new()),
            Token::Punct(")" | "]") => {
                if let Some(ops) = levels.pop() {
                    check(&ops, &mut errors);
                }
                if levels.is_empty() {
                    levels.push(Vec::new());
                }
            }
            Token::Punct(p) if SEPARATORS.contains(p) => {
                if let Some(ops) = levels.last_mut() {
                    check(ops, &mut errors);
                    ops.clear();
                }
            }
            Token::Punct(p) if BINARY.contains(p) => {
                // An operator is binary only after an operand; otherwise it
                // is unary (`-x`, `&v`, `*ptr`) and doesn't count.
                let after_operand = match prev {
                    Some(Token::Word(w)) => !KEYWORDS.contains(w),
                    Some(Token::Punct(q)) => matches!(*q, ")" | "]"),
                    None => false,
                };
                if after_operand && let Some(ops) = levels.last_mut() {
                    ops.push(p);
                }
            }
            _ => {}
        }
        prev = Some(token);
    }
    for ops in &levels {
        check(ops, &mut errors);
    }
    errors
}

#[test]
fn flags_mixes_that_need_parentheses() {
    let bad = "var h = u32(p.x) * 0x8da6b343u ^ u32(p.y) * 0xd8163841u;";
    assert_eq!(
        precedence_errors(bad),
        ["mixing '*' and '^' requires parentheses"]
    );
    for bad in [
        "let a = x & y | z;",
        "if (x & 1u == 0u) {}",
        "f(a, b >> 1u & 3u)",
        "let s = a << 1u + 2u;",
        "let s = a * 2u >> 1u;",
        "let s = a << b << c;",
        "if a && b || c {}",
        "let e = a == b == c;",
    ] {
        assert_eq!(precedence_errors(bad).len(), 1, "missed: {bad}");
    }
}

#[test]
fn accepts_valid_precedence() {
    for ok in [
        "var h = (u32(p.x) * 0x8da6b343u) ^ (u32(p.y) * 0xd8163841u);",
        "h = (h ^ (h >> 13u)) * 0x5bd1e995u;",
        "return h ^ (h >> 15u);",
        "let m = a & b & c;",
        "h ^= h >> 3u;",
        "let k = -a ^ b;",
        "if (a && b && c) || (d && e) {}",
        "let r = a + b * c == d && e != f;",
        "let s = a << b == c >> d;",
        "let f = 1.0e-3 * x + 0x1p+4;",
        "let v = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)); // a * b ^ c",
        "/* a /* b */ * c ^ d */ let x = i32(round(t.g * 255.0)) & 63;",
    ] {
        assert!(precedence_errors(ok).is_empty(), "false positive: {ok}");
    }
}

/// Every `.wgsl` file in this crate's sources must pass the check.
#[test]
fn shaders_parenthesise_operator_mixes() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut checked = 0;
    for entry in fs::read_dir(&dir).expect("read src dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().is_some_and(|e| e == "wgsl") {
            let src = fs::read_to_string(&path).expect("read shader");
            let errors = precedence_errors(&src);
            assert!(errors.is_empty(), "{}: {errors:?}", path.display());
            checked += 1;
        }
    }
    assert!(checked > 0, "no .wgsl files found in {}", dir.display());
}
