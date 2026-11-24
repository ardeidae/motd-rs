//! `/etc/os-release` (freedesktop.org format): `KEY=value` lines, values
//! optionally in single or double quotes, `#` comments.

/// The fields we use from os-release.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OsRelease {
    pub pretty_name: Option<String>,
    pub name: Option<String>,
    pub version_id: Option<String>,
}

impl OsRelease {
    /// The best display name: `PRETTY_NAME`, else `NAME VERSION_ID`, else `NAME`.
    #[must_use]
    pub fn display_name(&self) -> Option<String> {
        match (&self.pretty_name, &self.name, &self.version_id) {
            (Some(pretty), _, _) => Some(pretty.clone()),
            (None, Some(name), Some(version)) => Some(format!("{name} {version}")),
            (None, Some(name), None) => Some(name.clone()),
            (None, None, _) => None,
        }
    }
}

/// Parses os-release content. Never fails: unknown or malformed lines are skipped.
#[must_use]
pub fn parse(input: &str) -> OsRelease {
    let mut release = OsRelease::default();
    for line in input.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, raw)) = line.split_once('=') else {
            continue;
        };
        let value = unquote(raw.trim());
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "PRETTY_NAME" => release.pretty_name = Some(value),
            "NAME" => release.name = Some(value),
            "VERSION_ID" => release.version_id = Some(value),
            _ => {}
        }
    }
    release
}

/// Removes matching quotes; in double quotes, `\x` becomes `x` (shell-like escapes).
fn unquote(raw: &str) -> String {
    if let Some(inner) = raw.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            } else {
                out.push(c);
            }
        }
        out
    } else if let Some(inner) = raw.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')) {
        inner.to_owned()
    } else {
        raw.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEBIAN_13: &str = r#"PRETTY_NAME="Debian GNU/Linux 13 (trixie)"
NAME="Debian GNU/Linux"
VERSION_ID="13"
VERSION="13 (trixie)"
VERSION_CODENAME=trixie
ID=debian
HOME_URL="https://www.debian.org/"
"#;

    const ROCKY_8: &str = r#"NAME="Rocky Linux"
VERSION="8.10 (Green Obsidian)"
ID="rocky"
ID_LIKE="rhel centos fedora"
VERSION_ID="8.10"
PLATFORM_ID="platform:el8"
PRETTY_NAME="Rocky Linux 8.10 (Green Obsidian)"
"#;

    #[test]
    fn real_files() {
        assert_eq!(
            parse(DEBIAN_13).display_name().as_deref(),
            Some("Debian GNU/Linux 13 (trixie)")
        );
        let rocky = parse(ROCKY_8);
        assert_eq!(
            rocky.display_name().as_deref(),
            Some("Rocky Linux 8.10 (Green Obsidian)")
        );
        assert_eq!(rocky.version_id.as_deref(), Some("8.10"));
    }

    #[test]
    fn quoting_and_comments() {
        let release =
            parse("# comment\nNAME='Single quoted'\nPRETTY_NAME=\"A \\\"quoted\\\" name\"\n");
        assert_eq!(release.name.as_deref(), Some("Single quoted"));
        assert_eq!(release.pretty_name.as_deref(), Some("A \"quoted\" name"));
        assert_eq!(parse("NAME=bare\n").name.as_deref(), Some("bare"));
    }

    #[test]
    fn fallbacks() {
        assert_eq!(
            parse("NAME=Foo\nVERSION_ID=2\n").display_name().as_deref(),
            Some("Foo 2")
        );
        assert_eq!(parse("NAME=Foo\n").display_name().as_deref(), Some("Foo"));
        assert_eq!(parse("").display_name(), None);
    }

    #[test]
    fn malformed_lines_are_skipped() {
        let release = parse("garbage\n=novalue\nPRETTY_NAME=\nNAME=\"unterminated\n\0\0\n");
        assert_eq!(release.pretty_name, None);
        assert_eq!(release.name.as_deref(), Some("\"unterminated"));
    }
}
