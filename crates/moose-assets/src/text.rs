//! Line tokenizer shared by the text formats: whitespace-separated tokens,
//! `#` comments to end of line, and double-quoted strings that may contain spaces.

pub(crate) struct Line {
    /// 1-based line number in the source file.
    pub no: usize,
    pub tokens: Vec<String>,
}

/// Splits `src` into non-empty token lines. Errors carry a 1-based line number.
pub(crate) fn tokenize(src: &str) -> Result<Vec<Line>, (usize, String)> {
    let mut lines = Vec::new();
    for (i, raw) in src.lines().enumerate() {
        let no = i + 1;
        let mut tokens = Vec::new();
        let mut chars = raw.chars().peekable();
        loop {
            while chars.next_if(|c| c.is_whitespace()).is_some() {}
            match chars.peek() {
                None | Some('#') => break,
                Some('"') => {
                    chars.next();
                    let mut token = String::new();
                    loop {
                        match chars.next() {
                            Some('"') => break,
                            Some(c) => token.push(c),
                            None => return Err((no, "unterminated quoted string".into())),
                        }
                    }
                    tokens.push(token);
                }
                Some(_) => {
                    let mut token = String::new();
                    while let Some(c) = chars.next_if(|&c| !c.is_whitespace() && c != '#') {
                        token.push(c);
                    }
                    tokens.push(token);
                }
            }
        }
        if !tokens.is_empty() {
            lines.push(Line { no, tokens });
        }
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_comments_and_quotes() {
        let lines =
            tokenize("a 1 2.5 # comment\n\n  # only comment\nname \"Two # Rooms\" x#y").unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].no, 1);
        assert_eq!(lines[0].tokens, ["a", "1", "2.5"]);
        assert_eq!(lines[1].no, 4);
        assert_eq!(lines[1].tokens, ["name", "Two # Rooms", "x"]);
    }

    #[test]
    fn unterminated_quote() {
        assert_eq!(tokenize("ok\nname \"oops").err().unwrap().0, 2);
    }
}
