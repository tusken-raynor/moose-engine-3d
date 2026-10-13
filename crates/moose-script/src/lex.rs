//! Splits a script's text into tokens.

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Tok {
    Ident(String),
    /// A number, and whether it was written whole (`2`, not `2.0` or `2e3`).
    Num(f64, bool),
    Str(String),
    Punct(&'static str),
    Eof,
}

#[derive(Clone, Debug)]
pub(crate) struct Token {
    pub tok: Tok,
    /// The 1-based line it starts on.
    pub line: usize,
}

/// Longest first, so `===` is read before `==`.
const PUNCTS: [&str; 46] = [
    "...", "===", "!==", "=>", "==", "!=", "<=", ">=", "&&", "||", "??", "?.", "+=", "-=", "*=", "/=", "%=", "++",
    "--", "{", "}", "(", ")", "[", "]", ";", ",", ".", ":", "?", "<", ">", "+", "-", "*", "/", "%", "=", "!", "|",
    "&", "^", "~", "@", "#", "`",
];

/// The tokens of `src`, ending with [`Tok::Eof`]; or the line of what can't be read, and why.
pub(crate) fn lex(src: &str) -> Result<Vec<Token>, (usize, String)> {
    let bytes = src.as_bytes();
    let (mut i, mut line) = (0, 1);
    let mut tokens = Vec::new();
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\n' {
            line += 1;
            i += 1;
        } else if c.is_ascii_whitespace() {
            i += 1;
        } else if src[i..].starts_with("//") {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if src[i..].starts_with("/*") {
            let end = src[i + 2..].find("*/").ok_or((line, "this comment never ends".to_string()))?;
            line += src[i..i + 2 + end].matches('\n').count();
            i += end + 4;
        } else if c.is_ascii_alphabetic() || c == b'_' || c == b'$' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'$') {
                i += 1;
            }
            tokens.push(Token { tok: Tok::Ident(src[start..i].to_string()), line });
        } else if c.is_ascii_digit() || (c == b'.' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            let start = i;
            if src[i..].starts_with("0x") || src[i..].starts_with("0X") {
                i += 2;
                while i < bytes.len() && bytes[i].is_ascii_hexdigit() {
                    i += 1;
                }
                let value = u32::from_str_radix(&src[start + 2..i], 16)
                    .map_err(|_| (line, format!("'{}' is not a number", &src[start..i])))?;
                tokens.push(Token { tok: Tok::Num(value as f64, true), line });
                continue;
            }
            let mut whole = true;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'.' {
                whole = false;
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
            }
            if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
                whole = false;
                i += 1;
                if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
                    i += 1;
                }
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let value =
                src[start..i].parse::<f64>().map_err(|_| (line, format!("'{}' is not a number", &src[start..i])))?;
            tokens.push(Token { tok: Tok::Num(value, whole), line });
        } else if c == b'"' || c == b'\'' {
            let mut text = String::new();
            i += 1;
            loop {
                match bytes.get(i) {
                    None | Some(b'\n') => return Err((line, "this string never ends".to_string())),
                    Some(&q) if q == c => break,
                    Some(b'\\') => {
                        text.push(match bytes.get(i + 1) {
                            Some(b'n') => '\n',
                            Some(b't') => '\t',
                            Some(b'\\') => '\\',
                            Some(b'\'') => '\'',
                            Some(b'"') => '"',
                            _ => return Err((line, "unknown escape in a string (\\n, \\t, \\\\, \\' and \\\" are known)".to_string())),
                        });
                        i += 2;
                    }
                    Some(_) => {
                        let ch = src[i..].chars().next().unwrap();
                        text.push(ch);
                        i += ch.len_utf8();
                    }
                }
            }
            i += 1;
            tokens.push(Token { tok: Tok::Str(text), line });
        } else {
            let Some(mut p) = PUNCTS.iter().copied().find(|p| src[i..].starts_with(p)) else {
                let ch = src[i..].chars().next().unwrap();
                return Err((line, format!("'{ch}' is not something a script can contain")));
            };
            // `a ?.5 : b` is a conditional, as in JavaScript.
            if p == "?." && bytes.get(i + 2).is_some_and(u8::is_ascii_digit) {
                p = "?";
            }
            if p == "`" {
                return Err((line, "template strings (`...`) aren't supported in Moose scripts".to_string()));
            }
            i += p.len();
            tokens.push(Token { tok: Tok::Punct(p), line });
        }
    }
    tokens.push(Token { tok: Tok::Eof, line });
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_carry_their_lines_and_numbers_know_if_they_are_whole() {
        let tokens = lex("let a = 2; // two\n/* and\n more */ b?.c !== 0x10 + 2.5e1 + 'it\\'s'").unwrap();
        let toks: Vec<Tok> = tokens.iter().map(|t| t.tok.clone()).collect();
        assert_eq!(toks[3], Tok::Num(2.0, true));
        assert_eq!(toks[5], Tok::Ident("b".into()));
        assert_eq!(tokens[5].line, 3);
        assert_eq!(toks[6], Tok::Punct("?."));
        assert_eq!(toks[8], Tok::Punct("!=="));
        assert_eq!(toks[9], Tok::Num(16.0, true));
        assert_eq!(toks[11], Tok::Num(25.0, false));
        assert_eq!(toks[13], Tok::Str("it's".into()));
        assert_eq!(toks[14], Tok::Eof);
    }

    #[test]
    fn what_cannot_be_read_names_its_line() {
        assert_eq!(lex("a\n'open").unwrap_err().0, 2);
        assert_eq!(lex("/* open").unwrap_err().0, 1);
        assert!(lex("`hi`").unwrap_err().1.contains("template"));
        // A conditional, not an optional chain.
        assert_eq!(lex("a ?.5 : 1").unwrap()[1].tok, Tok::Punct("?"));
    }
}
