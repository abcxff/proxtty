//! Menu action helpers (Milestone 15).
//!
//! Pure building blocks for the actions a menu item can perform; the app wires
//! them to the child PTY, the outer terminal, and the OS. Keeping the logic here
//! (and tested) means [`crate::app`] only has to dispatch.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;

/// Build an OSC 52 sequence asking the terminal to put `text` on the system
/// clipboard. This works even over SSH, because the *outer* terminal performs
/// the copy when `proxtty` writes the sequence to it.
pub fn osc52_copy(text: &str) -> Vec<u8> {
    let encoded = STANDARD.encode(text.as_bytes());
    format!("\x1b]52;c;{encoded}\x07").into_bytes()
}

/// Find the first `http`/`https` URL in `text`, trimming trailing punctuation.
pub fn find_url(text: &str) -> Option<String> {
    let http = text.find("http://");
    let https = text.find("https://");
    let start = match (http, https) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => return None,
    };

    let tail = &text[start..];
    let end = tail
        .find(|c: char| c.is_whitespace() || "\"'<>`|".contains(c))
        .unwrap_or(tail.len());
    let url = tail[..end].trim_end_matches(|c: char| ".,);]}>".contains(c));

    // Require something after the scheme.
    if url.ends_with("//") {
        None
    } else {
        Some(url.to_string())
    }
}

/// The platform command that opens a URL in the user's default app.
pub fn opener() -> &'static str {
    if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    }
}

/// Open `url` in the user's default application, reaping the helper process so it
/// doesn't linger as a zombie.
pub fn open_url(url: String) {
    use std::process::Command;
    if let Ok(mut child) = Command::new(opener()).arg(url).spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc52_roundtrip() {
        // "hi" base64 is "aGk=".
        assert_eq!(osc52_copy("hi"), b"\x1b]52;c;aGk=\x07");
    }

    #[test]
    fn finds_first_url() {
        let t = "see https://example.com/path, and http://later.org";
        assert_eq!(find_url(t).as_deref(), Some("https://example.com/path"));
    }

    #[test]
    fn picks_earliest_scheme() {
        let t = "http://a.com then https://b.com";
        assert_eq!(find_url(t).as_deref(), Some("http://a.com"));
    }

    #[test]
    fn trims_trailing_punctuation() {
        assert_eq!(
            find_url("(https://x.io).").as_deref(),
            Some("https://x.io")
        );
    }

    #[test]
    fn none_when_absent() {
        assert_eq!(find_url("no links here"), None);
        assert_eq!(find_url("https://"), None);
    }
}
