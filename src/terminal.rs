use crate::app::{App, Command, Effect};
use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{
    env,
    io::{self, Write},
    process::Command as ProcessCommand,
    time::Duration,
};

pub struct TerminalRuntime;

impl TerminalRuntime {
    pub fn run(app: &mut App) -> Result<Effect> {
        enable_raw_mode().context("enable raw mode")?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        let result = (|| -> Result<Effect> {
            loop {
                terminal.draw(|frame| app.draw(frame))?;
                if event::poll(Duration::from_millis(100))? {
                    match event::read()? {
                        Event::Key(key) => match app.update(Command::Key(key)) {
                            Effect::None => {}
                            Effect::Copy(text) => {
                                let encoded = BASE64.encode(text);
                                write!(terminal.backend_mut(), "\x1b]52;c;{encoded}\x07")?;
                                terminal.backend_mut().flush()?;
                            }
                            Effect::RequestFileView(file) => {
                                app.update(Command::FileViewLoaded { file, lines: None });
                            }
                            Effect::OpenFile(path) => {
                                if let Err(error) = open_in_editor(&mut terminal, &path) {
                                    app.notice(format!("editor: {error:#}"));
                                }
                            }
                            Effect::Quit => break Ok(Effect::Quit),
                        },
                        Event::Mouse(mouse) => {
                            app.update(Command::Mouse(mouse));
                        }
                        Event::Resize(_, _) => {}
                        _ => {}
                    }
                }
            }
        })();
        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
        terminal.show_cursor()?;
        result
    }
}

fn open_in_editor(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, path: &str) -> Result<()> {
    disable_raw_mode()?;
    let editor_result = (|| -> Result<()> {
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
        terminal.show_cursor()?;
        let mut command = editor_command(path)?;
        let status = command.status().context("start $EDITOR")?;
        anyhow::ensure!(status.success(), "$EDITOR exited with {status}");
        Ok(())
    })();

    let restore_result = (|| -> Result<()> {
        enable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            EnterAlternateScreen,
            EnableMouseCapture
        )?;
        Ok(())
    })();
    restore_result?;
    editor_result
}

fn editor_command(path: &str) -> Result<ProcessCommand> {
    let editor = env::var("EDITOR").context("$EDITOR is not set")?;
    editor_command_from(&editor, path)
}

fn editor_command_from(editor: &str, path: &str) -> Result<ProcessCommand> {
    let mut parts = editor.split_whitespace();
    let program = parts.next().context("$EDITOR is empty")?;
    let mut command = ProcessCommand::new(program);
    command.args(parts).arg(path);
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_command_preserves_configured_arguments_and_file_path() {
        let command = editor_command_from("code --wait", "src/main file.rs").unwrap();
        assert_eq!(command.get_program(), "code");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["--wait", "src/main file.rs"]
        );
    }
}
