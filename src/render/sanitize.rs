//! Makes system strings safe to print: no control characters ever, and only
//! 7-bit bytes when the terminal is not known to handle UTF-8.
//!
//! Mount paths, interface names, the OS name or unit labels can contain any
//! character: a user can create a FUSE mount named with an escape sequence.
//! Every such string goes through [`sanitize`] before it is printed.

use std::ffi::OsString;

use crate::config::Charset;

/// The character set the output is written in, once `auto` is resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputCharset {
    /// Any character may be printed, except control characters.
    Utf8,
    /// Only bytes below 0x80: other characters are transliterated, or `?`.
    Ascii,
}

impl OutputCharset {
    /// Resolves the `charset` setting. `auto` looks at the locale the way
    /// the C library does: the first non-empty value of `LC_ALL`,
    /// `LC_CTYPE`, `LANG` decides, and only a UTF-8 locale gives UTF-8.
    /// No locale at all (cron, systemd units, ssh without `LANG`) is the C
    /// locale, which is ASCII.
    ///
    /// `var` reads an environment variable (a parameter so tests do not
    /// touch the process environment).
    pub fn resolve(setting: Charset, var: impl Fn(&str) -> Option<OsString>) -> Self {
        match setting {
            Charset::Utf8 => Self::Utf8,
            Charset::Ascii => Self::Ascii,
            Charset::Auto => {
                let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
                    .into_iter()
                    .filter_map(var)
                    .find(|value| !value.is_empty());
                match locale {
                    Some(value) if names_utf8(value.as_encoded_bytes()) => Self::Utf8,
                    _ => Self::Ascii,
                }
            }
        }
    }
}

/// `en_US.UTF-8`, `C.UTF-8`, `fr_FR.utf8`, `de_DE.UTF-8@euro`: glibc accepts
/// the codeset name in any case, with or without the dash.
fn names_utf8(locale: &[u8]) -> bool {
    let lower = locale.to_ascii_lowercase();
    [b"utf-8".as_slice(), b"utf8"]
        .iter()
        .any(|name| lower.windows(name.len()).any(|w| w == *name))
}

/// Replaces control characters (C0, DEL, C1, including ESC) with `?`, so that
/// system data can never inject terminal escape sequences. With an ASCII
/// output, also transliterates the other non-ASCII characters
/// (`/media/Données` → `/media/Donnees`), and those with no equivalent
/// (Cyrillic, Greek, CJK, emoji) become `?`. One `?` per character, so a
/// name keeps its length.
#[must_use]
pub fn sanitize(text: &str, charset: OutputCharset) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() {
            out.push('?');
        } else if c.is_ascii() || charset == OutputCharset::Utf8 {
            out.push(c);
        } else {
            match transliterate(c) {
                Some(ascii) => out.push_str(ascii),
                None => out.push('?'),
            }
        }
    }
    out
}

/// One ASCII letter per character of U+00C0–U+017F (Latin-1 Supplement
/// letters and Latin Extended-A), 16 per row. `*` marks the characters that
/// need two letters, handled in [`transliterate`] first.
const LATIN: &[u8; 192] = b"\
AAAAAA*CEEEEIIII\
DNOOOOOxOUUUUY**\
aaaaaa*ceeeeiiii\
dnooooo/ouuuuy*y\
AaAaAaCcCcCcCcDd\
DdEeEeEeEeEeGgGg\
GgGgHhHhIiIiIiIi\
Ii**JjKkkLlLlLlL\
lLlNnNnNn*NnOoOo\
Oo**RrRrRrSsSsSs\
SsTtTtTtUuUuUuUu\
UuUuWwYyYZzZzZzs";
// Rows, for review:
// C0 À Á Â Ã Ä Å Æ Ç È É Ê Ë Ì Í Î Ï    D0 Ð Ñ Ò Ó Ô Õ Ö × Ø Ù Ú Û Ü Ý Þ ß
// E0 à á â ã ä å æ ç è é ê ë ì í î ï    F0 ð ñ ò ó ô õ ö ÷ ø ù ú û ü ý þ ÿ
// 100 Ā ā Ă ă Ą ą Ć ć Ĉ ĉ Ċ ċ Č č Ď ď   110 Đ đ Ē ē Ĕ ĕ Ė ė Ę ę Ě ě Ĝ ĝ Ğ ğ
// 120 Ġ ġ Ģ ģ Ĥ ĥ Ħ ħ Ĩ ĩ Ī ī Ĭ ĭ Į į   130 İ ı Ĳ ĳ Ĵ ĵ Ķ ķ ĸ Ĺ ĺ Ļ ļ Ľ ľ Ŀ
// 140 ŀ Ł ł Ń ń Ņ ņ Ň ň ŉ Ŋ ŋ Ō ō Ŏ ŏ   150 Ő ő Œ œ Ŕ ŕ Ŗ ŗ Ř ř Ś ś Ŝ ŝ Ş ş
// 160 Š š Ţ ţ Ť ť Ŧ ŧ Ũ ũ Ū ū Ŭ ŭ Ů ů   170 Ű ű Ų ų Ŵ ŵ Ŷ ŷ Ÿ Ź ź Ż ż Ž ž ſ

