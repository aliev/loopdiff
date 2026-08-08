use crate::{
    model::{DiffLine, FileDiff, FileStatus, LineKind, hunk_ranges},
    review::{Annotation, Message, MessageRole, ThreadStatus},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};
use regex::Regex;
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const BG: Color = Color::Rgb(13, 17, 23);
const SURFACE: Color = Color::Rgb(22, 27, 34);
const BORDER: Color = Color::Rgb(48, 54, 61);
const TEXT: Color = Color::Rgb(230, 237, 243);
const MUTED: Color = Color::Rgb(139, 148, 158);
const BLUE: Color = Color::Rgb(88, 166, 255);
const GREEN: Color = Color::Rgb(63, 185, 80);
const GREEN_BG: Color = Color::Rgb(18, 45, 29);
const RED: Color = Color::Rgb(248, 81, 73);
const RED_BG: Color = Color::Rgb(55, 23, 26);
const HUNK_BG: Color = Color::Rgb(17, 34, 54);
const COMMENT: Color = Color::Rgb(210, 153, 34);
const COMMENT_BG: Color = Color::Rgb(38, 32, 19);
const AI_COMMENT: Color = Color::Rgb(88, 166, 255);
const AI_COMMENT_BG: Color = Color::Rgb(17, 34, 54);
const SELECT_BG: Color = Color::Rgb(32, 52, 75);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Focus {
    Files,
    Diff,
    Filter,
    Editor,
}
pub enum Outcome {
    Continue,
    Finish,
    Yank(String),
}

