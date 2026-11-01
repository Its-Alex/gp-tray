use crate::paths::{TUNNEL_UNIT_PREFIX, TUNNEL_UNIT_SUFFIX};

/// Mirrors `systemd-escape` for a single name: `/` becomes `-`, a leading `.`
/// and every byte outside `[A-Za-z0-9_.:-]` becomes a lowercase `\xNN` escape.
pub fn escape(name: &str) -> String {
    let mut escaped = String::with_capacity(name.len());
    for (index, byte) in name.bytes().enumerate() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(byte, b'_' | b':')
            || (byte == b'.' && index > 0);
        if byte == b'/' {
            escaped.push('-');
        } else if keep {
            escaped.push(char::from(byte));
        } else {
            escaped.push_str(&format!("\\x{byte:02x}"));
        }
    }
    escaped
}

pub fn unit_name(portal: &str) -> String {
    format!("{TUNNEL_UNIT_PREFIX}{}{TUNNEL_UNIT_SUFFIX}", escape(portal))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_systemd_escape_output() {
        let cases = [
            ("portal.example.com", "portal.example.com"),
            ("a b", "a\\x20b"),
            ("a-b", "a\\x2db"),
            ("a.b", "a.b"),
            ("a:b", "a:b"),
            ("a_b", "a_b"),
            ("a/b", "a-b"),
            ("A@b", "A\\x40b"),
            ("foo bar/baz", "foo\\x20bar-baz"),
            ("Ünïcode", "\\xc3\\x9cn\\xc3\\xafcode"),
            ("a+b", "a\\x2bb"),
            ("x$y", "x\\x24y"),
            ("   ", "\\x20\\x20\\x20"),
            ("", ""),
            ("a\\b", "a\\x5cb"),
            ("tab\tx", "tab\\x09x"),
            ("quote'q", "quote\\x27q"),
            ("a?b", "a\\x3fb"),
            ("a*b", "a\\x2ab"),
            (".a", "\\x2ea"),
            ("..a", "\\x2e.a"),
            (".", "\\x2e"),
            ("a.", "a."),
            (". a", "\\x2e\\x20a"),
            ("a b.c", "a\\x20b.c"),
            ("a/.b", "a-.b"),
            ("/.a", "-.a"),
            ("gp 0", "gp\\x200"),
            ("a\x01b", "a\\x01b"),
        ];
        for (input, expected) in cases {
            assert_eq!(escape(input), expected, "escaping {input:?}");
        }
    }

    #[test]
    fn builds_the_tunnel_unit_name() {
        assert_eq!(
            unit_name("portal.example.com"),
            "gp-tray-tunnel@portal.example.com.service"
        );
        assert_eq!(
            unit_name("My Portal"),
            "gp-tray-tunnel@My\\x20Portal.service"
        );
    }
}
