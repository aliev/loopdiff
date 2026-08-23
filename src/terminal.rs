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
    io::{self, Write},
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
