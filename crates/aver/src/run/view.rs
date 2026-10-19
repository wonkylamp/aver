//! What `aver run` shows while agents work (§2.10): a live table redrawn in
//! place on a terminal, or one line per event (`--no-tui`, or when stdout
//! is not a terminal) for CI, scripts and planner agents.

use std::io::{IsTerminal, Write};
use std::time::Instant;

use aver_core::Verdict;

use super::state::AgentState;

/// One agent's line in the live view.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub index: u32,
    pub template: String,
    pub state: AgentState,
    pub action: String,
    pub files: u32,
    pub added: u32,
    pub removed: u32,
    pub verdict: Option<Verdict>,
    pub proven: u32,
    pub criteria: u32,
    pub verifies: u32,
    /// What other tasks do to this agent's files, one short line each.
    pub notes: Vec<String>,
}

pub trait View {
    /// Lines printed once, before agents start.
    fn header(&mut self, lines: &[String]);
    /// The current state of every agent; called often.
    fn update(&mut self, rows: &[Row]);
    /// Something happened to agent `index` (started, verified, resumed, …).
    fn event(&mut self, index: u32, template: &str, what: &str);
    /// The run is over: leave the screen readable.
    fn finish(&mut self, rows: &[Row]);
    /// Something about the whole run (the cross-check starting).
    fn note(&mut self, _what: &str) {}
}

/// The live view on a terminal, else line mode.
pub fn for_stdout(no_tui: bool, title: String) -> Box<dyn View> {
    if !no_tui
        && std::io::stdout().is_terminal()
        && std::env::var_os("TERM").is_some_and(|t| t != "dumb")
    {
        Box::new(Live::new(title))
    } else {
        Box::new(Lines::new(std::io::stdout()))
    }
}

pub fn verdict_text(v: Option<Verdict>) -> &'static str {
    v.map_or("-", |v| v.as_str())
}

/// Terminal width, or 100 when it cannot be read.
pub fn columns() -> usize {
    // SAFETY: TIOCGWINSZ fills a winsize struct for a terminal descriptor.
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 {
            return ws.ws_col as usize;
        }
    }
    100
}

/// `s` cut to `width` characters, with `…` marking a cut.
pub fn fit(s: &str, width: usize) -> String {
    let count = s.chars().count();
    if count <= width {
        return format!("{s:<width$}");
    }
    let mut out: String = s.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// `s` cut to at most `width` characters at a word boundary (mid-word
/// only when the first word is longer than `width`), with `…` marking a
/// cut, unpadded.
pub fn fit_words(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_owned();
    }
    let keep: String = s.chars().take(width.saturating_sub(1)).collect();
    let cut = match keep.rfind(char::is_whitespace) {
        Some(i) if i > 0 => keep[..i].trim_end().to_owned(),
        _ => keep,
    };
    format!("{cut}…")
}

