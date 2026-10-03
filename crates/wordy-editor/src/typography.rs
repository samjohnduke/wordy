//! Smart typography applied as you type: curly quotes, em dashes, ellipses.
//! Pure functions over the text before the caret so they can be unit tested.

/// Given the (up to two) characters before the caret in the same paragraph and
/// the character just typed, return how many characters to delete backwards
/// and what to insert instead of the typed character.
pub fn smart_replace(before: &str, ch: char) -> Option<(usize, &'static str)> {
    let last = before.chars().last();
    let opens_after = |c: Option<char>| match c {
        None => true,
        Some(c) => c.is_whitespace() || matches!(c, '(' | '[' | '{' | '‘' | '“' | '—' | '–' | '/'),
    };
    match ch {
        '"' => Some((0, if opens_after(last) { "“" } else { "”" })),
        '\'' => Some((0, if opens_after(last) { "‘" } else { "’" })),
        '-' if last == Some('-') => Some((1, "—")),
        '.' if before.ends_with("..") => Some((2, "…")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_open_and_close_by_context() {
        assert_eq!(smart_replace("", '"'), Some((0, "“")));
        assert_eq!(smart_replace("He said ", '"'), Some((0, "“")));
        assert_eq!(smart_replace("word", '"'), Some((0, "”")));
        assert_eq!(smart_replace("(", '"'), Some((0, "“")));
        assert_eq!(smart_replace("don", '\''), Some((0, "’")));
        assert_eq!(smart_replace(" ", '\''), Some((0, "‘")));
    }

    #[test]
    fn dashes_and_ellipses() {
        assert_eq!(smart_replace("a-", '-'), Some((1, "—")));
        assert_eq!(smart_replace("a", '-'), None);
        assert_eq!(smart_replace("a..", '.'), Some((2, "…")));
        assert_eq!(smart_replace("a.", '.'), None);
    }

    #[test]
    fn ordinary_characters_pass_through() {
        assert_eq!(smart_replace("abc", 'd'), None);
        assert_eq!(smart_replace("", ' '), None);
    }
}
