use crate::model::DiffLine;
use crossterm::event::{KeyEvent, MouseEvent};

pub enum Command {
    Key(KeyEvent),
    Mouse(MouseEvent),
    FileViewLoaded {
        file: usize,
        lines: Option<Vec<DiffLine>>,
    },
}

#[derive(Debug, Eq, PartialEq)]
pub enum Effect {
    None,
    Quit,
    Copy(String),
    RequestFileView(usize),
}