/// Agent or repository text for one line of the live view: no control
/// characters, no Markdown fences or backticks, runs of spaces collapsed.
pub fn clean(s: &str) -> String {
    let flat: String = s
        .chars()
        .map(|c| if c.is_control() || c == '`' { ' ' } else { c })
        .collect();
    flat.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A path as a person reads it: canonical (no `/var` against
/// `/private/var`), the home directory as `~`.
pub fn display_path(path: &std::path::Path) -> String {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .and_then(|h| std::fs::canonicalize(&h).ok().or(Some(h)));
    match home.and_then(|h| canonical.strip_prefix(&h).ok().map(|r| r.to_owned())) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~/{}", rest.display()),
        None => canonical.display().to_string(),
    }
}

/// A revision for a title: a full commit hash cut to its short form.
pub fn short_rev(rev: &str) -> &str {
    if rev.len() > 12 && rev.chars().all(|c| c.is_ascii_hexdigit()) {
        &rev[..7]
    } else {
        rev
    }
}

pub fn elapsed(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

/// Lines redrawn in place on a terminal, the cursor hidden meanwhile.
#[derive(Default)]
pub struct Screen {
    drawn: usize,
}

impl Screen {
    pub fn draw(&mut self, lines: &[String]) {
        let mut out = String::new();
        if self.drawn > 0 {
            out.push_str(&format!("\x1b[{}A", self.drawn));
        } else {
            out.push_str("\x1b[?25l");
        }
        for line in lines {
            out.push_str("\x1b[2K");
            out.push_str(line);
            out.push('\n');
        }
        // Clear lines left over from a taller frame.
        for _ in lines.len()..self.drawn {
            out.push_str("\x1b[2K\n");
        }
        let extra = self.drawn.saturating_sub(lines.len());
        if extra > 0 {
            out.push_str(&format!("\x1b[{extra}A"));
        }
        self.drawn = lines.len();
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(out.as_bytes());
        let _ = stdout.flush();
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        if self.drawn > 0 {
            print!("\x1b[?25h");
            let _ = std::io::stdout().flush();
        }
    }
}

/// Live table, redrawn in place.
pub struct Live {
    title: String,
    started: Instant,
    screen: Screen,
    color: bool,
    tick: usize,
    events: Vec<String>,
}

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

impl Live {
    pub fn new(title: String) -> Self {
        Self {
            title,
            started: Instant::now(),
            screen: Screen::default(),
            color: std::env::var_os("NO_COLOR").is_none(),
            tick: 0,
            events: Vec::new(),
        }
    }

    fn paint(&self, code: &str, text: &str) -> String {
        crate::style::paint(self.color, code, text)
    }

    fn state_color(s: AgentState) -> &'static str {
        match s {
            AgentState::Done => "32",
            AgentState::Verifying | AgentState::Checking => "36",
            AgentState::Working | AgentState::Preparing => "0",
            AgentState::GaveUp
            | AgentState::OutOfVerifies
            | AgentState::Stopped
            | AgentState::Idle => "33",
            AgentState::Timeout | AgentState::Killed | AgentState::Failed => "31",
        }
    }

    fn lines(&self, rows: &[Row]) -> Vec<String> {
        render(
            rows,
            &self.title,
            self.started.elapsed().as_millis() as u64,
            &self.events,
            self.tick,
            columns().max(60),
            &|code, text| self.paint(code, text),
        )
    }

    fn draw(&mut self, rows: &[Row]) {
        let lines = self.lines(rows);
        self.screen.draw(&lines);
    }
}

impl View for Live {
    fn header(&mut self, lines: &[String]) {
        for line in lines {
            println!("{line}");
        }
    }

    fn update(&mut self, rows: &[Row]) {
        self.tick += 1;
        self.draw(rows);
    }

    fn event(&mut self, index: u32, template: &str, what: &str) {
        self.events.push(format!(
            "{} #{index} {template} {what}",
            elapsed(self.started.elapsed().as_millis() as u64)
        ));
    }

    fn finish(&mut self, rows: &[Row]) {
        self.draw(rows);
        print!("\x1b[?25h");
        let _ = std::io::stdout().flush();
    }

    fn note(&mut self, what: &str) {
        self.events.push(format!(
            "{} {what}",
            elapsed(self.started.elapsed().as_millis() as u64)
        ));
    }
}

/// The live view's lines for a terminal `width` columns wide: a title, a
/// line per agent (its state and last action in one column, cut on words),
/// up to three other-task notes and the last three events. `paint` colours
/// text by SGR code.
pub fn render(
    rows: &[Row],
    title: &str,
    elapsed_ms: u64,
    events: &[String],
    tick: usize,
    width: usize,
    paint: &dyn Fn(&str, &str) -> String,
) -> Vec<String> {
    // "  #  agent   " is 13 wide, files 11, verify 10, crit 5.
    let state_w = width.saturating_sub(13 + 11 + 10 + 6).clamp(16, 100);
    let mut out = Vec::new();
    let head = format!(" {} · {}", clean(title), elapsed(elapsed_ms));
    let hint = "ctrl-c stop";
    let pad = width.saturating_sub(head.chars().count() + hint.len() + 2);
    out.push(format!(
        "{}{}{}",
        paint("1", &head),
        " ".repeat(pad),
        paint("2", hint)
    ));
    out.push(paint(
        "2",
        &format!(
            "  #  agent   {}  files      verify    crit",
            fit("state", state_w)
        ),
    ));
    for r in rows {
        let spin = if r.state.is_terminal() {
            ' '
        } else {
            SPINNER[(tick + r.index as usize) % SPINNER.len()]
        };
        let action = clean(&r.action);
        let state = if action.is_empty() {
            r.state.as_str().to_owned()
        } else {
            format!("{}: {action}", r.state.as_str())
        };
        let state = fit_words(&state, state_w - 1);
        let state_pad = " ".repeat((state_w - 1).saturating_sub(state.chars().count()));
        let files = if r.files == 0 {
            "-".to_owned()
        } else {
            format!("{} +{}-{}", r.files, r.added, r.removed)
        };
        let crit = if r.criteria == 0 {
            "-".to_owned()
        } else {
            format!("{}/{}", r.proven, r.criteria)
        };
        out.push(format!(
            "  {:<2} {:<7} {}{}{}  {:<10} {} {}",
            r.index,
            fit(&r.template, 7),
            spin,
            paint(Live::state_color(r.state), &state),
            state_pad,
            fit(&files, 10),
            paint(
                crate::style::verdict_code(r.verdict),
                &fit(verdict_text(r.verdict), 9)
            ),
            crit,
        ));
    }
    let mut notes: Vec<(String, Vec<u32>)> = Vec::new();
    for r in rows {
        for note in &r.notes {
            let note = clean(note);
            match notes.iter_mut().find(|(n, _)| *n == note) {
                Some((_, who)) => who.push(r.index),
                None => notes.push((note, vec![r.index])),
            }
        }
    }
    for (note, who) in notes.iter().take(3) {
        let who: Vec<String> = who.iter().map(|i| format!("#{i}")).collect();
        out.push(paint(
            "33",
            &fit_words(
                &format!("  other task: {note} ({})", who.join(", ")),
                width - 1,
            ),
        ));
    }
    for e in events.iter().rev().take(3).rev() {
        out.push(paint(
            "2",
            &fit_words(&format!("  {}", clean(e)), width - 1),
        ));
    }
    out
}

/// One line per event, for logs and scripts.
pub struct Lines<W: Write> {
    out: W,
    started: Instant,
}

impl<W: Write> Lines<W> {
    pub fn new(out: W) -> Self {
        Self {
            out,
            started: Instant::now(),
        }
    }

    pub fn into_inner(self) -> W {
        self.out
    }
}

impl<W: Write> View for Lines<W> {
    fn header(&mut self, lines: &[String]) {
        for line in lines {
            let _ = writeln!(self.out, "{line}");
        }
    }

    fn update(&mut self, _: &[Row]) {}

    fn event(&mut self, index: u32, template: &str, what: &str) {
        let _ = writeln!(
            self.out,
            "[{:>6}] #{index} {template:<7} {what}",
            elapsed(self.started.elapsed().as_millis() as u64)
        );
        let _ = self.out.flush();
    }

    fn finish(&mut self, _: &[Row]) {}

    fn note(&mut self, what: &str) {
        let _ = writeln!(
            self.out,
            "[{:>6}] {what}",
            elapsed(self.started.elapsed().as_millis() as u64)
        );
        let _ = self.out.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_pads_and_cuts() {
        assert_eq!(fit("ab", 4), "ab  ");
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(elapsed(552_000), "9m12s");
        assert_eq!(elapsed(41_000), "41s");
        assert_eq!(elapsed(3_720_000), "1h02m");
    }

    #[test]
    fn fit_words_cuts_between_words() {
        assert_eq!(fit_words("short", 10), "short");
        assert_eq!(
            fit_words("editing the parser module now", 16),
            "editing the…"
        );
        assert_eq!(fit_words("supercalifragilistic", 6), "super…");
        assert_eq!(short_rev("HEAD"), "HEAD");
        assert_eq!(
            short_rev("0123456789abcdef0123456789abcdef01234567"),
            "0123456"
        );
    }

    fn row(index: u32, state: AgentState, action: &str) -> Row {
        Row {
            index,
            template: "claude".into(),
            state,
            action: action.into(),
            files: 2,
            added: 10,
            removed: 1,
            verdict: Some(Verdict::Pass),
            proven: 1,
            criteria: 1,
            verifies: 1,
            notes: vec!["```rust\nfn x()``` also changes src/a.rs".into()],
        }
    }

    #[test]
    fn live_rows_merge_state_and_action_without_fences_or_mid_word_cuts() {
        let rows = [
            row(
                1,
                AgentState::Working,
                "Running ```cargo test``` on\nthe whole workspace now please",
            ),
            row(2, AgentState::Done, ""),
        ];
        let plain = |_: &str, t: &str| t.to_owned();
        let lines = render(
            &rows,
            "aver run · 2 agents · HEAD",
            61_000,
            &[],
            0,
            80,
            &plain,
        );
        assert!(
            lines[0].contains("aver run · 2 agents · HEAD · 1m01s"),
            "{lines:#?}"
        );
        assert!(lines[1].contains("state"), "{lines:#?}");
        assert!(!lines[1].contains("last action"), "{lines:#?}");
        let one = &lines[2];
        assert!(one.contains("working: Running cargo test on"), "{one}");
        assert!(one.contains("…"), "{one}");
        assert!(!one.contains('`'), "{one}");
        assert!(lines[3].contains("done "), "{lines:#?}");
        for line in &lines {
            assert!(line.chars().count() <= 80, "{line}");
            assert!(!line.contains("```") && !line.contains('\n'), "{line}");
        }
        // The cut ends a word: the character before `…` ends a whole word
        // of the action.
        let before = one.split('…').next().unwrap();
        let last = before.split_whitespace().last().unwrap();
        assert!(
            "Running cargo test on the whole workspace now please"
                .split_whitespace()
                .any(|w| w == last),
            "{one}"
        );
    }
}
