use regex::Regex;
use syntect::{easy::HighlightLines, highlighting::ThemeSet, parsing::SyntaxSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineKind {
    Context,
    Add,
    Remove,
    Hunk,
    Meta,
}

#[derive(Clone, Debug)]
pub struct SyntaxSpan {
    pub text: String,
    pub rgb: (u8, u8, u8),
    pub bold: bool,
    pub italic: bool,
}

#[derive(Clone, Debug)]
pub struct DiffLine {
    pub text: String,
    pub kind: LineKind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub syntax: Vec<SyntaxSpan>,
}

impl DiffLine {
    pub fn marker(&self) -> char {
        match self.kind {
            LineKind::Add => '+',
            LineKind::Remove => '-',
            _ => ' ',
        }
    }
    pub fn review_line(&self) -> Option<u32> {
        if self.kind == LineKind::Remove {
            self.old
        } else {
            self.new
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileStatus {
    Added,
    Deleted,
    Renamed,
    Modified,
}

#[derive(Clone, Debug)]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub lines: Vec<DiffLine>,
}

pub fn hunk_ranges(header: &str) -> Option<(u32, u32, u32, u32)> {
    let captures = Regex::new(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")
        .ok()?
        .captures(header)?;
    let old_start = captures[1].parse().ok()?;
    let old_count: u32 = captures
        .get(2)
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(1);
    let new_start = captures[3].parse().ok()?;
    let new_count: u32 = captures
        .get(4)
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(1);
    Some((
        old_start,
        old_start.saturating_add(old_count.saturating_sub(1)),
        new_start,
        new_start.saturating_add(new_count.saturating_sub(1)),
    ))
}

impl FileDiff {
    pub fn additions(&self) -> usize {
        self.lines
            .iter()
            .filter(|l| l.kind == LineKind::Add)
            .count()
    }
    pub fn deletions(&self) -> usize {
        self.lines
            .iter()
            .filter(|l| l.kind == LineKind::Remove)
            .count()
    }
}

pub fn parse_unified_diff(raw: &str) -> Vec<FileDiff> {
    let hunk = Regex::new(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@").unwrap();
    let mut files = Vec::new();
    let mut current: Option<FileDiff> = None;
    let mut old_path: Option<String> = None;
    let mut status = FileStatus::Modified;
    let (mut old_no, mut new_no) = (0, 0);
    for raw_line in raw.lines() {
        if raw_line.starts_with("diff --git ") {
            if let Some(file) = current.take() {
                files.push(file);
            }
            old_path = None;
            status = FileStatus::Modified;
            continue;
        }
        if raw_line.starts_with("new file mode ") {
            status = FileStatus::Added;
            continue;
        }
        if raw_line.starts_with("deleted file mode ") {
            status = FileStatus::Deleted;
            continue;
        }
        if let Some(path) = raw_line.strip_prefix("rename from ") {
            old_path = Some(path.into());
            status = FileStatus::Renamed;
            continue;
        }
        if raw_line.starts_with("rename to ") {
            status = FileStatus::Renamed;
            continue;
        }
        if let Some(path) = raw_line.strip_prefix("--- ") {
            old_path = Some(path.strip_prefix("a/").unwrap_or(path).to_string());
            continue;
        }
        if let Some(path) = raw_line.strip_prefix("+++ ") {
            let path = if path == "/dev/null" {
                old_path.clone().unwrap_or_else(|| path.into())
            } else {
                path.strip_prefix("b/").unwrap_or(path).into()
            };
            current = Some(FileDiff {
                path,
                old_path: old_path.clone(),
                status,
                lines: Vec::new(),
            });
            continue;
        }
        let Some(file) = current.as_mut() else {
            continue;
        };
        if let Some(c) = hunk.captures(raw_line) {
            old_no = c[1].parse().unwrap_or(0);
            new_no = c[3].parse().unwrap_or(0);
            file.lines.push(DiffLine {
                text: raw_line.into(),
                kind: LineKind::Hunk,
                old: None,
                new: None,
                syntax: Vec::new(),
            });
        } else if raw_line.starts_with('+') && !raw_line.starts_with("+++") {
            file.lines.push(DiffLine {
                text: raw_line[1..].into(),
                kind: LineKind::Add,
                old: None,
                new: Some(new_no),
                syntax: Vec::new(),
            });
            new_no += 1;
        } else if raw_line.starts_with('-') && !raw_line.starts_with("---") {
            file.lines.push(DiffLine {
                text: raw_line[1..].into(),
                kind: LineKind::Remove,
                old: Some(old_no),
                new: None,
                syntax: Vec::new(),
            });
            old_no += 1;
        } else if let Some(text) = raw_line.strip_prefix(' ') {
            file.lines.push(DiffLine {
                text: text.into(),
                kind: LineKind::Context,
                old: Some(old_no),
                new: Some(new_no),
                syntax: Vec::new(),
            });
            old_no += 1;
            new_no += 1;
        } else if raw_line.starts_with("\\ No newline") {
            file.lines.push(DiffLine {
                text: raw_line.into(),
                kind: LineKind::Meta,
                old: None,
                new: None,
                syntax: Vec::new(),
            });
        }
    }
    if let Some(file) = current {
        files.push(file);
    }
    for file in &mut files {
        highlight(file);
    }
    files
}

fn highlight(file: &mut FileDiff) {
    let syntaxes = SyntaxSet::load_defaults_newlines();
    let themes = ThemeSet::load_defaults();
    let Some(syntax) = syntaxes
        .find_syntax_for_file(&file.path)
        .ok()
        .flatten()
        .or_else(|| syntaxes.find_syntax_plain_text().into())
    else {
        return;
    };
    let theme = themes
        .themes
        .get("base16-ocean.dark")
        .or_else(|| themes.themes.values().next())
        .unwrap();
    let mut old = HighlightLines::new(syntax, theme);
    let mut new = HighlightLines::new(syntax, theme);
    for line in &mut file.lines {
        if line.kind == LineKind::Hunk {
            old = HighlightLines::new(syntax, theme);
            new = HighlightLines::new(syntax, theme);
            continue;
        }
        let ranges = match line.kind {
            LineKind::Remove => old.highlight_line(&line.text, &syntaxes),
            LineKind::Add => new.highlight_line(&line.text, &syntaxes),
            LineKind::Context => {
                let _ = old.highlight_line(&line.text, &syntaxes);
                new.highlight_line(&line.text, &syntaxes)
            }
            _ => continue,
        };
        if let Ok(ranges) = ranges {
            line.syntax = ranges
                .into_iter()
                .map(|(style, text)| SyntaxSpan {
                    text: text.into(),
                    rgb: (style.foreground.r, style.foreground.g, style.foreground.b),
                    bold: style
                        .font_style
                        .contains(syntect::highlighting::FontStyle::BOLD),
                    italic: style
                        .font_style
                        .contains(syntect::highlighting::FontStyle::ITALIC),
                })
                .collect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const SAMPLE: &str = "diff --git a/app.py b/app.py\n--- a/app.py\n+++ b/app.py\n@@ -1,2 +1,3 @@\n old\n-bad()\n+good()\n+extra()\n";
    #[test]
    fn parses_both_gutters() {
        let f = parse_unified_diff(SAMPLE);
        assert_eq!(f[0].additions(), 2);
        assert_eq!(f[0].lines[2].old, Some(2));
        assert_eq!(f[0].lines[3].new, Some(2));
    }

    #[test]
    fn parses_hunk_ranges() {
        assert_eq!(
            hunk_ranges("@@ -10,3 +12,5 @@ fn main"),
            Some((10, 12, 12, 16))
        );
    }
}
