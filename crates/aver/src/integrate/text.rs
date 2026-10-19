//! The instructions every integration gives its agent tool, and the marked
//! blocks that carry them into files Aver shares with the user.
//!
//! One source: the body of the Claude Code plugin's `aver` skill
//! (`integrations/claude-code/skills/aver/SKILL.md`). Cursor's rule,
//! `AGENTS.md`, `GEMINI.md` and Copilot's instructions get the same text,
//! so every tool tells its agent the same thing.

/// The plugin's `aver` skill, frontmatter and all.
pub const SKILL: &str = super::plugin::SHARED[0].text;

/// The first line of a block Aver manages in a Markdown file; a line that
/// starts with [`BEGIN`] opens one.
pub const BEGIN_LINE: &str =
    "<!-- BEGIN aver: written by `aver integrate`; running it again replaces this block -->";
pub const BEGIN: &str = "<!-- BEGIN aver";
pub const END: &str = "<!-- END aver -->";

/// The same markers in TOML, around the MCP server Aver before 0.4 wrote
/// into `.codex/config.toml`.
pub const TOML_BEGIN: &str = "# BEGIN aver";
pub const TOML_END: &str = "# END aver";

fn split() -> (&'static str, &'static str) {
    let rest = SKILL
        .strip_prefix("---\n")
        .expect("SKILL.md starts with frontmatter");
    rest.split_once("\n---\n")
        .expect("SKILL.md's frontmatter ends")
}

/// The skill's body: the instructions themselves.
pub fn instructions() -> &'static str {
    split().1.trim()
}

/// The skill's one-line `description`.
pub fn description() -> &'static str {
    split()
        .0
        .lines()
        .find_map(|l| l.strip_prefix("description: "))
        .expect("SKILL.md has a description")
        .trim()
}

/// The instructions as a marked Markdown block, ending in a newline.
pub fn block() -> String {
    format!("{BEGIN_LINE}\n\n{}\n\n{END}\n", instructions())
}

/// `existing` (the file at `path`, if any) with the block between
/// `begin` and `end` replaced by `block`, or `block` appended when there is
/// none; everything outside the block is kept. `block` starts with its
/// begin line and ends with its end line and a newline.
pub fn upsert(
    path: &str,
    existing: Option<&str>,
    begin: &str,
    end: &str,
    block: &str,
) -> Result<String, String> {
    let text = existing.unwrap_or("");
    let mut offset = 0;
    let mut begins = Vec::new();
    let mut ends = Vec::new();
    for line in text.split_inclusive('\n') {
        let bare = line.trim_end();
        if bare.starts_with(begin) {
            begins.push((offset, offset + line.len()));
        } else if bare == end {
            ends.push((offset, offset + line.len()));
        }
        offset += line.len();
    }
    match (begins.as_slice(), ends.as_slice()) {
        ([], []) if text.trim().is_empty() => Ok(block.to_owned()),
        ([], []) => {
            let mut out = text.to_owned();
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
            out.push_str(block);
            Ok(out)
        }
        ([(start, _)], [(_, stop)]) if start < stop => {
            let mut out = String::with_capacity(text.len() + block.len());
            out.push_str(&text[..*start]);
            out.push_str(block);
            out.push_str(&text[*stop..]);
            Ok(out)
        }
        _ => Err(format!(
            "{path} holds Aver's markers ({begin} … {end}) more than once or unmatched; fix them or remove the block first"
        )),
    }
}

/// `existing` without the block between `begin` and `end` (and the blank
/// line [`upsert`] put before it); unchanged when there is no block.
pub fn remove(path: &str, existing: &str, begin: &str, end: &str) -> Result<String, String> {
    let mut offset = 0;
    let mut begins = Vec::new();
    let mut ends = Vec::new();
    for line in existing.split_inclusive('\n') {
        let bare = line.trim_end();
        if bare.starts_with(begin) {
            begins.push(offset);
        } else if bare == end {
            ends.push(offset + line.len());
        }
        offset += line.len();
    }
    match (begins.as_slice(), ends.as_slice()) {
        ([], []) => Ok(existing.to_owned()),
        ([start], [stop]) if start < stop => {
            let head = &existing[..*start];
            let head = head.strip_suffix("\n\n").map_or(head.to_owned(), |h| format!("{h}\n"));
            Ok(format!("{head}{}", &existing[*stop..]))
        }
        _ => Err(format!(
            "{path} holds Aver's markers ({begin} … {end}) more than once or unmatched; remove the block by hand"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_undoes_upsert() {
        let b = format!("{BEGIN_LINE}\none\n{END}\n");
        for mine in ["# Mine\nkeep\n", "top\n"] {
            let with = upsert("f", Some(mine), BEGIN, END, &b).unwrap();
            assert_eq!(remove("f", &with, BEGIN, END).unwrap(), mine);
        }
        let around = format!("top\n{b}bottom\n");
        assert_eq!(remove("f", &around, BEGIN, END).unwrap(), "top\nbottom\n");
        assert_eq!(remove("f", &b, BEGIN, END).unwrap(), "");
        assert_eq!(remove("f", "plain\n", BEGIN, END).unwrap(), "plain\n");
        assert!(remove("f", &format!("{BEGIN_LINE}\n"), BEGIN, END).is_err());
    }

    #[test]
    fn the_skill_has_a_description_and_a_body() {
        assert!(description().len() <= 1024);
        assert!(description().contains("aver verify"));
        assert!(instructions().starts_with("## Aver"));
        assert!(!instructions().contains("\n---\n"));
    }

    #[test]
    fn upsert_appends_once_then_replaces_in_place() {
        let b1 = format!("{BEGIN_LINE}\none\n{END}\n");
        let b2 = format!("{BEGIN_LINE}\ntwo\n{END}\n");
        assert_eq!(upsert("f", None, BEGIN, END, &b1).unwrap(), b1);
        let once = upsert("f", Some("# Mine\nkeep"), BEGIN, END, &b1).unwrap();
        assert_eq!(once, format!("# Mine\nkeep\n\n{b1}"));
        assert_eq!(upsert("f", Some(&once), BEGIN, END, &b1).unwrap(), once);
        let around = format!("top\n{b1}bottom\n");
        assert_eq!(
            upsert("f", Some(&around), BEGIN, END, &b2).unwrap(),
            format!("top\n{b2}bottom\n")
        );
    }

    #[test]
    fn upsert_refuses_broken_markers() {
        for text in [
            format!("{BEGIN_LINE}\nno end\n"),
            format!("{END}\n{BEGIN_LINE}\n"),
            format!("{BEGIN_LINE}\n{END}\n{BEGIN_LINE}\n{END}\n"),
        ] {
            assert!(
                upsert("f", Some(&text), BEGIN, END, "x\n").is_err(),
                "{text}"
            );
        }
    }
}
