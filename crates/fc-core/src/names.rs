//! File-name compatibility between NTFS / APFS / exFAT / FAT32.
//!
//! * Unicode NFC normalization (macOS often hands out NFD names).
//! * Windows-forbidden characters → visually identical full-width characters
//!   (reversible: `＜` ↔ `<`), control chars → `_`.
//! * Reserved device names (CON, NUL, COM1…) and trailing dots/spaces.
//! * Case/normalization collision detection.

use std::collections::HashMap;
use unicode_normalization::UnicodeNormalization;

const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "COM¹", "COM²", "COM³", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8",
    "LPT9", "LPT¹", "LPT²", "LPT³",
];

fn fullwidth(c: char) -> Option<char> {
    Some(match c {
        '<' => '＜',
        '>' => '＞',
        ':' => '：',
        '"' => '＂',
        '\\' => '＼',
        '|' => '｜',
        '?' => '？',
        '*' => '＊',
        _ => return None,
    })
}

/// Reverse mapping, used when copying back to a filesystem that allows these chars.
pub fn from_fullwidth(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '＜' => '<',
            '＞' => '>',
            '：' => ':',
            '＂' => '"',
            '＼' => '\\',
            '｜' => '|',
            '？' => '?',
            '＊' => '*',
            other => other,
        })
        .collect()
}

/// Make a single path component valid for the target.
/// Returns the (possibly unchanged) name.
pub fn sanitize_component(name: &str, windows_rules: bool) -> String {
    let mut s: String = name.nfc().collect();
    if !windows_rules {
        return s;
    }
    s = s
        .chars()
        .map(|c| {
            if (c as u32) < 32 {
                '_'
            } else {
                fullwidth(c).unwrap_or(c)
            }
        })
        .collect();
    // Trailing dots and spaces are silently stripped by Windows → replace them.
    let trimmed_len = s.trim_end_matches(['.', ' ']).len();
    if trimmed_len < s.len() {
        let tail: String = s[trimmed_len..]
            .chars()
            .map(|c| if c == '.' { '．' } else { '_' })
            .collect();
        s.truncate(trimmed_len);
        s.push_str(&tail);
    }
    let stem = s
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end()
        .to_ascii_uppercase();
    if RESERVED
        .iter()
        .any(|r| r.eq_ignore_ascii_case(&stem) || *r == stem)
    {
        s.insert(0, '_');
    }
    if s.is_empty() {
        s.push('_');
    }
    // Component length limit: 255 UTF-16 units.
    if s.encode_utf16().count() > 255 {
        let (stem, ext) = split_ext(&s);
        let ext_len = ext.encode_utf16().count();
        let mut out = String::new();
        for c in stem.chars() {
            if out.encode_utf16().count() + c.len_utf16() + ext_len > 255 {
                break;
            }
            out.push(c);
        }
        out.push_str(ext);
        s = out;
    }
    s
}

/// Split `name.ext` → (`name`, `.ext`). Dotfiles keep their leading dot in the stem.
pub fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// `name (2).ext`, `name (3).ext`…
pub fn numbered(name: &str, n: u32) -> String {
    let (stem, ext) = split_ext(name);
    format!("{stem} ({n}){ext}")
}

/// Key used for collision detection on case-insensitive, normalization-insensitive targets.
pub fn collision_key(rel: &str) -> String {
    rel.nfc().collect::<String>().to_lowercase()
}

/// Tracks names already used per destination, resolving collisions with " (n)".
#[derive(Default)]
pub struct CollisionMap {
    seen: HashMap<String, u32>,
}

impl CollisionMap {
    /// Returns a unique relative path (components joined by '/').
    pub fn unique(&mut self, rel: &str) -> String {
        let key = collision_key(rel);
        if let std::collections::hash_map::Entry::Vacant(e) = self.seen.entry(key) {
            e.insert(1);
            return rel.to_string();
        }
        let (dir, file) = match rel.rfind('/') {
            Some(i) => (&rel[..=i], &rel[i + 1..]),
            None => ("", rel),
        };
        let mut n = 2;
        loop {
            let cand = format!("{dir}{}", numbered(file, n));
            if let std::collections::hash_map::Entry::Vacant(e) =
                self.seen.entry(collision_key(&cand))
            {
                e.insert(1);
                return cand;
            }
            n += 1;
        }
    }
}

/// macOS / Windows metadata junk that should not be copied by default.
pub fn is_junk(name: &str) -> bool {
    name.starts_with("._")
        || matches!(
            name,
            ".DS_Store"
                | ".Spotlight-V100"
                | ".fseventsd"
                | ".Trashes"
                | ".TemporaryItems"
                | ".DocumentRevisions-V100"
                | "Thumbs.db"
                | "desktop.ini"
                | "$RECYCLE.BIN"
                | "System Volume Information"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forbidden_chars() {
        assert_eq!(sanitize_component("a:b?c*.txt", true), "a：b？c＊.txt");
        assert_eq!(from_fullwidth("a：b？c＊.txt"), "a:b?c*.txt");
        assert_eq!(sanitize_component("a:b", false), "a:b");
    }

    #[test]
    fn reserved_and_trailing() {
        assert_eq!(sanitize_component("CON", true), "_CON");
        assert_eq!(sanitize_component("nul.txt", true), "_nul.txt");
        assert_eq!(sanitize_component("console.txt", true), "console.txt");
        assert_eq!(sanitize_component("name.", true), "name．");
        assert_eq!(sanitize_component("name ", true), "name_");
    }

    #[test]
    fn nfc() {
        let nfd = "e\u{0301}te\u{0301}.txt";
        assert_eq!(sanitize_component(nfd, false), "\u{e9}t\u{e9}.txt");
    }

    #[test]
    fn collisions() {
        let mut m = CollisionMap::default();
        assert_eq!(m.unique("dir/Readme.md"), "dir/Readme.md");
        assert_eq!(m.unique("dir/README.md"), "dir/README (2).md");
        assert_eq!(m.unique("dir/readme.MD"), "dir/readme (3).MD");
        assert_eq!(m.unique("other/README.md"), "other/README.md");
    }

    #[test]
    fn long_names() {
        let n = "a".repeat(300) + ".txt";
        let s = sanitize_component(&n, true);
        assert_eq!(s.encode_utf16().count(), 255);
        assert!(s.ends_with(".txt"));
    }

    #[test]
    fn junk() {
        assert!(is_junk("._foo"));
        assert!(is_junk(".DS_Store"));
        assert!(!is_junk(".gitignore"));
    }
}
