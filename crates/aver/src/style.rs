//! Colour for the CLI's human output: verdicts and diff counts. Colour only
//! when stdout is a terminal and `NO_COLOR` is unset; everything else (pipes,
//! files, agents reading the output) gets the plain text.

use std::io::IsTerminal;

use aver_core::Verdict;

/// Whether stdout gets colour.
pub fn enabled() -> bool {
    std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

/// `text` in the SGR `code` when `on`, else as is.
pub fn paint(on: bool, code: &str, text: &str) -> String {
    if on {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

/// The SGR code for a verdict (`None`: no receipt).
pub fn verdict_code(v: Option<Verdict>) -> &'static str {
    match v {
        Some(Verdict::Pass) => "32;1",
        Some(Verdict::Review) => "36",
        Some(Verdict::FailOwn) => "31",
        Some(Verdict::Unproven) => "33",
        Some(Verdict::BlockedNotOwn) => "35",
        None => "2",
    }
}

/// `text` coloured as the verdict `v`.
pub fn verdict(v: Option<Verdict>, text: &str) -> String {
    paint(enabled(), verdict_code(v), text)
}

/// Added lines or files (`+12`).
pub fn added(text: &str) -> String {
    paint(enabled(), "32", text)
}

/// Removed lines or files (`-3`).
pub fn removed(text: &str) -> String {
    paint(enabled(), "31", text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_is_plain_when_off() {
        assert_eq!(paint(false, "32", "PASS"), "PASS");
        assert_eq!(paint(true, "32", "PASS"), "\x1b[32mPASS\x1b[0m");
        assert_eq!(verdict_code(Some(Verdict::Pass)), "32;1");
    }
}
