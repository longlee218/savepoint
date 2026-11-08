//! Redis-compatible glob matching over raw bytes (`*`, `?`, `[abc]`, `[a-z]`,
//! `[^x]`/`[!x]`, `\` escapes). Used for Redis and Memcached key scoping.

pub fn glob_match(pattern: &[u8], text: &[u8]) -> bool {
    // Iterative matcher with single-star backtracking: O(len(p) * len(t)) worst case,
    // no recursion, so hostile patterns cannot blow the stack.
    let (mut p, mut t) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() {
            match pattern[p] {
                b'*' => {
                    while p < pattern.len() && pattern[p] == b'*' {
                        p += 1;
                    }
                    if p == pattern.len() {
                        return true;
                    }
                    star = Some((p, t));
                    continue;
                }
                b'?' => {
                    p += 1;
                    t += 1;
                    continue;
                }
                b'[' => {
                    if let Some((matched, next_p)) = match_class(pattern, p, text[t]) {
                        if matched {
                            p = next_p;
                            t += 1;
                            continue;
                        }
                    } else if text[t] == b'[' {
                        // Unterminated class: treat '[' literally.
                        p += 1;
                        t += 1;
                        continue;
                    }
                }
                b'\\' if p + 1 < pattern.len() => {
                    if pattern[p + 1] == text[t] {
                        p += 2;
                        t += 1;
                        continue;
                    }
                }
                c => {
                    if c == text[t] {
                        p += 1;
                        t += 1;
                        continue;
                    }
                }
            }
        }
        match star {
            Some((sp, st)) => {
                p = sp;
                t = st + 1;
                star = Some((sp, st + 1));
            }
            None => return false,
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

/// Matches `c` against the class starting at `pattern[start] == '['`.
/// Returns `(matched, index after ']')`, or `None` if the class is unterminated.
fn match_class(pattern: &[u8], start: usize, c: u8) -> Option<(bool, usize)> {
    let mut i = start + 1;
    let negate = i < pattern.len() && (pattern[i] == b'^' || pattern[i] == b'!');
    if negate {
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    while i < pattern.len() {
        let mut lo = pattern[i];
        if lo == b']' && !first {
            return Some((matched != negate, i + 1));
        }
        first = false;
        if lo == b'\\' && i + 1 < pattern.len() {
            i += 1;
            lo = pattern[i];
        }
        if i + 2 < pattern.len() && pattern[i + 1] == b'-' && pattern[i + 2] != b']' {
            let mut hi = pattern[i + 2];
            let mut adv = 3;
            if hi == b'\\' && i + 3 < pattern.len() {
                hi = pattern[i + 3];
                adv = 4;
            }
            let (a, b) = if lo <= hi { (lo, hi) } else { (hi, lo) };
            if c >= a && c <= b {
                matched = true;
            }
            i += adv;
        } else {
            if c == lo {
                matched = true;
            }
            i += 1;
        }
    }
    None
}

/// A key is in scope when it matches any `include` pattern (empty = everything)
/// and no `exclude` pattern.
pub fn in_scope(include: &[String], exclude: &[String], key: &[u8]) -> bool {
    let included = include.is_empty() || include.iter().any(|p| glob_match(p.as_bytes(), key));
    included && !exclude.iter().any(|p| glob_match(p.as_bytes(), key))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(p: &str, t: &str) -> bool {
        glob_match(p.as_bytes(), t.as_bytes())
    }

    #[test]
    fn literals_and_wildcards() {
        assert!(m("abc", "abc"));
        assert!(!m("abc", "abcd"));
        assert!(m("*", ""));
        assert!(m("*", "anything"));
        assert!(m("bull:*", "bull:email:wait"));
        assert!(!m("bull:*", "xbull:email"));
        assert!(m("a*b*c", "a__b__c"));
        assert!(!m("a*b*c", "a__b__"));
        assert!(m("h?llo", "hello"));
        assert!(!m("h?llo", "hllo"));
        assert!(m("*:*:wait", "bull:q:wait"));
    }

    #[test]
    fn classes_and_escapes() {
        assert!(m("h[ae]llo", "hallo"));
        assert!(!m("h[ae]llo", "hillo"));
        assert!(m("h[a-c]llo", "hbllo"));
        assert!(!m("h[a-c]llo", "hdllo"));
        assert!(m("h[^e]llo", "hallo"));
        assert!(!m("h[^e]llo", "hello"));
        assert!(m("h[!e]llo", "hallo"));
        assert!(m("a\\*b", "a*b"));
        assert!(!m("a\\*b", "axb"));
        assert!(m("[\\]]", "]"));
    }

    #[test]
    fn binary_keys() {
        assert!(glob_match(b"k\x00*", b"k\x00\xff\xfe"));
        assert!(!glob_match(b"k\x01*", b"k\x00\xff"));
    }

    #[test]
    fn scope_rules() {
        let inc = vec!["app:*".to_string(), "session:*".to_string()];
        let exc = vec!["app:tmp:*".to_string()];
        assert!(in_scope(&inc, &exc, b"app:user:1"));
        assert!(in_scope(&inc, &exc, b"session:9"));
        assert!(!in_scope(&inc, &exc, b"app:tmp:1"));
        assert!(!in_scope(&inc, &exc, b"bull:q:1"));
        assert!(in_scope(&[], &[], b"whatever"));
        assert!(!in_scope(&[], &["bull:*".into()], b"bull:q:id"));
    }
}