#[derive(Clone)]
struct SideEntry {
    label: String,
    depth: usize,
    target: Option<SideTarget>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SideTarget {
    File(usize),
    Comment { file: usize, note: usize },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum VisualMode {
    Character {
        anchor_row: usize,
        anchor_col: usize,
    },
    Line {
        anchor_row: usize,
    },
}

pub struct App {
    pub files: Vec<FileDiff>,
    pub notes: Vec<Annotation>,
    file: usize,
    cursor: usize,
    scroll: usize,
    file_cursors: Vec<usize>,
    range_anchor: Option<usize>,
    visual_mode: Option<VisualMode>,
    visual_col: usize,
    focus: Focus,
    filter: String,
    search_restore_filter: String,
    search_return_focus: Focus,
    search_no_match: bool,
    editor: String,
    editor_cursor: usize,
    editor_anchor: Option<usize>,
    editing_key: Option<String>,
    replying_thread: Option<String>,
    vim_command: String,
    last_click: Option<(Instant, usize)>,
    diff_area: Rect,
    sidebar_area: Rect,
    row_map: Vec<Option<usize>>,
    sidebar_row_map: Vec<Option<SideTarget>>,
    sidebar_scroll_x: usize,
    sidebar_scroll_y: usize,
    sidebar_follow_selection: bool,
    sidebar_selection: Option<SideTarget>,
    help_open: bool,
    notice: Option<(String, Instant)>,
    comparison: String,
    review_output: String,
    reviewer_name: Option<String>,
    deleted_notes: Vec<(usize, Annotation)>,
}

impl App {
    pub fn new(files: Vec<FileDiff>, notes: Vec<Annotation>) -> Self {
        let count = files.len();
        let first = files
            .first()
            .and_then(|f| f.lines.iter().position(|l| l.review_line().is_some()))
            .unwrap_or(0);
        Self {
            files,
            notes,
            file: 0,
            cursor: first,
            scroll: 0,
            file_cursors: vec![0; count],
            range_anchor: None,
            visual_mode: None,
            visual_col: 0,
            focus: Focus::Diff,
            filter: String::new(),
            search_restore_filter: String::new(),
            search_return_focus: Focus::Diff,
            search_no_match: false,
            editor: String::new(),
            editor_cursor: 0,
            editor_anchor: None,
            editing_key: None,
            replying_thread: None,
            vim_command: String::new(),
            last_click: None,
            diff_area: Rect::default(),
            sidebar_area: Rect::default(),
            row_map: Vec::new(),
            sidebar_row_map: Vec::new(),
            sidebar_scroll_x: 0,
            sidebar_scroll_y: 0,
            sidebar_follow_selection: true,
            sidebar_selection: None,
            help_open: false,
            notice: None,
            comparison: String::new(),
            review_output: String::new(),
            reviewer_name: None,
            deleted_notes: Vec::new(),
        }
    }

    pub fn set_review_context(&mut self, comparison: String, review_output: String) {
        self.comparison = comparison;
        self.review_output = review_output;
    }

    pub fn set_reviewer_name(&mut self, reviewer_name: Option<String>) {
        self.reviewer_name = reviewer_name;
        if let Some(name) = &self.reviewer_name {
            for message in self
                .notes
                .iter_mut()
                .flat_map(|thread| &mut thread.messages)
                .filter(|message| message.role == MessageRole::Human && message.author.is_none())
            {
                message.author = Some(name.clone());
            }
        }
    }

    pub fn notice(&mut self, message: impl Into<String>) {
        self.notice = Some((message.into(), Instant::now()));
    }

    fn current(&self) -> &FileDiff {
        &self.files[self.file]
    }
    fn selected_bounds(&self) -> (usize, usize) {
        let a = self.range_anchor.unwrap_or(self.cursor);
        (a.min(self.cursor), a.max(self.cursor))
    }
    fn in_range(&self, p: usize) -> bool {
        self.range_anchor.is_some() && {
            let (a, b) = self.selected_bounds();
            a <= p && p <= b
        }
    }
    fn visual_line_selected(&self, row: usize) -> bool {
        match self.visual_mode {
            Some(VisualMode::Line { anchor_row }) => {
                let (start, end) = ordered(anchor_row, self.cursor);
                start <= row && row <= end
            }
            _ => false,
        }
    }
    fn visual_character_range(&self, row: usize) -> Option<(usize, usize)> {
        let VisualMode::Character {
            anchor_row,
            anchor_col,
        } = self.visual_mode?
        else {
            return None;
        };
        let ((start_row, start_col), (end_row, end_col)) =
            ordered_position((anchor_row, anchor_col), (self.cursor, self.visual_col));
        if !(start_row..=end_row).contains(&row) {
            return None;
        }
        let length = self.current().lines[row].text.chars().count();
        if length == 0 {
            return None;
        }
        let start = if row == start_row { start_col } else { 0 }.min(length - 1);
        let end = if row == end_row {
            end_col.min(length - 1)
        } else {
            length - 1
        };
        Some((start, end))
    }
    fn note_at(&self, p: usize) -> Option<&Annotation> {
        let path = &self.current().path;
        self.notes
            .iter()
            .find(|n| n.path == *path && anchor_position(self.current(), n) == Some(p))
            .or_else(|| {
                self.notes
                    .iter()
                    .find(|n| n.path == *path && line_in_note(&self.current().lines[p], n))
            })
    }
    fn anchored_note_at(&self, p: usize) -> Option<&Annotation> {
        let path = &self.current().path;
        self.notes
            .iter()
            .find(|n| n.path == *path && anchor_position(self.current(), n) == Some(p))
    }
    fn annotated(&self, p: usize) -> bool {
        self.note_at(p).is_some()
    }

    pub fn draw(&mut self, frame: &mut Frame) {
        let root = frame.area();
        frame.render_widget(Block::default().style(Style::default().bg(BG)), root);
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(8), Constraint::Length(1)])
            .split(root);
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(36), Constraint::Min(50)])
            .split(rows[0]);
        self.sidebar_area = body[0];
        self.draw_sidebar(frame, body[0]);
        self.draw_main(frame, body[1]);
        self.draw_footer(frame, rows[1]);
        if self.help_open {
            self.draw_help(frame, root);
        }
    }

    fn draw_help(&self, frame: &mut Frame, root: Rect) {
        let width = root.width.saturating_sub(4).min(72);
        let height = root.height.saturating_sub(2).min(24);
        let area = Rect {
            x: root.x + root.width.saturating_sub(width) / 2,
            y: root.y + root.height.saturating_sub(height) / 2,
            width,
            height,
        };
        let lines = vec![
            help_line("PANELS", "-", "toggle explorer / diff"),
            help_line("", "/", "search files"),
            help_line("", "?", "open / close this help"),
            Line::default(),
            help_line("NAVIGATION", "j k / ↑ ↓", "move"),
            help_line("", "Ctrl+U / Ctrl+D", "half-page up / down"),
            help_line("", "gg / G", "start / end"),
            help_line("", "{line}gg", "jump to line"),
            help_line("", "h l / ← →", "scroll explorer horizontally"),
            Line::default(),
            help_line("REVIEW", "c", "select lines for a comment"),
            help_line("", "v / Shift+V", "visual character / line mode"),
            help_line("", "y", "yank visual selection"),
            help_line("", "Enter", "add or edit comment"),
            help_line("", "r", "reply to thread"),
            help_line("", "[ / ]", "previous / next thread"),
            help_line("", "d", "delete thread"),
            help_line("", "u", "undo deleted thread"),
            Line::default(),
            help_line("EDITOR", "Enter", "save comment"),
            help_line("", "Shift+Enter", "insert newline"),
            help_line("", "Esc", "cancel"),
            Line::default(),
            help_line("SESSION", "q", "finish review"),
        ];
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(" Help · ? or Esc to close ")
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(BLUE)),
                )
                .style(Style::default().fg(TEXT).bg(SURFACE)),
            area,
        );
    }

    fn side_entries(&self) -> Vec<SideEntry> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for (i, file) in self.files.iter().enumerate() {
            if !self.filter.is_empty() && !fuzzy(&self.filter, &file.path) {
                continue;
            }
            let parts: Vec<_> = file.path.split('/').collect();
            for d in 0..parts.len().saturating_sub(1) {
                let key = parts[..=d].join("/");
                if seen.insert(key) {
                    out.push(SideEntry {
                        label: format!("▰  {}", parts[d]),
                        depth: d,
                        target: None,
                    });
                }
            }
            out.push(SideEntry {
                label: parts.last().unwrap_or(&file.path.as_str()).to_string(),
                depth: parts.len().saturating_sub(1),
                target: Some(SideTarget::File(i)),
            });
            for (note, annotation) in self
                .notes
                .iter()
                .enumerate()
                .filter(|(_, annotation)| annotation.path == file.path)
            {
                let text = annotation.first_text().replace('\n', " ");
                let mut short = text.chars().take(25).collect::<String>();
                if text.chars().count() > 25 {
                    short.push('…');
                }
                let replies = annotation.messages.len().saturating_sub(1);
                if replies > 0 {
                    short.push_str(&format!(
                        " · {replies} repl{}",
                        if replies == 1 { "y" } else { "ies" }
                    ));
                }
                out.push(SideEntry {
                    label: short,
                    depth: parts.len(),
                    target: Some(SideTarget::Comment { file: i, note }),
                });
            }
        }
        out
    }

    fn draw_sidebar(&mut self, f: &mut Frame, a: Rect) {
        let entries = self.side_entries();
        let content_width = entries
            .iter()
            .map(|entry| {
                entry.depth * 2
                    + match entry.target {
                        Some(SideTarget::File(_)) => {
                            3 + UnicodeWidthStr::width(entry.label.as_str())
                        }
                        Some(SideTarget::Comment { .. }) => {
                            3 + UnicodeWidthStr::width(entry.label.as_str())
                        }
                        None => UnicodeWidthStr::width(entry.label.as_str()),
                    }
            })
            .max()
            .unwrap_or(0);
        let viewport_width = a.width.saturating_sub(1) as usize;
        let max_scroll = content_width.saturating_sub(viewport_width);
        self.sidebar_scroll_x = self.sidebar_scroll_x.min(max_scroll);
        let scroll_x = self.sidebar_scroll_x;
        let list_area = a;
        let viewport_height = list_area.height as usize;
        let active_target = self
            .sidebar_selection
            .unwrap_or(SideTarget::File(self.file));
        let active_row = entries
            .iter()
            .position(|entry| entry.target == Some(active_target))
            .unwrap_or(0);
        let max_vertical_scroll = entries.len().saturating_sub(viewport_height);
        self.sidebar_scroll_y = self.sidebar_scroll_y.min(max_vertical_scroll);
        if self.sidebar_follow_selection {
            if active_row < self.sidebar_scroll_y {
                self.sidebar_scroll_y = active_row;
            } else if active_row >= self.sidebar_scroll_y.saturating_add(viewport_height) {
                self.sidebar_scroll_y =
                    active_row.saturating_sub(viewport_height.saturating_sub(1));
            }
            self.sidebar_follow_selection = false;
        }
        let visible_entries = entries
            .iter()
            .skip(self.sidebar_scroll_y)
            .take(viewport_height)
            .cloned()
            .collect::<Vec<_>>();
        self.sidebar_row_map = visible_entries.iter().map(|entry| entry.target).collect();
        let items = visible_entries
            .into_iter()
            .map(|e| {
                let indent = "  ".repeat(e.depth);
                if let Some(SideTarget::File(i)) = e.target {
                    let file = &self.files[i];
                    let mut spans = vec![Span::raw(indent)];
                    spans.extend(file_status_spans(file.status));
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(e.label, Style::default().fg(TEXT)));
                    let style = if i == self.file {
                        Style::default().bg(SELECT_BG)
                    } else {
                        Style::default()
                    };
                    ListItem::new(Line::from(crop_spans(spans, scroll_x, viewport_width)))
                        .style(style)
                } else if let Some(SideTarget::Comment { file, note }) = e.target {
                    let target = SideTarget::Comment { file, note };
                    let selected =
                        if self.focus == Focus::Files {
                            self.sidebar_selection == Some(target)
                        } else {
                            file == self.file
                                && self.notes.get(note).and_then(|annotation| {
                                    anchor_position(self.current(), annotation)
                                }) == Some(self.cursor)
                        };
                    let style = if selected {
                        Style::default().bg(SELECT_BG)
                    } else {
                        Style::default()
                    };
                    ListItem::new(Line::from(crop_spans(
                        vec![
                            Span::raw(indent),
                            Span::styled("└─ ", Style::default().fg(BORDER)),
                            Span::styled(
                                e.label,
                                Style::default().fg(if selected { TEXT } else { MUTED }),
                            ),
                        ],
                        scroll_x,
                        viewport_width,
                    )))
                    .style(style)
                } else {
                    ListItem::new(Line::from(crop_spans(
                        vec![
                            Span::raw(indent),
                            Span::styled(e.label, Style::default().fg(MUTED)),
                        ],
                        scroll_x,
                        viewport_width,
                    )))
                }
            })
            .collect::<Vec<_>>();
        f.render_widget(
            List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::RIGHT)
                        .border_style(Style::default().fg(
                            if matches!(self.focus, Focus::Files | Focus::Filter) {
                                BLUE
                            } else {
                                BORDER
                            },
                        )),
                )
                .style(Style::default().bg(SURFACE)),
            list_area,
        );
    }

    fn draw_main(&mut self, f: &mut Frame, a: Rect) {
        let parts = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Min(3)])
            .split(a);
        let file = self.current();
        let shown_path = if file.status == FileStatus::Renamed {
            format!(
                "{} → {}",
                file.old_path.as_deref().unwrap_or("?"),
                file.path
            )
        } else {
            file.path.clone()
        };
        let h = vec![
            Span::styled(
                format!(" {shown_path}"),
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(format!("+{}", file.additions()), Style::default().fg(GREEN)),
            Span::styled(format!(" −{}", file.deletions()), Style::default().fg(RED)),
        ];
        f.render_widget(
            Paragraph::new(Line::from(h))
                .block(
                    Block::default()
                        .borders(Borders::BOTTOM)
                        .border_style(Style::default().fg(
                            if matches!(self.focus, Focus::Diff | Focus::Editor) {
                                BLUE
                            } else {
                                BORDER
                            },
                        )),
                )
                .style(Style::default().bg(SURFACE)),
            parts[0],
        );
        self.diff_area = parts[1];
        self.draw_diff(f, parts[1]);
    }

    fn draw_diff(&mut self, f: &mut Frame, a: Rect) {
        let height = a.height as usize;
        self.ensure_visible(height);
        let mut lines = Vec::new();
        let mut map = Vec::new();
        let file = self.current().clone();
        for p in self.scroll..file.lines.len() {
            if lines.len() >= height {
                break;
            }
            let l = &file.lines[p];
            lines.push(self.diff_line(l, p, a.width as usize));
            map.push(Some(p));
            let editor_here = Some(p) == self.editor_anchor && self.focus == Focus::Editor;
            if editor_here && self.replying_thread.is_none() {
                let title = if self.editing_key.is_some() {
                    "Edit comment"
                } else {
                    "Add comment"
                };
                self.append_editor(&mut lines, &mut map, title, a.width as usize);
            }
            for n in self
                .notes
                .iter()
                .filter(|n| n.path == file.path && anchor_position(&file, n) == Some(p))
            {
                for message in &n.messages {
                    for comment_line in inline_message_lines(message, a.width as usize) {
                        lines.push(comment_line);
                        map.push(None);
                    }
                }
                if editor_here && self.replying_thread.as_deref() == Some(n.id.as_str()) {
                    self.append_editor(&mut lines, &mut map, "Reply", a.width as usize);
                }
            }
        }
        self.row_map = map;
        f.render_widget(Paragraph::new(lines).style(Style::default().bg(BG)), a);
    }

    fn append_editor<'a>(
        &self,
        lines: &mut Vec<Line<'a>>,
        map: &mut Vec<Option<usize>>,
        title: &str,
        width: usize,
    ) {
        lines.push(Line::from(Span::styled(
            format!("             ┌─ {title}"),
            Style::default().fg(BLUE),
        )));
        map.push(None);
        const EDITOR_PREFIX_WIDTH: usize = 15;
        let visual_rows = editor_visual_rows(
            &self.editor,
            width.saturating_sub(EDITOR_PREFIX_WIDTH).max(1),
        );
        let cursor_row = visual_rows
            .iter()
            .rposition(|(start, _)| *start <= self.editor_cursor)
            .unwrap_or(0);
        for (index, (start, end)) in visual_rows.into_iter().enumerate() {
            let edit_line = &self.editor[start..end];
            let mut editor_spans = vec![Span::raw("             │ ")];
            if index == cursor_row {
                let cursor_column = self
                    .editor_cursor
                    .saturating_sub(start)
                    .min(edit_line.len());
                let before = &edit_line[..cursor_column];
                let after = &edit_line[cursor_column..];
                editor_spans.push(Span::styled(before.to_owned(), Style::default().fg(TEXT)));
                if let Some(character) = after.chars().next() {
                    editor_spans.push(Span::styled(
                        character.to_string(),
                        Style::default().fg(BG).bg(TEXT),
                    ));
                    editor_spans.push(Span::styled(
                        after[character.len_utf8()..].to_owned(),
                        Style::default().fg(TEXT),
                    ));
                } else {
                    editor_spans.push(Span::styled(" ", Style::default().bg(TEXT)));
                }
            } else {
                editor_spans.push(Span::styled(
                    edit_line.to_owned(),
                    Style::default().fg(TEXT),
                ));
            }
            lines.push(Line::from(editor_spans));
            map.push(None);
        }
        lines.push(Line::from(Span::styled(
            "             └─ Enter save · Shift+Enter newline · Esc cancel",
            Style::default().fg(MUTED),
        )));
        map.push(None);
    }

    fn diff_line<'a>(&self, l: &'a DiffLine, p: usize, width: usize) -> Line<'a> {
        let base_bg = match l.kind {
            LineKind::Add => GREEN_BG,
            LineKind::Remove => RED_BG,
            LineKind::Hunk => HUNK_BG,
            _ => BG,
        };
        let bg = if self.visual_line_selected(p) {
            SELECT_BG
        } else {
            base_bg
        };
        let sign = if self.in_range(p) {
            Span::styled("▌", Style::default().fg(BLUE))
        } else if self.annotated(p) {
            Span::styled("▌", Style::default().fg(COMMENT))
        } else {
            Span::raw(" ")
        };
        let old = l.old.map_or("    ".into(), |v| format!("{v:>4}"));
        let new = l.new.map_or("    ".into(), |v| format!("{v:>4}"));
        let marker = match l.kind {
            LineKind::Add => Style::default().fg(GREEN),
            LineKind::Remove => Style::default().fg(RED),
            _ => Style::default().fg(MUTED),
        };
        let mut spans = vec![
            sign,
            Span::styled(format!("{old} {new} "), Style::default().fg(MUTED)),
            Span::styled(format!("{} ", l.marker()), marker),
        ];
        if l.kind == LineKind::Hunk {
            let re = Regex::new(r"^(@@.*?@@)(.*)$").unwrap();
            if let Some(c) = re.captures(&l.text) {
                spans.push(Span::styled(c[1].to_string(), Style::default().fg(BLUE)));
                spans.push(Span::styled(c[2].to_string(), Style::default().fg(MUTED)));
            } else {
                spans.push(Span::styled(l.text.clone(), Style::default().fg(BLUE)));
            }
        } else if l.syntax.is_empty() {
            spans.push(Span::styled(l.text.clone(), Style::default().fg(TEXT)));
        } else {
            for s in &l.syntax {
                let mut st = Style::default().fg(Color::Rgb(s.rgb.0, s.rgb.1, s.rgb.2));
                if s.bold {
                    st = st.add_modifier(Modifier::BOLD)
                }
                if s.italic {
                    st = st.add_modifier(Modifier::ITALIC)
                }
                spans.push(Span::styled(s.text.clone(), st));
            }
        }
        for span in &mut spans {
            if span.style.bg.is_none() {
                span.style = span.style.bg(bg);
            }
        }
        if let Some((start, end)) = self.visual_character_range(p) {
            apply_character_selection(
                &mut spans,
                3,
                start,
                end,
                (p == self.cursor).then_some(self.visual_col.clamp(start, end)),
            );
        } else if p == self.cursor && self.visual_mode.is_none() {
            apply_block_cursor(&mut spans, 3, self.visual_col, bg);
        }
        let content_width = spans
            .iter()
            .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
            .sum::<usize>();
        if content_width < width {
            spans.push(Span::styled(
                " ".repeat(width - content_width),
                Style::default().bg(bg),
            ));
        }
        Line::from(spans)
    }

    fn draw_footer(&self, f: &mut Frame, a: Rect) {
        let width = a.width as usize;
        let (mut left, right) = if self.focus == Focus::Filter {
            let mut prompt = vec![
                Span::styled(" /", Style::default().fg(BLUE).add_modifier(Modifier::BOLD)),
                Span::styled(self.filter.clone(), Style::default().fg(TEXT)),
                Span::styled(" ", Style::default().bg(TEXT)),
            ];
            let right = if self.search_no_match {
                Span::styled(" no matches ", Style::default().fg(RED))
            } else {
                Span::styled(
                    " Enter accept · Esc cancel · Ctrl+U clear ",
                    Style::default().fg(MUTED),
                )
            };
            (std::mem::take(&mut prompt), right)
        } else {
            let (mode, mode_color) = match self.focus {
                Focus::Files => (" FILES ", COMMENT),
                Focus::Editor => (" COMMENT ", GREEN),
                _ if self.visual_mode.is_some() => (" VISUAL ", COMMENT),
                _ if self.range_anchor.is_some() => (" COMMENT SELECT ", BLUE),
                _ => (" NORMAL ", BLUE),
            };
            let detail = if !self.vim_command.is_empty() {
                self.vim_command.clone()
            } else {
                self.current().path.clone()
            };
            let session = match (self.comparison.is_empty(), self.review_output.is_empty()) {
                (true, true) => "? help".into(),
                (false, true) => format!("{} · ? help", self.comparison),
                (true, false) => format!("review {} · ? help", self.review_output),
                (false, false) => {
                    format!(
                        "{} · review {} · ? help",
                        self.comparison, self.review_output
                    )
                }
            };
            let right = if let Some((message, shown_at)) = &self.notice
                && shown_at.elapsed() < Duration::from_secs(4)
            {
                if session.is_empty() {
                    format!(" {message} ")
                } else {
                    format!(" {message} · {session} ")
                }
            } else {
                let state = match self.focus {
                    Focus::Files => format!(
                        " {} files · {} threads ",
                        self.files.len(),
                        self.notes.len()
                    ),
                    Focus::Editor => " Enter save · Shift+Enter newline · Esc cancel ".into(),
                    _ => {
                        let line = &self.current().lines[self.cursor];
                        let location = match (line.old, line.new) {
                            (_, Some(number)) => format!("new L{number}"),
                            (Some(number), None) => format!("old L{number}"),
                            _ => "hunk".into(),
                        };
                        let percent = (self.cursor + 1) * 100 / self.current().lines.len().max(1);
                        format!(" {location} · {percent}% ")
                    }
                };
                if session.is_empty() {
                    state
                } else {
                    format!(" {} ·{}", session, state)
                }
            };
            (
                vec![
                    Span::styled(
                        mode,
                        Style::default()
                            .fg(BG)
                            .bg(mode_color)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!(" {detail}"), Style::default().fg(TEXT)),
                ],
                Span::styled(right, Style::default().fg(MUTED)),
            )
        };
        let right_width = UnicodeWidthStr::width(right.content.as_ref()).min(width);
        let left_width = width.saturating_sub(right_width);
        left = crop_spans(left, 0, left_width);
        let rendered_left_width = left
            .iter()
            .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
            .sum::<usize>();
        left.push(Span::raw(
            " ".repeat(left_width.saturating_sub(rendered_left_width)),
        ));
        left.push(right);
        f.render_widget(
            Paragraph::new(Line::from(left)).style(Style::default().fg(MUTED).bg(SURFACE)),
            a,
        );
    }

    fn ensure_visible(&mut self, h: usize) {
        if self.cursor < self.scroll {
            self.scroll = self.cursor
        }
        if self.cursor >= self.scroll + h.saturating_sub(1) {
            self.scroll = self.cursor.saturating_sub(h.saturating_sub(2))
        }
    }
    fn move_cursor(&mut self, d: isize) {
        let max = self.current().lines.len().saturating_sub(1) as isize;
        self.cursor = (self.cursor as isize + d).clamp(0, max) as usize;
        let column_max = self.current().lines[self.cursor]
            .text
            .chars()
            .count()
            .saturating_sub(1);
        self.visual_col = self.visual_col.min(column_max);
    }
    fn jump_to_line(&mut self, line: u32) {
        let exact = self
            .current()
            .lines
            .iter()
            .position(|diff_line| diff_line.new == Some(line))
            .or_else(|| {
                self.current()
                    .lines
                    .iter()
                    .position(|diff_line| diff_line.old == Some(line))
            });
        let nearest = || {
            self.current()
                .lines
                .iter()
                .enumerate()
                .filter_map(|(position, diff_line)| {
                    diff_line
                        .new
                        .or(diff_line.old)
                        .map(|number| (position, number.abs_diff(line)))
                })
                .min_by_key(|(_, distance)| *distance)
                .map(|(position, _)| position)
        };
        if let Some(position) = exact.or_else(nearest) {
            self.cursor = position;
        }
    }
    fn switch_file(&mut self, i: usize) {
        if i >= self.files.len() {
            return;
        }
        self.file_cursors[self.file] = self.cursor;
        self.file = i;
        self.cursor = self.file_cursors[i].min(self.current().lines.len().saturating_sub(1));
        self.scroll = 0;
        self.range_anchor = None
    }

    fn select_side_target(&mut self, target: SideTarget, keep_sidebar_focus: bool) {
        match target {
            SideTarget::File(file) => self.switch_file(file),
            SideTarget::Comment { file, note } => {
                self.switch_file(file);
                if let Some(position) = self
                    .notes
                    .get(note)
                    .and_then(|annotation| anchor_position(self.current(), annotation))
                {
                    self.cursor = position;
                }
            }
        }
        self.focus = if keep_sidebar_focus {
            Focus::Files
        } else {
            Focus::Diff
        };
        self.sidebar_selection = keep_sidebar_focus.then_some(target);
        self.sidebar_follow_selection = keep_sidebar_focus;
    }

    fn move_sidebar(&mut self, delta: isize) {
        let targets = self
            .side_entries()
            .into_iter()
            .filter_map(|entry| entry.target)
            .collect::<Vec<_>>();
        if targets.is_empty() {
            return;
        }
        let current_comment = self
            .notes
            .iter()
            .enumerate()
            .find_map(|(note, annotation)| {
                (annotation.path == self.current().path
                    && anchor_position(self.current(), annotation) == Some(self.cursor))
                .then_some(SideTarget::Comment {
                    file: self.file,
                    note,
                })
            });
        let current = self
            .sidebar_selection
            .or(current_comment)
            .unwrap_or(SideTarget::File(self.file));
        let index = targets
            .iter()
            .position(|target| *target == current)
            .unwrap_or(0);
        let next = (index as isize + delta).clamp(0, targets.len() as isize - 1) as usize;
        self.select_side_target(targets[next], true);
    }

    fn begin_search(&mut self) {
        self.search_return_focus = self.focus;
        self.search_restore_filter = self.filter.clone();
        self.search_no_match = false;
        self.focus = Focus::Filter;
    }

    fn toggle_panel_focus(&mut self) {
        if self.focus == Focus::Files {
            self.sidebar_selection = None;
            self.focus = Focus::Diff;
            return;
        }
        let selected_comment = self.notes.iter().enumerate().find_map(|(note, thread)| {
            (thread.path == self.current().path
                && anchor_position(self.current(), thread) == Some(self.cursor))
            .then_some(SideTarget::Comment {
                file: self.file,
                note,
            })
        });
        self.sidebar_selection = Some(selected_comment.unwrap_or(SideTarget::File(self.file)));
        self.sidebar_follow_selection = true;
        self.focus = Focus::Files;
    }

    pub fn key(&mut self, k: KeyEvent) -> Outcome {
        if self.help_open {
            if matches!(k.code, KeyCode::Esc | KeyCode::Char('?')) {
                self.help_open = false;
            }
            return Outcome::Continue;
        }
        if self.focus == Focus::Editor {
            return self.editor_key(k);
        }
        if self.focus == Focus::Filter {
            return self.filter_key(k);
        }
        if k.modifiers.contains(KeyModifiers::CONTROL) {
            match k.code {
                KeyCode::Char('d') => {
                    self.vim_command.clear();
                    let n = (self.diff_area.height / 2).max(1) as isize;
                    self.move_cursor(n);
                    return Outcome::Continue;
                }
                KeyCode::Char('u') => {
                    self.vim_command.clear();
                    let n = (self.diff_area.height / 2).max(1) as isize;
                    self.move_cursor(-n);
                    return Outcome::Continue;
                }
                _ => {}
            }
        }
        if self.focus == Focus::Diff {
            match k.code {
                KeyCode::Char(character) if character.is_ascii_digit() => {
                    if self.vim_command.chars().all(|part| part.is_ascii_digit()) {
                        self.vim_command.push(character);
                    } else {
                        self.vim_command.clear();
                        self.vim_command.push(character);
                    }
                    return Outcome::Continue;
                }
                KeyCode::Char('g') => {
                    if self.vim_command.ends_with('g') {
                        let count = self.vim_command.trim_end_matches('g').parse::<u32>().ok();
                        if let Some(line) = count {
                            self.jump_to_line(line);
                        } else {
                            self.cursor = 0;
                        }
                        self.vim_command.clear();
                    } else if self.vim_command.chars().all(|part| part.is_ascii_digit()) {
                        self.vim_command.push('g');
                    } else {
                        self.vim_command.clear();
                    }
                    return Outcome::Continue;
                }
                _ => {}
            }
        }
        if !matches!(k.code, KeyCode::Esc) {
            self.vim_command.clear();
        }
        match k.code {
            KeyCode::Char('G') => self.cursor = self.current().lines.len().saturating_sub(1),
            KeyCode::Char('j') | KeyCode::Down => {
                if self.focus == Focus::Files {
                    self.move_sidebar(1)
                } else {
                    self.move_cursor(1)
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.focus == Focus::Files {
                    self.move_sidebar(-1)
                } else {
                    self.move_cursor(-1)
                }
            }
            KeyCode::Char('h') | KeyCode::Left if self.focus == Focus::Files => {
                self.sidebar_scroll_x = self.sidebar_scroll_x.saturating_sub(2)
            }
            KeyCode::Char('l') | KeyCode::Right if self.focus == Focus::Files => {
                self.sidebar_scroll_x = self.sidebar_scroll_x.saturating_add(2)
            }
            KeyCode::Char('h') | KeyCode::Left if self.focus == Focus::Diff => {
                self.visual_col = self.visual_col.saturating_sub(1)
            }
            KeyCode::Char('l') | KeyCode::Right if self.focus == Focus::Diff => {
                let max = self.current().lines[self.cursor]
                    .text
                    .chars()
                    .count()
                    .saturating_sub(1);
                self.visual_col = (self.visual_col + 1).min(max);
            }
            KeyCode::Char('c') => {
                self.visual_mode = None;
                self.range_anchor = if self.range_anchor.is_some() {
                    None
                } else {
                    Some(self.cursor)
                }
            }
            KeyCode::Char('v') => {
                self.range_anchor = None;
                self.visual_mode = if matches!(self.visual_mode, Some(VisualMode::Character { .. }))
                {
                    None
                } else {
                    Some(VisualMode::Character {
                        anchor_row: self.cursor,
                        anchor_col: self.visual_col,
                    })
                };
            }
            KeyCode::Char('V') => {
                self.range_anchor = None;
                self.visual_mode = if matches!(self.visual_mode, Some(VisualMode::Line { .. })) {
                    None
                } else {
                    Some(VisualMode::Line {
                        anchor_row: self.cursor,
                    })
                };
            }
            KeyCode::Char('y') => {
                if let Some(code) = self.yank_selection() {
                    return Outcome::Yank(code);
                }
            }
            KeyCode::Enter if self.visual_mode.is_none() => self.open_editor(),
            KeyCode::Char('r') => self.open_reply(),
            KeyCode::Char('d') => self.delete_note(),
            KeyCode::Char('u') => self.undo_delete_note(),
            KeyCode::Char(']') => self.jump_note(true),
            KeyCode::Char('[') => self.jump_note(false),
            KeyCode::Char('/') => {
                self.begin_search();
            }
            KeyCode::Char('-') | KeyCode::Tab => self.toggle_panel_focus(),
            KeyCode::Char('?') => self.help_open = true,
            KeyCode::Esc => {
                self.range_anchor = None;
                self.visual_mode = None;
                self.vim_command.clear();
            }
            KeyCode::Char('q') => return Outcome::Finish,
            _ => {}
        }
        Outcome::Continue
    }

    fn yank_selection(&mut self) -> Option<String> {
        let mode = self.visual_mode?;
        let code = match mode {
            VisualMode::Line { anchor_row } => {
                let (start, end) = ordered(anchor_row, self.cursor);
                self.current().lines[start..=end]
                    .iter()
                    .filter(|line| line.kind != LineKind::Meta)
                    .map(|line| line.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            VisualMode::Character {
                anchor_row,
                anchor_col,
            } => self.character_selection(anchor_row, anchor_col),
        };
        if code.is_empty() {
            return None;
        }
        let lines = code.lines().count();
        self.visual_mode = None;
        self.notice(format!(
            "yanked {lines} line{}",
            if lines == 1 { "" } else { "s" }
        ));
        Some(code)
    }

    fn character_selection(&self, anchor_row: usize, anchor_col: usize) -> String {
        let ((start_row, start_col), (end_row, end_col)) =
            ordered_position((anchor_row, anchor_col), (self.cursor, self.visual_col));
        (start_row..=end_row)
            .filter_map(|row| {
                let line = &self.current().lines[row];
                if line.kind == LineKind::Meta {
                    return None;
                }
                let length = line.text.chars().count();
                let start = if row == start_row { start_col } else { 0 }.min(length);
                let end = if row == end_row {
                    (end_col + 1).min(length)
                } else {
                    length
                };
                Some(
                    line.text
                        .chars()
                        .skip(start)
                        .take(end.saturating_sub(start))
                        .collect::<String>(),
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn filter_key(&mut self, k: KeyEvent) -> Outcome {
        match k.code {
            KeyCode::Esc => {
                self.filter = self.search_restore_filter.clone();
                self.focus = self.search_return_focus;
                self.search_no_match = false;
            }
            KeyCode::Enter => {
                if self.filter.is_empty() {
                    self.focus = Focus::Files;
                    self.sidebar_selection = Some(SideTarget::File(self.file));
                    self.sidebar_follow_selection = true;
                    return Outcome::Continue;
                }
                if let Some(file) = self
                    .side_entries()
                    .into_iter()
                    .find_map(|e| match e.target {
                        Some(SideTarget::File(i)) => Some(i),
                        _ => None,
                    })
                {
                    self.select_side_target(SideTarget::File(file), true);
                    self.search_restore_filter = self.filter.clone();
                    self.search_no_match = false;
                } else {
                    self.search_no_match = true;
                }
            }
            KeyCode::Backspace => {
                self.filter.pop();
                self.search_no_match = false;
            }
            KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                self.filter.clear();
                self.search_no_match = false;
            }
            KeyCode::Char(c)
                if !k.modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) =>
            {
                self.filter.push(c);
                self.search_no_match = false;
            }
            _ => {}
        }
        Outcome::Continue
    }
    fn editor_key(&mut self, k: KeyEvent) -> Outcome {
        match k.code {
            KeyCode::Esc => {
                self.focus = Focus::Diff;
                self.editor_anchor = None;
                self.editor.clear();
                self.editor_cursor = 0;
                self.editing_key = None;
                self.replying_thread = None;
            }
            KeyCode::Enter if k.modifiers.contains(KeyModifiers::SHIFT) => {
                self.editor.insert(self.editor_cursor, '\n');
                self.editor_cursor += 1;
            }
            KeyCode::Enter => {
                self.save_editor();
            }
            KeyCode::Char('j') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                self.editor.insert(self.editor_cursor, '\n');
                self.editor_cursor += 1;
            }
            KeyCode::Backspace => {
                if let Some(previous) = previous_boundary(&self.editor, self.editor_cursor) {
                    self.editor.drain(previous..self.editor_cursor);
                    self.editor_cursor = previous;
                }
            }
            KeyCode::Left => {
                if let Some(previous) = previous_boundary(&self.editor, self.editor_cursor) {
                    self.editor_cursor = previous;
                }
            }
            KeyCode::Right => {
                if let Some(next) = next_boundary(&self.editor, self.editor_cursor) {
                    self.editor_cursor = next;
                }
            }
            KeyCode::Up => {
                self.editor_cursor = vertical_cursor(&self.editor, self.editor_cursor, false)
            }
            KeyCode::Down => {
                self.editor_cursor = vertical_cursor(&self.editor, self.editor_cursor, true)
            }
            KeyCode::Char(c)
                if !k.modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) =>
            {
                self.editor.insert(self.editor_cursor, c);
                self.editor_cursor += c.len_utf8();
            }
            _ => {}
        }
        Outcome::Continue
    }

    fn open_editor(&mut self) {
        if matches!(self.current().lines[self.cursor].kind, LineKind::Meta) {
            return;
        }
        let existing = self.anchored_note_at(self.cursor).cloned();
        if existing
            .as_ref()
            .and_then(|thread| thread.messages.first())
            .is_some_and(|message| message.role == MessageRole::Assistant)
        {
            self.open_reply();
            return;
        }
        self.editor = existing
            .as_ref()
            .map_or(String::new(), |thread| thread.first_text().to_owned());
        self.editor_cursor = self.editor.len();
        self.editing_key = existing.as_ref().map(|n| n.key());
        self.replying_thread = None;
        let range_bottom = self.selected_bounds().1;
        self.editor_anchor = existing
            .as_ref()
            .and_then(|n| anchor_position(self.current(), n))
            .or(Some(range_bottom));
        self.focus = Focus::Editor
    }
    fn open_reply(&mut self) {
        let Some(thread) = self.anchored_note_at(self.cursor).cloned() else {
            return;
        };
        self.editor.clear();
        self.editor_cursor = 0;
        self.editing_key = None;
        self.editor_anchor = anchor_position(self.current(), &thread).or(Some(self.cursor));
        self.replying_thread = Some(thread.id);
        self.focus = Focus::Editor;
    }
    fn save_editor(&mut self) {
        if self.editor.trim().is_empty() {
            self.focus = Focus::Diff;
            self.editor_anchor = None;
            self.editing_key = None;
            self.replying_thread = None;
            return;
        }
        if let Some(thread_id) = self.replying_thread.take() {
            let message_id = next_id(
                "m",
                self.notes
                    .iter()
                    .flat_map(|thread| thread.messages.iter().map(|message| message.id.as_str())),
            );
            if let Some(thread) = self.notes.iter_mut().find(|thread| thread.id == thread_id) {
                thread.messages.push(Message {
                    id: message_id,
                    role: MessageRole::Human,
                    author: self.reviewer_name.clone(),
                    text: self.editor.trim().into(),
                });
            }
            self.range_anchor = None;
            self.focus = Focus::Diff;
            self.editor_anchor = None;
            self.editor.clear();
            self.editor_cursor = 0;
            return;
        }
        if let Some(thread_id) = self.editing_key.take() {
            if let Some(thread) = self.notes.iter_mut().find(|thread| thread.id == thread_id)
                && let Some(message) = thread.messages.first_mut()
            {
                message.text = self.editor.trim().into();
            }
            self.range_anchor = None;
            self.focus = Focus::Diff;
            self.editor_anchor = None;
            self.editor.clear();
            self.editor_cursor = 0;
            return;
        }
        let (a, b) = self.selected_bounds();
        let lines = &self.current().lines[a..=b];
        if lines.iter().any(|l| l.kind == LineKind::Meta) {
            return;
        }
        let nums = |old: bool| {
            lines
                .iter()
                .filter_map(|l| if old { l.old } else { l.new })
                .collect::<Vec<_>>()
        };
        let mut o = nums(true);
        let mut n = nums(false);
        if o.is_empty()
            && n.is_empty()
            && lines.len() == 1
            && let Some((old_start, old_end, new_start, new_end)) = hunk_ranges(&lines[0].text)
        {
            o.extend([old_start, old_end]);
            n.extend([new_start, new_end]);
        }
        let excerpt = lines
            .iter()
            .map(|l| {
                format!(
                    "{}{}",
                    if l.kind == LineKind::Hunk {
                        ""
                    } else {
                        match l.kind {
                            LineKind::Add => "+",
                            LineKind::Remove => "-",
                            _ => " ",
                        }
                    },
                    l.text
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let anchor_position = self.editor_anchor.unwrap_or(b);
        let anchor = &self.current().lines[anchor_position];
        let thread_id = next_id("t", self.notes.iter().map(|thread| thread.id.as_str()));
        let message_id = next_id(
            "m",
            self.notes
                .iter()
                .flat_map(|thread| thread.messages.iter().map(|message| message.id.as_str())),
        );
        let note = Annotation {
            id: thread_id,
            path: self.current().path.clone(),
            excerpt,
            old_start: o.first().copied(),
            old_end: o.last().copied(),
            new_start: n.first().copied(),
            new_end: n.last().copied(),
            anchor_old: anchor.old,
            anchor_new: anchor.new,
            status: ThreadStatus::Open,
            messages: vec![Message {
                id: message_id,
                role: MessageRole::Human,
                author: self.reviewer_name.clone(),
                text: self.editor.trim().into(),
            }],
        };
        self.deleted_notes.clear();
        self.notes.push(note);
        self.range_anchor = None;
        self.focus = Focus::Diff;
        self.editor_anchor = None;
        self.editor.clear();
        self.editor_cursor = 0;
    }
    fn delete_note(&mut self) {
        if let Some(key) = self.note_at(self.cursor).map(|note| note.key())
            && let Some(index) = self.notes.iter().position(|note| note.key() == key)
        {
            let note = self.notes.remove(index);
            self.deleted_notes.push((index, note));
            if self.focus == Focus::Files {
                self.sidebar_selection = Some(SideTarget::File(self.file));
            }
            self.notice("thread deleted · u undo");
        }
    }

    fn undo_delete_note(&mut self) {
        if let Some((index, note)) = self.deleted_notes.pop() {
            let restored = index.min(self.notes.len());
            self.notes.insert(restored, note);
            if self.focus == Focus::Files {
                self.sidebar_selection = Some(SideTarget::Comment {
                    file: self.file,
                    note: restored,
                });
            }
            self.notice("deleted thread restored");
        }
    }
    fn jump_note(&mut self, next: bool) {
        let mut p = self
            .notes
            .iter()
            .filter(|n| n.path == self.current().path)
            .filter_map(|n| anchor_position(self.current(), n))
            .collect::<Vec<_>>();
        p.sort_unstable();
        if p.is_empty() {
            return;
        }
        self.cursor = if next {
            p.iter().copied().find(|x| *x > self.cursor).unwrap_or(p[0])
        } else {
            p.iter()
                .rev()
                .copied()
                .find(|x| *x < self.cursor)
                .unwrap_or(*p.last().unwrap())
        }
    }

    pub fn mouse(&mut self, m: MouseEvent) {
        if self.help_open {
            return;
        }
        match m.kind {
            MouseEventKind::ScrollDown => {
                if self.sidebar_area.contains((m.column, m.row).into()) {
                    self.sidebar_scroll_y = self.sidebar_scroll_y.saturating_add(3);
                    self.sidebar_follow_selection = false;
                } else {
                    self.move_cursor(3)
                }
            }
            MouseEventKind::ScrollUp => {
                if self.sidebar_area.contains((m.column, m.row).into()) {
                    self.sidebar_scroll_y = self.sidebar_scroll_y.saturating_sub(3);
                    self.sidebar_follow_selection = false;
                } else {
                    self.move_cursor(-3)
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if self.diff_area.contains((m.column, m.row).into()) {
                    let row = m.row.saturating_sub(self.diff_area.y) as usize;
                    if let Some(p) = self.row_map.get(row).copied().flatten() {
                        self.cursor = p;
                        self.focus = Focus::Diff;
                        let now = Instant::now();
                        if self.last_click.is_some_and(|(t, x)| {
                            x == p && now.duration_since(t) < Duration::from_millis(450)
                        }) {
                            self.open_editor()
                        }
                        self.last_click = Some((now, p));
                    }
                } else if self.sidebar_area.contains((m.column, m.row).into()) {
                    self.focus = Focus::Files;
                    let row = m.row.saturating_sub(self.sidebar_area.y) as usize;
                    if let Some(target) = self.sidebar_row_map.get(row).copied().flatten() {
                        self.select_side_target(target, true);
                    }
                }
            }
            _ => {}
        }
    }
}

fn ordered(first: usize, second: usize) -> (usize, usize) {
    (first.min(second), first.max(second))
}

fn file_status_spans(status: FileStatus) -> Vec<Span<'static>> {
    match status {
        FileStatus::Added => vec![Span::styled("+ ", Style::default().fg(GREEN))],
        FileStatus::Deleted => vec![Span::styled("- ", Style::default().fg(RED))],
        FileStatus::Modified => vec![
            Span::styled("+", Style::default().fg(GREEN)),
            Span::styled("-", Style::default().fg(RED)),
        ],
        FileStatus::Renamed => vec![Span::styled("R ", Style::default().fg(BLUE))],
    }
}

fn ordered_position(
    first: (usize, usize),
    second: (usize, usize),
) -> ((usize, usize), (usize, usize)) {
    if first <= second {
        (first, second)
    } else {
        (second, first)
    }
}

fn apply_block_cursor<'a>(
    spans: &mut Vec<Span<'a>>,
    code_start: usize,
    column: usize,
    background: Color,
) {
    let mut offset = 0;
    for index in code_start..spans.len() {
        let length = spans[index].content.chars().count();
        if column < offset + length {
            let local = column - offset;
            let content = spans[index].content.to_string();
            let style = spans[index].style;
            let before = content.chars().take(local).collect::<String>();
            let cursor = content.chars().nth(local).unwrap().to_string();
            let after = content.chars().skip(local + 1).collect::<String>();
            spans.splice(
                index..=index,
                [
                    Span::styled(before, style),
                    Span::styled(
                        cursor,
                        Style::default()
                            .fg(background)
                            .bg(TEXT)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(after, style),
                ],
            );
            return;
        }
        offset += length;
    }
    if column == 0 && offset == 0 {
        spans.push(Span::styled(
            " ",
            Style::default()
                .fg(background)
                .bg(TEXT)
                .add_modifier(Modifier::BOLD),
        ));
    }
}

fn apply_character_selection<'a>(
    spans: &mut Vec<Span<'a>>,
    code_start: usize,
    start: usize,
    end: usize,
    cursor: Option<usize>,
) {
    let original = std::mem::take(spans);
    let mut rebuilt = Vec::new();
    let mut column = 0;
    for (index, span) in original.into_iter().enumerate() {
        if index < code_start {
            rebuilt.push(span);
            continue;
        }
        for character in span.content.chars() {
            let style = if cursor == Some(column) {
                Style::default()
                    .fg(BG)
                    .bg(TEXT)
                    .add_modifier(Modifier::BOLD)
            } else if (start..=end).contains(&column) {
                span.style.bg(SELECT_BG)
            } else {
                span.style
            };
            rebuilt.push(Span::styled(character.to_string(), style));
            column += 1;
        }
    }
    *spans = rebuilt;
}

fn help_line(section: &'static str, key: &'static str, description: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!(" {section:<12}"),
            Style::default().fg(BLUE).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("{key:<20}"), Style::default().fg(COMMENT)),
        Span::styled(description, Style::default().fg(TEXT)),
    ])
}

fn line_in_note(l: &DiffLine, n: &Annotation) -> bool {
    if l.kind == LineKind::Hunk {
        return n.excerpt.lines().any(|x| x.trim() == l.text);
    }
    (l.old
        .zip(n.old_start)
        .is_some_and(|(x, s)| s <= x && x <= n.old_end.unwrap_or(s)))
        || (l
            .new
            .zip(n.new_start)
            .is_some_and(|(x, s)| s <= x && x <= n.new_end.unwrap_or(s)))
}
fn anchor_position(f: &FileDiff, n: &Annotation) -> Option<usize> {
    f.lines
        .iter()
        .position(|l| {
            (n.anchor_old.is_none() || l.old == n.anchor_old)
                && (n.anchor_new.is_none() || l.new == n.anchor_new)
                && line_in_note(l, n)
        })
        .or_else(|| f.lines.iter().position(|l| line_in_note(l, n)))
}
fn fuzzy(q: &str, s: &str) -> bool {
    let mut chars = q.to_lowercase().chars().collect::<Vec<_>>().into_iter();
    let mut want = chars.next();
    for c in s.to_lowercase().chars() {
        if want == Some(c) {
            want = chars.next();
            if want.is_none() {
                return true;
            }
        }
    }
    want.is_none()
}

fn previous_boundary(text: &str, cursor: usize) -> Option<usize> {
    text[..cursor]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
}

fn next_boundary(text: &str, cursor: usize) -> Option<usize> {
    text[cursor..]
        .chars()
        .next()
        .map(|character| cursor + character.len_utf8())
}

fn vertical_cursor(text: &str, cursor: usize, down: bool) -> usize {
    let line_start = text[..cursor].rfind('\n').map_or(0, |index| index + 1);
    let column = text[line_start..cursor].chars().count();
    let (target_start, target_end) = if down {
        let Some(current_end_offset) = text[cursor..].find('\n') else {
            return cursor;
        };
        let target_start = cursor + current_end_offset + 1;
        let target_end = text[target_start..]
            .find('\n')
            .map_or(text.len(), |offset| target_start + offset);
        (target_start, target_end)
    } else {
        if line_start == 0 {
            return cursor;
        }
        let target_end = line_start - 1;
        let target_start = text[..target_end].rfind('\n').map_or(0, |index| index + 1);
        (target_start, target_end)
    };
    text[target_start..target_end]
        .char_indices()
        .nth(column)
        .map_or(target_end, |(offset, _)| target_start + offset)
}

fn crop_spans(spans: Vec<Span<'static>>, offset: usize, width: usize) -> Vec<Span<'static>> {
    let mut skipped = 0;
    let mut visible = 0;
    let mut out = Vec::new();
    for span in spans {
        let mut text = String::new();
        for character in span.content.chars() {
            let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
            if skipped + character_width <= offset {
                skipped += character_width;
                continue;
            }
            if visible + character_width > width {
                break;
            }
            text.push(character);
            visible += character_width;
        }
        if !text.is_empty() {
            out.push(Span::styled(text, span.style));
        }
        if visible >= width {
            break;
        }
    }
    out
}

fn inline_message_lines(message: &Message, width: usize) -> Vec<Line<'static>> {
    const CODE_COLUMN: usize = 13;
    let prefix_width = CODE_COLUMN.min(width);
    let card_width = width.saturating_sub(prefix_width);
    let (accent, background) = match message.role {
        MessageRole::Human => (COMMENT, COMMENT_BG),
        MessageRole::Assistant => (AI_COMMENT, AI_COMMENT_BG),
    };
    let label = message.author_name();
    let wrapped = wrap_comment(&message.text, card_width, label);
    wrapped
        .into_iter()
        .enumerate()
        .map(|(index, text_line)| {
            let lead = if index == 0 {
                format!("┃ {label} · ")
            } else {
                "┃   ".into()
            };
            let mut card = crop_spans(
                vec![
                    Span::styled(lead, Style::default().fg(accent).bg(background)),
                    Span::styled(text_line, Style::default().fg(TEXT).bg(background)),
                ],
                0,
                card_width,
            );
            let visible = card
                .iter()
                .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
                .sum::<usize>();
            if visible < card_width {
                card.push(Span::styled(
                    " ".repeat(card_width - visible),
                    Style::default().bg(background),
                ));
            }
            let mut spans = vec![Span::styled(
                " ".repeat(prefix_width),
                Style::default().bg(BG),
            )];
            spans.extend(card);
            Line::from(spans)
        })
        .collect()
}

fn wrap_comment(text: &str, card_width: usize, label: &str) -> Vec<String> {
    let mut output = Vec::new();
    for logical_line in text.split('\n') {
        if logical_line.is_empty() {
            output.push(String::new());
            continue;
        }
        let mut remaining = logical_line;
        while !remaining.is_empty() {
            let lead = if output.is_empty() {
                format!("┃ {label} · ")
            } else {
                "┃   ".into()
            };
            let available = card_width
                .saturating_sub(UnicodeWidthStr::width(lead.as_str()))
                .max(1);
            let (line, rest) = split_for_width(remaining, available);
            output.push(line.to_owned());
            remaining = rest;
        }
    }
    if output.is_empty() {
        output.push(String::new());
    }
    output
}

fn editor_visual_rows(text: &str, width: usize) -> Vec<(usize, usize)> {
    let mut rows = Vec::new();
    let mut line_start = 0;
    loop {
        let line_end = text[line_start..]
            .find('\n')
            .map_or(text.len(), |offset| line_start + offset);
        if line_start == line_end {
            rows.push((line_start, line_end));
        } else {
            let mut start = line_start;
            while start < line_end {
                let mut used = 0;
                let mut last_space_end = None;
                let mut hard_cut = line_end;
                let mut overflowed = false;
                for (offset, character) in text[start..line_end].char_indices() {
                    let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
                    if used + character_width > width {
                        hard_cut = start + offset;
                        overflowed = true;
                        break;
                    }
                    used += character_width;
                    if character.is_whitespace() {
                        last_space_end = Some(start + offset + character.len_utf8());
                    }
                }
                if !overflowed {
                    rows.push((start, line_end));
                    break;
                }
                let cut = last_space_end
                    .filter(|cut| *cut > start)
                    .unwrap_or_else(|| {
                        hard_cut.max(next_boundary(text, start).unwrap_or(line_end))
                    });
                rows.push((start, cut));
                start = cut;
            }
        }
        if line_end == text.len() {
            break;
        }
        line_start = line_end + 1;
    }
    rows
}

fn split_for_width(text: &str, width: usize) -> (&str, &str) {
    if UnicodeWidthStr::width(text) <= width {
        return (text, "");
    }
    let mut used = 0;
    let mut last_space = None;
    let mut hard_cut = text.len();
    for (index, character) in text.char_indices() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + character_width > width {
            hard_cut = index.max(character.len_utf8());
            break;
        }
        used += character_width;
        if character.is_whitespace() {
            last_space = Some(index);
        }
    }
    let cut = last_space.filter(|cut| *cut > 0).unwrap_or(hard_cut);
    (text[..cut].trim_end(), text[cut..].trim_start())
}

