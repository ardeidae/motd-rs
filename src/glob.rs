//! Minimal glob matching for config patterns (`disks.exclude`).
//!
//! `*` matches any run of characters, `/` included, so `/snap/*` hides every
//! mount under `/snap` at any depth: in mount-point lists that is what people
//! mean, and a shell-like `**` would be one more rule to learn. `?` matches
//! exactly one character. There is no escape and no `[...]` class: mount
//! points with a literal `*` or `?` are not worth the extra syntax.

/// Whether `text` matches `pattern` as a whole.
#[must_use]
pub fn matches(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    // Iterative matching with backtracking to the last `*` only: linear in
    // practice, never exponential on patterns like `*a*a*a*b`.
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while let Some(&c) = text.get(t) {
        match pattern.get(p) {
            Some('*') => {
                star = Some((p, t));
                p += 1;
            }
            Some(&pc) if pc == '?' || pc == c => {
                p += 1;
                t += 1;
            }
            _ => match star {
                // Let the last `*` absorb one more character and retry.
                Some((sp, st)) => {
                    star = Some((sp, st + 1));
                    p = sp + 1;
                    t = st + 1;
                }
                None => return false,
            },
        }
    }
    pattern
        .get(p..)
        .is_some_and(|rest| rest.iter().all(|&c| c == '*'))
}

#[cfg(test)]
mod tests {
    use super::matches;

    #[test]
    fn literal() {
        assert!(matches("/boot", "/boot"));
        assert!(!matches("/boot", "/boot/efi"));
        assert!(!matches("/boot/efi", "/boot"));
        assert!(matches("", ""));
        assert!(!matches("", "/"));
    }

    #[test]
    fn star_crosses_slashes() {
        assert!(matches("/snap/*", "/snap/core/123"));
        assert!(matches("/snap/*", "/snap/"));
        assert!(!matches("/snap/*", "/snap"));
        assert!(matches("*/kubelet/*", "/var/lib/kubelet/pods/x/mount"));
        assert!(matches("*", ""));
        assert!(matches("**", "/a"));
        assert!(matches("/a*b*c", "/aXbYbZc"));
        assert!(!matches("/a*b*c", "/aXbYbZ"));
    }

    #[test]
    fn question_mark_is_one_char() {
        assert!(matches("/mnt/disk?", "/mnt/disk1"));
        assert!(!matches("/mnt/disk?", "/mnt/disk"));
        assert!(!matches("/mnt/disk?", "/mnt/disk12"));
        assert!(
            matches("/media/donn?es", "/media/données"),
            "one char, not one byte"
        );
    }

    #[test]
    fn pathological_pattern_is_fast() {
        let text = "a".repeat(10_000);
        assert!(!matches(&format!("{}b", "*a".repeat(50)), &text));
    }
}
