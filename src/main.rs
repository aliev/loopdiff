mod app;
mod comment;
mod input;
mod model;
mod terminal;

use anyhow::Result;
use app::App;
use input::{DiffSource, StdinDiffSource};
use terminal::TerminalRuntime;

fn main() {
    let code = match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("loopdiff: {error:#}");
            1
        }
    };
    std::process::exit(code);
}

fn run() -> Result<i32> {
    let raw = StdinDiffSource.read()?;
    view_diff(&raw)
}

fn view_diff(raw: &str) -> Result<i32> {
    let files = model::parse_unified_diff(raw);
    if files.is_empty() {
        eprintln!("loopdiff: nothing to view");
        return Ok(0);
    }

    let mut app = App::new(files, Vec::new());
    TerminalRuntime::run(&mut app)?;
    Ok(0)
}