/// The ASCII spelling of a non-ASCII character, if it has an obvious one:
/// Western and Central European letters, and the typography that word
/// processors and translated strings bring in (curly quotes, dashes,
/// ellipsis, non-breaking spaces).
fn transliterate(c: char) -> Option<&'static str> {
    let two = match c {
        'Æ' => "AE",
        'æ' => "ae",
        'Þ' => "TH",
        'þ' => "th",
        'ß' => "ss",
        'Ĳ' => "IJ",
        'ĳ' => "ij",
        'ŉ' => "'n",
        'Œ' => "OE",
        'œ' => "oe",
        '\u{a0}' | '\u{2002}'..='\u{200a}' | '\u{202f}' => " ",
        '‘' | '’' | '‚' | '‹' | '›' => "'",
        '“' | '”' | '„' | '«' | '»' => "\"",
        '‐' | '‑' | '‒' | '–' | '—' | '−' => "-",
        '…' => "...",
        _ => "",
    };
    if !two.is_empty() {
        return Some(two);
    }
    let at = usize::try_from(u32::from(c).checked_sub(0xc0)?).ok()?;
    // A byte of LATIN is one ASCII character, so a one-byte slice of the
    // table is a valid `str`.
    let letter = LATIN.get(at..=at)?;
    if letter == b"*" {
        return None;
    }
    std::str::from_utf8(letter).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn explicit_setting_ignores_the_locale() {
        let utf8 = env(&[("LANG", "en_US.UTF-8")]);
        assert_eq!(
            OutputCharset::resolve(Charset::Ascii, &utf8),
            OutputCharset::Ascii
        );
        assert_eq!(
            OutputCharset::resolve(Charset::Utf8, env(&[])),
            OutputCharset::Utf8
        );
    }

    #[test]
    fn auto_follows_the_first_non_empty_locale_variable() {
        let resolve = |pairs: &[(&str, &str)]| OutputCharset::resolve(Charset::Auto, env(pairs));
        assert_eq!(resolve(&[]), OutputCharset::Ascii, "no locale = C");
        assert_eq!(resolve(&[("LANG", "C")]), OutputCharset::Ascii);
        assert_eq!(resolve(&[("LANG", "POSIX")]), OutputCharset::Ascii);
        assert_eq!(resolve(&[("LANG", "fr_FR.UTF-8")]), OutputCharset::Utf8);
        assert_eq!(resolve(&[("LANG", "C.utf8")]), OutputCharset::Utf8);
        assert_eq!(
            resolve(&[("LANG", "de_DE.UTF-8@euro")]),
            OutputCharset::Utf8
        );
        assert_eq!(
            resolve(&[("LANG", "fr_FR.ISO-8859-15")]),
            OutputCharset::Ascii
        );
        assert_eq!(
            resolve(&[("LC_ALL", "C"), ("LANG", "en_US.UTF-8")]),
            OutputCharset::Ascii,
            "LC_ALL wins"
        );
        assert_eq!(
            resolve(&[("LC_CTYPE", "en_US.UTF-8"), ("LANG", "C")]),
            OutputCharset::Utf8,
            "LC_CTYPE wins over LANG"
        );
        assert_eq!(
            resolve(&[("LC_ALL", ""), ("LC_CTYPE", ""), ("LANG", "en_US.UTF-8")]),
            OutputCharset::Utf8,
            "empty values are skipped"
        );
    }

    #[test]
    fn auto_accepts_a_locale_that_is_not_utf8_bytes() {
        use std::os::unix::ffi::OsStringExt;
        let var =
            |name: &str| (name == "LANG").then(|| OsString::from_vec(b"x\xff.UTF-8".to_vec()));
        assert_eq!(
            OutputCharset::resolve(Charset::Auto, var),
            OutputCharset::Utf8
        );
    }

    #[test]
    fn control_characters_always_become_question_marks() {
        for charset in [OutputCharset::Utf8, OutputCharset::Ascii] {
            assert_eq!(sanitize("/mnt/\u{1b}[31mred", charset), "/mnt/?[31mred");
            assert_eq!(sanitize("a\tb\nc\u{7f}\u{9b}", charset), "a?b?c??");
            assert_eq!(sanitize("", charset), "");
        }
    }

    #[test]
    fn utf8_output_keeps_other_characters() {
        assert_eq!(
            sanitize("/media/Données ½ Дом 😀", OutputCharset::Utf8),
            "/media/Données ½ Дом 😀"
        );
    }

    #[test]
    fn ascii_output_transliterates_european_letters() {
        let ascii = |s| sanitize(s, OutputCharset::Ascii);
        assert_eq!(ascii("/media/Données"), "/media/Donnees");
        assert_eq!(ascii("Çà et là, Ñandú"), "Ca et la, Nandu");
        assert_eq!(ascii("Straße Øl Æble"), "Strasse Ol AEble");
        assert_eq!(ascii("Łódź Đakovo Þórr"), "Lodz Dakovo THorr");
        assert_eq!(ascii("cœur ĳs ŉ"), "coeur ijs 'n");
        assert_eq!(ascii("İı ſ ĸ ÿŸ"), "Ii s k yY");
        assert_eq!(ascii("×÷"), "x/");
    }

    #[test]
    fn ascii_output_transliterates_typography() {
        let ascii = |s| sanitize(s, OutputCharset::Ascii);
        assert_eq!(
            ascii("l’été « chaud » – “ok”…"),
            "l'ete \" chaud \" - \"ok\"..."
        );
        assert_eq!(ascii("10\u{a0}Go\u{202f}!"), "10 Go !");
        assert_eq!(ascii("a—b‐c−d"), "a-b-c-d");
    }

    #[test]
    fn ascii_output_marks_what_has_no_equivalent() {
        let ascii = |s| sanitize(s, OutputCharset::Ascii);
        assert_eq!(ascii("Дом"), "???");
        assert_eq!(ascii("αβ 日本 😀"), "?? ?? ?");
        assert_eq!(ascii("½ ° ¿"), "? ? ?", "below U+00C0: not in the table");
        assert_eq!(ascii("ƀ"), "?", "just past the table (U+0180)");
    }

    #[test]
    fn ascii_output_is_seven_bit_for_every_character_of_the_table() {
        for code in 0x80..=0x2fff_u32 {
            let Some(c) = char::from_u32(code) else {
                continue;
            };
            let out = sanitize(&c.to_string(), OutputCharset::Ascii);
            assert!(out.is_ascii(), "U+{code:04X} gave {out:?}");
            assert!(
                !out.is_empty() && !out.contains('*'),
                "U+{code:04X} gave {out:?}"
            );
        }
    }

    #[test]
    fn every_latin_letter_is_transliterated() {
        for code in 0xc0..=0x17f_u32 {
            let c = char::from_u32(code).unwrap();
            assert!(
                transliterate(c).is_some_and(|s| s.bytes().all(|b| b.is_ascii_graphic())),
                "U+{code:04X} {c}"
            );
        }
    }
}
