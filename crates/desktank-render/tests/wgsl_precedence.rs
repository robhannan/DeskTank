//! Regression guard for WGSL operator-precedence rules that naga accepts
//! but Chrome's WGSL compiler (Tint) rejects.
//!
//! WGSL has no precedence between the bitwise operators (`&`, `|`, `^`) and
//! any other binary operator: their operands must be unary expressions, so
//! `a * b ^ c` is an error ("mixing '*' and '^' requires parenthesis") and
//! must be written `(a * b) ^ c`. Chaining the *same* bitwise operator
//! (`a & b & c`) is allowed. naga is laxer, so native tests and the WebGL2
//! (naga → GLSL) browser tests never see the error; only WebGPU in Chrome
//! does, where it leaves a black canvas.
//!
//! The check is a deliberately small token scan, not a parser: within each
//! bracket group (and each `,`/`;`/assignment-separated segment of it), if
//! a bitwise operator appears at that nesting level, every other binary
//! operator at the same level must be that same bitwise operator. Single
//! `<` / `>` are skipped because they are indistinguishable from template
//! brackets (`vec2<f32>`) without a parser, so mixing with a comparison
//! like `a & b < c` is not caught.

use std::fs;
use std::path::Path;

/// Binary operators (other than single `<` / `>`) that matter for the rule.
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

/// Strips `//` and `/* */` comments, then splits into words and punctuation
/// (longest match first, so `^=` and `&&` stay whole).
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
            rest = rest.find("*/").map_or("", |i| &rest[i + 2..]);
        } else if c.is_whitespace() {
            rest = &rest[c.len_utf8()..];
        } else if c.is_alphanumeric() || c == '_' {
            let end = rest
                .find(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
                .unwrap_or(rest.len());
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

/// Records an error if `ops` (the binary operators of one segment) mixes a
/// bitwise operator with any different binary operator.
fn check(ops: &[&str], errors: &mut Vec<String>) {
    if let Some(bit) = ops.iter().find(|op| matches!(**op, "&" | "|" | "^"))
        && let Some(other) = ops.iter().find(|op| *op != bit)
    {
        errors.push(format!("mixing '{other}' and '{bit}' requires parentheses"));
    }
}

/// Returns a message for each bitwise operator mixed with a different binary
/// operator at the same nesting level.
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
fn flags_mixed_bitwise_and_arithmetic() {
    let bad = "var h = u32(p.x) * 0x8da6b343u ^ u32(p.y) * 0xd8163841u;";
    assert_eq!(
        precedence_errors(bad),
        ["mixing '*' and '^' requires parentheses"]
    );
    assert_eq!(precedence_errors("let a = x & y | z;").len(), 1);
    assert_eq!(precedence_errors("if (x & 1u == 0u) {}").len(), 1);
    assert_eq!(precedence_errors("f(a, b >> 1u & 3u)").len(), 1);
}

#[test]
fn accepts_parenthesised_and_chained_bitwise() {
    for ok in [
        "var h = (u32(p.x) * 0x8da6b343u) ^ (u32(p.y) * 0xd8163841u);",
        "h = (h ^ (h >> 13u)) * 0x5bd1e995u;",
        "return h ^ (h >> 15u);",
        "let m = a & b & c;",
        "h ^= h >> 3u;",
        "let k = -a ^ b;",
        "if a && b || c {}",
        "let v = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)); // a * b ^ c",
        "/* a * b ^ c */ let x = i32(round(t.g * 255.0)) & 63;",
    ] {
        assert!(precedence_errors(ok).is_empty(), "false positive: {ok}");
    }
}

/// Every `.wgsl` file in this crate's sources must pass the check.
#[test]
fn shaders_parenthesise_bitwise_operands() {
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
