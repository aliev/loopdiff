use ratatui::style::Color;

mod command;
mod comment_editor;
mod controller;
mod diff_pane;
mod diff_view;
mod editor;
mod file_tree;
mod help;
mod render;
mod search;
mod session;
mod statusline;
mod view_helpers;
pub use command::{Command, Effect};
use comment_editor::CommentEditor;
use diff_pane::DiffPane;
use file_tree::FileTree;
use help::Help;
use session::Session;
use statusline::Statusline;

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
const SELECT_BG: Color = Color::Rgb(32, 52, 75);
const TAB_WIDTH: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Focus {
    Files,
    Diff,
    Filter,
    Editor,
}
enum Outcome {
    Continue,
    Finish,
    Yank(String),
    LoadFileView(usize),
}

pub struct App {
    session: Session,
    diff_pane: DiffPane,
    focus: Focus,
    search_return_focus: Focus,
    comment_editor: CommentEditor,
    file_tree: FileTree,
    help: Help,
    statusline: Statusline,
}

#[cfg(test)]
mod tests;
