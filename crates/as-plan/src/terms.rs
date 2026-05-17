//! Term tokenisation for the grep stage.
//!
//! Splits an NL query into ASCII / Unicode tokens, drops short noise
//! words, and returns the unique remaining tokens in input order.

const STOP: &[&str] = &[
    "a", "an", "the", "and", "or", "of", "to", "in", "for", "on", "is", "are", "be", "by", "with",
    "as", "at", "it",
];

pub fn tokenize(query: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for raw in query.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-')) {
        let t = raw.trim().to_lowercase();
        if t.len() < 3 {
            continue;
        }
        if STOP.contains(&t.as_str()) {
            continue;
        }
        if seen.insert(t.clone()) {
            out.push(t);
        }
        if out.len() >= 12 {
            break;
        }
    }
    out
}

pub fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        if matches!(
            c,
            '.' | '+'
                | '*'
                | '?'
                | '('
                | ')'
                | '|'
                | '['
                | ']'
                | '{'
                | '}'
                | '^'
                | '$'
                | '\\'
                | '/'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_short_and_stop_words() {
        let t = tokenize("the quick brown fox jumps over the lazy dog");
        assert!(t.contains(&"quick".to_string()));
        assert!(t.contains(&"brown".to_string()));
        assert!(!t.contains(&"the".to_string()));
        assert!(!t.contains(&"a".to_string()));
    }

    #[test]
    fn preserves_unique_order() {
        let t = tokenize("alpha beta alpha gamma");
        assert_eq!(t, vec!["alpha", "beta", "gamma"]);
    }
}