fn next_id<'a>(prefix: &str, existing: impl Iterator<Item = &'a str>) -> String {
    let existing = existing.collect::<HashSet<_>>();
    (1..)
        .map(|number| format!("{prefix}-{number:03}"))
        .find(|candidate| !existing.contains(candidate.as_str()))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::parse_unified_diff;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn renders_complete_layout() {
        let diff = "diff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-fn old() {}\n+fn new() {}\n";
        let mut app = App::new(parse_unified_diff(diff), Vec::new());
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        assert!(
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .any(|cell| cell.symbol() == "l")
        );
        // Changed-line background reaches the right edge of the diff viewport.
        assert_eq!(
            terminal.backend().buffer().cell((99, 3)).unwrap().bg,
            RED_BG
        );
        assert_eq!(
            terminal.backend().buffer().cell((99, 4)).unwrap().bg,
            GREEN_BG
        );
    }

    #[test]
    fn tab_moves_focus_accent_between_panels() {
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let mut app = App::new(parse_unified_diff(diff), Vec::new());
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        assert_eq!(terminal.backend().buffer().cell((50, 1)).unwrap().fg, BLUE);
        assert_eq!(
            terminal.backend().buffer().cell((35, 4)).unwrap().fg,
            BORDER
        );

        app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        terminal.draw(|frame| app.draw(frame)).unwrap();
        assert_eq!(
            terminal.backend().buffer().cell((50, 1)).unwrap().fg,
            BORDER
        );
        assert_eq!(terminal.backend().buffer().cell((35, 4)).unwrap().fg, BLUE);
    }

    #[test]
    fn minus_toggles_between_explorer_and_diff() {
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let mut app = App::new(parse_unified_diff(diff), Vec::new());
        app.key(KeyEvent::new(KeyCode::Char('-'), KeyModifiers::NONE));
        assert_eq!(app.focus, Focus::Files);
        assert_eq!(app.sidebar_selection, Some(SideTarget::File(0)));
        app.key(KeyEvent::new(KeyCode::Char('-'), KeyModifiers::NONE));
        assert_eq!(app.focus, Focus::Diff);
    }

    #[test]
    fn question_mark_opens_modal_help_until_closed() {
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let mut app = App::new(parse_unified_diff(diff), Vec::new());
        app.key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert!(app.help_open);
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Help"));

        app.key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert!(app.help_open);
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.help_open);
    }

    #[test]
    fn numbered_gg_jumps_to_exact_or_nearest_diff_line() {
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -120,3 +120,3 @@\n one\n two\n three\n";
        let mut app = App::new(parse_unified_diff(diff), Vec::new());
        for character in "121gg".chars() {
            app.key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        assert_eq!(app.current().lines[app.cursor].new, Some(121));
        assert!(app.vim_command.is_empty());

        for character in "999gg".chars() {
            app.key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        assert_eq!(app.current().lines[app.cursor].new, Some(122));
    }

    #[test]
    fn escape_cancels_search_and_restores_previous_state() {
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let mut app = App::new(parse_unified_diff(diff), Vec::new());
        app.filter = "previous".into();
        app.key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        assert_eq!(app.focus, Focus::Filter);
        assert_eq!(app.filter, "previous");
        app.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert_eq!(app.filter, "previousa");
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.focus, Focus::Diff);
        assert_eq!(app.filter, "previous");
    }

    #[test]
    fn enter_accepts_search_in_file_explorer_and_empty_search_clears_it() {
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let template = parse_unified_diff(diff).remove(0);
        let mut second = template.clone();
        second.path = "second.rs".into();
        let mut app = App::new(vec![template, second], Vec::new());

        app.key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        for character in "second".chars() {
            app.key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.focus, Focus::Files);
        assert_eq!(app.file, 1);
        assert_eq!(app.filter, "second");

        app.key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        assert_eq!(app.filter, "second");
        app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.focus, Focus::Files);
        assert!(app.filter.is_empty());
    }

    #[test]
    fn reverse_range_places_new_comment_at_visual_bottom() {
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let mut app = App::new(parse_unified_diff(diff), Vec::new());
        app.range_anchor = Some(2);
        app.cursor = 1;

        app.open_editor();
        assert_eq!(app.editor_anchor, Some(2));
        app.editor = "Looks good?".into();
        app.save_editor();

        assert_eq!(app.notes[0].anchor_new, Some(1));
        assert_eq!(app.notes[0].anchor_old, None);
    }

    #[test]
    fn enter_inside_existing_range_starts_a_new_comment() {
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,3 +1,3 @@\n one\n two\n three\n";
        let outer = Annotation {
            id: "t-001".into(),
            path: "a.rs".into(),
            excerpt: " one\n two\n three".into(),
            old_start: Some(1),
            old_end: Some(3),
            new_start: Some(1),
            new_end: Some(3),
            anchor_old: Some(3),
            anchor_new: Some(3),
            status: ThreadStatus::Open,
            messages: vec![Message {
                id: "m-001".into(),
                role: MessageRole::Human,
                author: None,
                text: "Outer".into(),
            }],
        };
        let mut app = App::new(parse_unified_diff(diff), vec![outer]);
        app.cursor = 2;

        app.open_editor();

        assert!(app.editing_key.is_none());
        assert!(app.editor.is_empty());
        assert_eq!(app.editor_anchor, Some(2));
    }

    #[test]
    fn u_restores_the_last_deleted_thread() {
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n line\n";
        let note = Annotation {
            id: "t-001".into(),
            path: "a.rs".into(),
            excerpt: " line".into(),
            old_start: Some(1),
            old_end: Some(1),
            new_start: Some(1),
            new_end: Some(1),
            anchor_old: Some(1),
            anchor_new: Some(1),
            status: ThreadStatus::Open,
            messages: vec![Message {
                id: "m-001".into(),
                role: MessageRole::Human,
                author: Some("Alice".into()),
                text: "Why?".into(),
            }],
        };
        let mut app = App::new(parse_unified_diff(diff), vec![note.clone()]);
        app.cursor = 1;

        app.key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert!(app.notes.is_empty());
        app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));

        assert_eq!(app.notes, vec![note]);
    }

    #[test]
    fn shift_v_selects_lines_and_y_yanks_code_without_diff_prefixes() {
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,2 +1,2 @@\n-old\n+new\n same\n";
        let mut app = App::new(parse_unified_diff(diff), Vec::new());
        app.cursor = 1;

        app.key(KeyEvent::new(KeyCode::Char('V'), KeyModifiers::SHIFT));
        app.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        let outcome = app.key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));

        assert!(matches!(outcome, Outcome::Yank(ref code) if code == "old\nnew"));
        assert!(app.range_anchor.is_none());
    }

    #[test]
    fn v_selects_characters_for_yank_without_creating_comment_range() {
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n+hello\n";
        let mut app = App::new(parse_unified_diff(diff), Vec::new());
        app.cursor = 1;

        app.key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        let outcome = app.key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));

        assert!(matches!(outcome, Outcome::Yank(ref code) if code == "hel"));
        assert!(app.range_anchor.is_none());
    }

    #[test]
    fn characterwise_visual_mode_renders_a_distinct_block_cursor() {
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n+hello\n";
        let mut app = App::new(parse_unified_diff(diff), Vec::new());
        app.cursor = 1;
        app.key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        let line = app.current().lines[1].clone();

        let rendered = app.diff_line(&line, 1, 40);
        let cursor = rendered
            .spans
            .iter()
            .find(|span| span.content == "l" && span.style.fg == Some(BG))
            .unwrap();

        assert_eq!(cursor.style.bg, Some(TEXT));
        assert!(cursor.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn characterwise_selection_preserves_syntax_foreground() {
        let syntax_color = Color::Rgb(214, 93, 14);
        let mut spans = vec![
            Span::raw("prefix"),
            Span::styled("token", Style::default().fg(syntax_color).bg(GREEN_BG)),
        ];

        apply_character_selection(&mut spans, 1, 0, 3, Some(3));

        assert_eq!(spans[1].content, "t");
        assert_eq!(spans[1].style.fg, Some(syntax_color));
        assert_eq!(spans[1].style.bg, Some(SELECT_BG));
        assert_eq!(spans[4].style.fg, Some(BG));
        assert_eq!(spans[4].style.bg, Some(TEXT));
    }

    #[test]
    fn normal_mode_renders_and_moves_the_character_cursor() {
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n+hello\n";
        let mut app = App::new(parse_unified_diff(diff), Vec::new());
        app.cursor = 1;
        app.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        let line = app.current().lines[1].clone();

        let rendered = app.diff_line(&line, 1, 40);
        let cursor = rendered
            .spans
            .iter()
            .find(|span| span.content == "l" && span.style.bg == Some(TEXT))
            .unwrap();

        assert_eq!(app.visual_col, 2);
        assert_eq!(cursor.style.fg, Some(GREEN_BG));
    }

    #[test]
    fn normal_cursor_is_visible_on_a_hunk_header() {
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n line\n";
        let mut app = App::new(parse_unified_diff(diff), Vec::new());
        app.cursor = 0;
        let line = app.current().lines[0].clone();

        let rendered = app.diff_line(&line, 0, 40);
        let cursor = rendered
            .spans
            .iter()
            .find(|span| span.content == "@" && span.style.bg == Some(TEXT))
            .unwrap();

        assert_eq!(cursor.style.fg, Some(HUNK_BG));
    }

    #[test]
    fn hunk_header_supports_characterwise_visual_selection_and_yank() {
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n line\n";
        let mut app = App::new(parse_unified_diff(diff), Vec::new());
        app.cursor = 0;

        app.key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        let line = app.current().lines[0].clone();
        let rendered = app.diff_line(&line, 0, 40);
        assert!(
            rendered
                .spans
                .iter()
                .any(|span| span.style.bg == Some(TEXT))
        );
        let outcome = app.key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));

        assert!(matches!(outcome, Outcome::Yank(ref text) if text == "@@ "));
    }

    #[test]
    fn c_selects_a_diff_range_for_commenting() {
        let diff =
            "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,2 +1,2 @@\n one\n two\n";
        let mut app = App::new(parse_unified_diff(diff), Vec::new());
        app.cursor = 1;

        app.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app.editor = "Review both".into();
        app.editor_cursor = app.editor.len();
        app.save_editor();

        assert_eq!(app.notes[0].new_start, Some(1));
        assert_eq!(app.notes[0].new_end, Some(2));
        assert_eq!(app.notes[0].excerpt, " one\n two");
    }

    #[test]
    fn sidebar_spans_can_scroll_horizontally() {
        let spans = vec![
            Span::styled("  ", Style::default().fg(MUTED)),
            Span::styled("длинный.rs", Style::default().fg(TEXT)),
        ];
        let cropped = crop_spans(spans, 4, 5);
        assert_eq!(
            cropped
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
            "инный"
        );
        assert_eq!(cropped[0].style.fg, Some(TEXT));
    }

    #[test]
    fn sidebar_file_statuses_are_compact_and_color_coded() {
        let text = |status| {
            file_status_spans(status)
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        };

        assert_eq!(text(FileStatus::Added), "+ ");
        assert_eq!(text(FileStatus::Deleted), "- ");
        assert_eq!(text(FileStatus::Modified), "+-");
        assert_eq!(text(FileStatus::Renamed), "R ");
        let modified = file_status_spans(FileStatus::Modified);
        assert_eq!(modified[0].style.fg, Some(GREEN));
        assert_eq!(modified[1].style.fg, Some(RED));
    }

    #[test]
    fn inline_comment_is_a_full_width_visual_card() {
        let message = Message {
            id: "m-001".into(),
            role: MessageRole::Human,
            author: Some("Alice".into()),
            text: "Please simplify\n```rust\nfix();\n```".into(),
        };
        let lines = inline_message_lines(&message, 60);
        assert_eq!(lines.len(), 4);
        assert!(lines.iter().all(|line| line.width() == 60));
        assert_eq!(lines[0].spans[0].style.bg, Some(BG));
        assert!(
            lines[0]
                .spans
                .iter()
                .skip(1)
                .all(|span| span.style.bg == Some(COMMENT_BG))
        );
        assert_eq!(lines[0].spans[1].style.fg, Some(COMMENT));

        let reply = Message {
            id: "m-002".into(),
            role: MessageRole::Assistant,
            author: Some("Nova".into()),
            text: "Applied".into(),
        };
        let reply_lines = inline_message_lines(&reply, 60);
        assert_eq!(reply_lines[0].spans[1].style.fg, Some(AI_COMMENT));
        assert!(
            reply_lines[0]
                .spans
                .iter()
                .skip(1)
                .all(|span| span.style.bg == Some(AI_COMMENT_BG))
        );
    }

    #[test]
    fn inline_comments_wrap_words_and_long_tokens_to_the_viewport() {
        let message = Message {
            id: "m-001".into(),
            role: MessageRole::Human,
            author: Some("Ali".into()),
            text: "This comment is deliberately long enough to wrap without disappearing.\n012345678901234567890123456789"
                .into(),
        };

        let lines = inline_message_lines(&message, 42);

        assert!(lines.len() >= 4);
        assert!(lines.iter().all(|line| line.width() == 42));
        let rendered = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(rendered.contains("disappearing."));
        assert!(rendered.contains("0123456789"));
    }

    #[test]
    fn editor_cursor_moves_to_the_new_line() {
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let mut app = App::new(parse_unified_diff(diff), Vec::new());
        app.open_editor();
        app.editor = "first\n".into();
        app.editor_cursor = app.editor.len();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut text_row = None;
        let mut cursor_row = None;
        for y in 0..30 {
            for x in 0..100 {
                let cell = buffer.cell((x, y)).unwrap();
                match cell.symbol() {
                    "f" if text_row.is_none() => text_row = Some(y),
                    " " if cell.bg == TEXT => cursor_row = Some(y),
                    _ => {}
                }
            }
        }
        assert!(cursor_row.unwrap() > text_row.unwrap());
    }

    #[test]
    fn editor_word_wraps_long_input_without_changing_its_text() {
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n line\n";
        let mut app = App::new(parse_unified_diff(diff), Vec::new());
        app.open_editor();
        app.editor = "This reply is intentionally long and should wrap inside the inline editor without inserting newline characters into the saved text."
            .into();
        app.editor_cursor = app.editor.len();
        let original = app.editor.clone();
        let mut lines = Vec::new();
        let mut map = Vec::new();

        app.append_editor(&mut lines, &mut map, "Reply", 52);

        assert!(lines.len() >= 5);
        assert_eq!(app.editor, original);
        assert_eq!(map.len(), lines.len());
        assert!(
            lines[1..lines.len() - 1]
                .iter()
                .all(|line| line.width() <= 52)
        );
    }

    #[test]
    fn editor_arrows_move_cursor_between_lines() {
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let mut app = App::new(parse_unified_diff(diff), Vec::new());
        app.open_editor();
        app.editor = "ab\ncde".into();
        app.editor_cursor = app.editor.len();

        app.editor_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.editor_cursor, 2);
        app.editor_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.editor_cursor, 1);
        app.editor_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.editor_cursor, 4);
        app.editor_key(KeyEvent::new(KeyCode::Char('λ'), KeyModifiers::NONE));
        assert_eq!(app.editor, "ab\ncλde");
    }

    #[test]
    fn terminal_shift_enter_inserts_newline_instead_of_j() {
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let mut app = App::new(parse_unified_diff(diff), Vec::new());
        app.open_editor();
        app.editor = "Ready".into();
        app.editor_cursor = app.editor.len();
        app.editor_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
        assert_eq!(app.editor, "Ready\n");
        assert_eq!(app.editor_cursor, 6);
        assert!(app.notes.is_empty());
        assert_eq!(app.focus, Focus::Editor);
    }

    #[test]
    fn sidebar_arrows_continue_after_selecting_a_comment() {
        let diff =
            "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,2 +1,2 @@\n one\n two\n";
        let note = |line, text: &str| Annotation {
            id: format!("t-{line:03}"),
            path: "a.rs".into(),
            excerpt: format!(" {text}"),
            old_start: Some(line),
            old_end: Some(line),
            new_start: Some(line),
            new_end: Some(line),
            anchor_old: Some(line),
            anchor_new: Some(line),
            status: ThreadStatus::Open,
            messages: vec![Message {
                id: format!("m-{line:03}"),
                role: MessageRole::Human,
                author: None,
                text: text.into(),
            }],
        };
        let mut app = App::new(
            parse_unified_diff(diff),
            vec![note(1, "one"), note(2, "two")],
        );
        app.select_side_target(SideTarget::Comment { file: 0, note: 0 }, true);

        app.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

        assert_eq!(
            app.sidebar_selection,
            Some(SideTarget::Comment { file: 0, note: 1 })
        );
        assert_eq!(app.focus, Focus::Files);
    }

    #[test]
    fn enter_on_ai_thread_opens_and_saves_a_human_reply() {
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n old\n";
        let thread = Annotation {
            id: "t-001".into(),
            path: "a.rs".into(),
            excerpt: " old".into(),
            old_start: Some(1),
            old_end: Some(1),
            new_start: Some(1),
            new_end: Some(1),
            anchor_old: Some(1),
            anchor_new: Some(1),
            status: ThreadStatus::Open,
            messages: vec![Message {
                id: "m-001".into(),
                role: MessageRole::Assistant,
                author: None,
                text: "Could this be simplified?".into(),
            }],
        };
        let mut app = App::new(parse_unified_diff(diff), vec![thread]);
        app.cursor = 1;

        app.open_editor();
        assert_eq!(app.replying_thread.as_deref(), Some("t-001"));
        assert!(app.editor.is_empty());
        app.editor = "Yes, I will update it.".into();
        app.editor_cursor = app.editor.len();
        app.save_editor();

        assert_eq!(app.notes[0].messages.len(), 2);
        assert_eq!(app.notes[0].messages[1].role, MessageRole::Human);
    }

    #[test]
    fn reply_editor_renders_after_the_visible_thread() {
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n line\n";
        let thread = Annotation {
            id: "t-001".into(),
            path: "a.rs".into(),
            excerpt: " line".into(),
            old_start: Some(1),
            old_end: Some(1),
            new_start: Some(1),
            new_end: Some(1),
            anchor_old: Some(1),
            anchor_new: Some(1),
            status: ThreadStatus::Open,
            messages: vec![Message {
                id: "m-001".into(),
                role: MessageRole::Assistant,
                author: Some("Nova".into()),
                text: "The existing reply stays visible.".into(),
            }],
        };
        let mut app = App::new(parse_unified_diff(diff), vec![thread]);
        app.cursor = 1;
        app.scroll = 1;
        app.open_reply();
        assert_eq!(app.scroll, 1);
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();

        terminal.draw(|frame| app.draw(frame)).unwrap();

        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        let comment = rendered.find("The existing reply stays visible.").unwrap();
        let editor = rendered.find("┌─ Reply").unwrap();
        assert!(comment < editor);
    }

    #[test]
    fn sidebar_scrolls_selected_file_into_view() {
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let template = parse_unified_diff(diff).remove(0);
        let files = (0..30)
            .map(|index| {
                let mut file = template.clone();
                file.path = format!("src/file_{index:02}.rs");
                file
            })
            .collect();
        let mut app = App::new(files, Vec::new());
        app.select_side_target(SideTarget::File(29), true);
        let mut terminal = Terminal::new(TestBackend::new(80, 14)).unwrap();

        terminal.draw(|frame| app.draw(frame)).unwrap();

        assert!(app.sidebar_scroll_y > 0);
        assert!(app.sidebar_row_map.contains(&Some(SideTarget::File(29))));
    }
}
