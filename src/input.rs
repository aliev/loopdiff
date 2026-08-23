use anyhow::{Context, Result};
use std::io::{self, IsTerminal, Read};

pub trait DiffSource {
    fn read(&mut self) -> Result<String>;
}

pub struct StdinDiffSource;

impl DiffSource for StdinDiffSource {
    fn read(&mut self) -> Result<String> {
        let mut stdin = io::stdin();
        let is_terminal = stdin.is_terminal();
        read_diff(&mut stdin, is_terminal)
    }
}

fn read_diff(reader: &mut impl Read, is_terminal: bool) -> Result<String> {
    if is_terminal {
        anyhow::bail!("expected a unified diff on stdin (for example: git diff | loopdiff)");
    }
    let mut raw = String::new();
    reader
        .read_to_string(&mut raw)
        .context("could not read diff from stdin")?;
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn reader_preserves_piped_diff_and_rejects_terminal_input() {
        let mut pipe = Cursor::new(b"diff --git a/a b/a\n".to_vec());
        assert_eq!(read_diff(&mut pipe, false).unwrap(), "diff --git a/a b/a\n");

        let mut terminal = Cursor::new(Vec::new());
        assert!(read_diff(&mut terminal, true).is_err());
    }
}
