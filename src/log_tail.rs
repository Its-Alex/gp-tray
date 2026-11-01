use std::path::Path;

const TAIL_LINES: usize = 40;
const MAX_LENGTH: usize = 300;

/// Best guess at the reason a connection failed: the last error-looking line
/// of the connect log, else the last non-empty line.
pub fn error_tail(log_file: &Path) -> String {
    let Ok(bytes) = std::fs::read(log_file) else {
        return String::new();
    };
    error_tail_of(&String::from_utf8_lossy(&bytes))
}

pub fn error_tail_of(text: &str) -> String {
    let tail: Vec<&str> = text.lines().rev().take(TAIL_LINES).collect();

    for line in &tail {
        let line = line.trim();
        if line.starts_with("Error:")
            || line.contains("certificate verify failed")
            || line.contains("Failed to")
        {
            return truncate(line);
        }
    }
    for line in &tail {
        let line = line.trim();
        if !line.is_empty() {
            return truncate(line);
        }
    }
    String::new()
}

fn truncate(line: &str) -> String {
    line.chars().take(MAX_LENGTH).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_the_last_error_line() {
        let text = "\
Connected to portal.example.com
Error: gateway unreachable
random chatter
";
        assert_eq!(error_tail_of(text), "Error: gateway unreachable");
    }

    #[test]
    fn recognises_error_shapes() {
        let cases = [
            ("noise\nError: bad cookie\n", "Error: bad cookie"),
            (
                "noise\nssl: certificate verify failed\n",
                "ssl: certificate verify failed",
            ),
            (
                "noise\nFailed to connect to 1.2.3.4\n",
                "Failed to connect to 1.2.3.4",
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(error_tail_of(text), expected);
        }
    }

    #[test]
    fn returns_last_error_when_noise_follows() {
        let text = "Error: first problem\nnoise one\nnoise two\n";
        assert_eq!(error_tail_of(text), "Error: first problem");
    }

    #[test]
    fn falls_back_to_the_last_non_empty_line_trimmed() {
        assert_eq!(error_tail_of("  hello  \n\n   \n"), "hello");
        assert_eq!(error_tail_of(""), "");
        assert_eq!(error_tail_of("\n\n"), "");
    }

    #[test]
    fn only_the_last_forty_lines_count() {
        let mut text = String::from("Error: ancient failure\n");
        for index in 0..45 {
            text.push_str(&format!("line {index}\n"));
        }
        assert_eq!(error_tail_of(&text), "line 44");
    }

    #[test]
    fn truncates_at_three_hundred_characters() {
        let long = format!("Error: {}", "x".repeat(400));
        assert_eq!(error_tail_of(&long).len(), MAX_LENGTH);
        let unicode = "é".repeat(400);
        assert_eq!(error_tail_of(&unicode).chars().count(), MAX_LENGTH);
    }

    #[test]
    fn missing_log_reads_as_empty() {
        assert_eq!(
            error_tail(Path::new("/nonexistent/gp-tray/connect.log")),
            ""
        );
    }

    #[test]
    fn invalid_utf8_is_ignored() {
        let dir = std::env::temp_dir().join("gp-tray-log-test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("connect.log");
        let mut bytes = b"bad \xff byte\nError: real failure\n".to_vec();
        bytes.extend_from_slice(b"\xfe\n");
        std::fs::write(&file, bytes).unwrap();
        assert_eq!(error_tail(&file), "Error: real failure");
    }
}
