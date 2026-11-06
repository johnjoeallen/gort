use crate::diag::{Diag, R};

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident(String),
    Int(u64),
    Float(f64),
    Str(Vec<u8>),
    Char(u8),
    Sym(&'static str),
    Eof,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub line: u32,
    pub col: u32,
    /// a newline separates this token from the previous one
    pub nl: bool,
}

const SYMS: &[&str] = &[
    "...", "==", "!=", "<=", ">=", "&&", "||", "+=", "-=", "*=", "/=", "%=", "++", "--", "->", "::", "..", "+", "-", "*", "/", "%", "<", ">",
    "=", "!", ".", ",", ":", ";", "(", ")", "{", "}", "[", "]", "?", "&", "|",
];

pub fn lex(file: &str, src: &str) -> R<Vec<Token>> {
    let b = src.as_bytes();
    let mut i = 0;
    let (mut line, mut col) = (1u32, 1u32);
    let mut out = Vec::new();
    let mut nl = false;
    let err = |line, col, msg: &str| Diag { file: file.to_string(), line, col, msg: msg.to_string() };
    macro_rules! adv {
        () => {{
            if b[i] == b'\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
            i += 1;
        }};
    }
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            nl = true;
            adv!();
            continue;
        }
        if c == b' ' || c == b'\t' || c == b'\r' {
            adv!();
            continue;
        }
        if c == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                adv!();
            }
            continue;
        }
        if c == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            let (sl, sc) = (line, col);
            adv!();
            adv!();
            loop {
                if i + 1 >= b.len() {
                    return Err(err(sl, sc, "unterminated block comment"));
                }
                if b[i] == b'*' && b[i + 1] == b'/' {
                    adv!();
                    adv!();
                    break;
                }
                adv!();
            }
            continue;
        }
        let (tl, tc) = (line, col);
        let push = |out: &mut Vec<Token>, tok: Tok, nl: &mut bool| {
            out.push(Token { tok, line: tl, col: tc, nl: *nl });
            *nl = false;
        };
        if c.is_ascii_alphabetic() || c == b'_' {
            let s = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                adv!();
            }
            push(&mut out, Tok::Ident(src[s..i].to_string()), &mut nl);
            continue;
        }
        if c.is_ascii_digit() {
            let s = i;
            if c == b'0' && i + 1 < b.len() && (b[i + 1] == b'x' || b[i + 1] == b'X') {
                adv!();
                adv!();
                let hs = i;
                while i < b.len() && (b[i].is_ascii_hexdigit() || b[i] == b'_') {
                    adv!();
                }
                let t: String = src[hs..i].chars().filter(|c| *c != '_').collect();
                let v = u64::from_str_radix(&t, 16).map_err(|_| err(tl, tc, "bad hex literal"))?;
                push(&mut out, Tok::Int(v), &mut nl);
                continue;
            }
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
                adv!();
            }
            let mut is_float = false;
            if i + 1 < b.len() && b[i] == b'.' && b[i + 1].is_ascii_digit() {
                is_float = true;
                adv!();
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
                    adv!();
                }
            }
            let t: String = src[s..i].chars().filter(|c| *c != '_').collect();
            if is_float {
                push(&mut out, Tok::Float(t.parse().map_err(|_| err(tl, tc, "bad float literal"))?), &mut nl);
            } else {
                push(&mut out, Tok::Int(t.parse().map_err(|_| err(tl, tc, "integer literal too large"))?), &mut nl);
            }
            continue;
        }
        if c == b'"' {
            adv!();
            let mut v = Vec::new();
            loop {
                if i >= b.len() || b[i] == b'\n' {
                    return Err(err(tl, tc, "unterminated string literal"));
                }
                if b[i] == b'"' {
                    adv!();
                    break;
                }
                if b[i] == b'\\' {
                    adv!();
                    if i >= b.len() {
                        return Err(err(tl, tc, "unterminated string literal"));
                    }
                    let e = b[i];
                    adv!();
                    match e {
                        b'n' => v.push(b'\n'),
                        b'r' => v.push(b'\r'),
                        b't' => v.push(b'\t'),
                        b'0' => v.push(0),
                        b'\\' => v.push(b'\\'),
                        b'"' => v.push(b'"'),
                        b'\'' => v.push(b'\''),
                        b'x' => {
                            if i + 1 >= b.len() {
                                return Err(err(line, col, "bad \\x escape"));
                            }
                            let h = std::str::from_utf8(&b[i..i + 2]).unwrap_or("");
                            let x = u8::from_str_radix(h, 16).map_err(|_| err(line, col, "bad \\x escape"))?;
                            adv!();
                            adv!();
                            v.push(x);
                        }
                        _ => return Err(err(line, col - 1, "unknown escape sequence")),
                    }
                } else {
                    v.push(b[i]);
                    adv!();
                }
            }
            push(&mut out, Tok::Str(v), &mut nl);
            continue;
        }
        if c == b'\'' {
            adv!();
            if i >= b.len() {
                return Err(err(tl, tc, "unterminated character literal"));
            }
            let v = if b[i] == b'\\' {
                adv!();
                let e = b[i];
                adv!();
                match e {
                    b'n' => b'\n',
                    b'r' => b'\r',
                    b't' => b'\t',
                    b'0' => 0,
                    b'\\' => b'\\',
                    b'\'' => b'\'',
                    _ => return Err(err(tl, tc, "unknown escape sequence")),
                }
            } else {
                let v = b[i];
                adv!();
                v
            };
            if i >= b.len() || b[i] != b'\'' {
                return Err(err(tl, tc, "unterminated character literal"));
            }
            adv!();
            push(&mut out, Tok::Char(v), &mut nl);
            continue;
        }
        let mut matched = None;
        for s in SYMS {
            if src[i..].starts_with(s) {
                matched = Some(*s);
                break;
            }
        }
        match matched {
            Some(s) => {
                for _ in 0..s.len() {
                    adv!();
                }
                push(&mut out, Tok::Sym(s), &mut nl);
            }
            None => return Err(err(tl, tc, &format!("unexpected character '{}'", c as char))),
        }
    }
    out.push(Token { tok: Tok::Eof, line, col, nl: true });
    Ok(out)
}
