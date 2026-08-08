mod app;
mod git;
mod model;
mod review;

use anyhow::{Context, Result};
use app::{App, Outcome};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

struct OutputWatch {
    path: PathBuf,
    synced_contents: String,
    observed_contents: String,
    synced_threads: Vec<review::Annotation>,
    last_check: Instant,
}

#[derive(Parser, Debug)]
#[command(
    name = "loopdiff",
    about = "Fast GitHub-like terminal diff review for humans and AI"
)]
struct Args {
    /// Git revision/range, or the first file when TARGET is present
    base: Option<String>,
    /// Second file; two Git revisions must use FROM..TO syntax
    target: Option<String>,
    /// Review staged changes
    #[arg(long)]
    staged: bool,
    /// Read a unified diff from stdin
    #[arg(long)]
    stdin: bool,
    /// Session Markdown file (load and save)
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Validate a Loopdiff review and exit
    #[arg(long, value_name = "PATH")]
    validate_review: Option<PathBuf>,
}

fn main() {
    let code = match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("loopdiff: {e:#}");
            1
        }
    };
    std::process::exit(code);
}

fn run() -> Result<i32> {
    let args = Args::parse();
    if let Some(path) = args.validate_review {
        let review = review::load(&path)?;
        let messages = review
            .threads
            .iter()
            .map(|thread| thread.messages.len())
            .sum::<usize>();
        println!("loopdiff: valid review v{}", review.version);
        println!("diff: {}", review::diff_title(&review.diff));
        println!("threads: {}", review.threads.len());
        println!("messages: {messages}");
        return Ok(0);
    }
    let comparison = comparison_label(&args);
    let review_output = args
        .output
        .as_ref()
        .map_or_else(|| "stdout".into(), |path| path.display().to_string());
    let loaded = if args.stdin {
        if args.base.is_some() || args.target.is_some() || args.staged {
            anyhow::bail!("--stdin cannot be combined with revisions or --staged");
        }
        let mut s = String::new();
        io::stdin().read_to_string(&mut s)?;
        git::stdin_diff(s)
    } else {
        git::load_diff(args.base.as_deref(), args.target.as_deref(), args.staged)?
    };
    let files = model::parse_unified_diff(&loaded.raw);
    if files.is_empty() {
        eprintln!("loopdiff: nothing to review");
        return Ok(0);
    }
    let existing_output = args.output.as_ref().filter(|path| path.is_file());
    let original_markdown = existing_output
        .map(|path| {
            fs::read_to_string(path).with_context(|| format!("can't read {}", path.display()))
        })
        .transpose()?;
    let mut session = if let (Some(path), Some(markdown)) = (existing_output, &original_markdown) {
        let session = review::parse_review(markdown)
            .with_context(|| format!("can't load {}", path.display()))?;
        if session.diff.patch_sha256 != loaded.identity.patch_sha256 {
            anyhow::bail!(
                "{} belongs to a different diff (expected {}, found {})",
                path.display(),
                loaded.identity.patch_sha256,
                session.diff.patch_sha256
            );
        }
        session
    } else {
        review::empty(loaded.identity)
    };
    let original_threads = session.threads.clone();
    let mut app = App::new(files, session.threads);
    app.set_review_context(comparison, review_output);
    app.set_reviewer_name(git::user_name());
    let mut watch = original_markdown.as_ref().map(|contents| OutputWatch {
        path: args.output.clone().unwrap(),
        synced_contents: contents.clone(),
        observed_contents: contents.clone(),
        synced_threads: original_threads.clone(),
        last_check: Instant::now(),
    });
    let _outcome = run_tui(&mut app, watch.as_mut())?;
    session.threads = app.notes;
    let batch = review::format_review(&session)?;
    if let Some(path) = args.output {
        persist_session(
            &path,
            &session,
            watch
                .as_ref()
                .map(|watch| watch.synced_contents.as_str())
                .or(original_markdown.as_deref()),
            watch.as_ref().map_or_else(
                || session.threads != original_threads,
                |watch| session.threads != watch.synced_threads,
            ),
        )?;
    } else if !session.threads.is_empty() {
        print!("{batch}")
    }
    Ok(if session.threads.is_empty() { 0 } else { 10 })
}

fn comparison_label(args: &Args) -> String {
    if args.stdin {
        return "stdin diff".into();
    }
    if let Some(target) = &args.target {
        return format!("{} ↔ {target}", args.base.as_deref().unwrap_or("?"));
    }
    if args.staged {
        return "HEAD..index".into();
    }
    match args.base.as_deref() {
        Some(range) if range.contains("..") => range.into(),
        Some(revision) => format!("{revision}..worktree"),
        None => "HEAD..worktree".into(),
    }
}

fn persist_session(
    path: &Path,
    session: &review::Review,
    original_markdown: Option<&str>,
    changed_in_tui: bool,
) -> Result<()> {
    if original_markdown.is_some() && !changed_in_tui {
        return Ok(());
    }
    if let Some(original) = original_markdown {
        let current = fs::read_to_string(path)
            .with_context(|| format!("can't check {} before saving", path.display()))?;
        if current != original {
            anyhow::bail!(
                "{} changed outside loopdiff while the review was open; refusing to overwrite it",
                path.display()
            );
        }
    }
    review::save(path, session)
}

fn run_tui(app: &mut App, mut watch: Option<&mut OutputWatch>) -> Result<Outcome> {
    enable_raw_mode().context("enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    let result = (|| -> Result<Outcome> {
        loop {
            if let Some(watch) = watch.as_deref_mut() {
                poll_output(watch, app);
            }
            terminal.draw(|f| app.draw(f))?;
            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    Event::Key(k) => {
                        let o = app.key(k);
                        match o {
                            Outcome::Continue => {}
                            Outcome::Yank(text) => {
                                let encoded = BASE64.encode(text);
                                write!(terminal.backend_mut(), "\x1b]52;c;{encoded}\x07")?;
                                terminal.backend_mut().flush()?;
                            }
                            Outcome::Finish => break Ok(Outcome::Finish),
                        }
                    }
                    Event::Mouse(m) => app.mouse(m),
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

fn poll_output(watch: &mut OutputWatch, app: &mut App) {
    if watch.last_check.elapsed() < Duration::from_millis(750) {
        return;
    }
    watch.last_check = Instant::now();
    let Ok(contents) = fs::read_to_string(&watch.path) else {
        return;
    };
    if contents == watch.observed_contents {
        return;
    }
    watch.observed_contents.clone_from(&contents);
    let Ok(external) = review::parse_review(&contents) else {
        app.notice("external review is not valid yet");
        return;
    };
    if app.notes != watch.synced_threads {
        app.notice("external changes waiting · finish or restart");
        return;
    }
    if external.diff.patch_sha256
        != review::parse_review(&watch.synced_contents)
            .map(|review| review.diff.patch_sha256)
            .unwrap_or_default()
    {
        app.notice("external review belongs to another diff");
        return;
    }
    app.notes = external.threads;
    watch.synced_threads.clone_from(&app.notes);
    watch.synced_contents = contents;
    app.notice("external replies loaded");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::{
        Annotation, DiffEndpoint, DiffIdentity, EndpointKind, Message, MessageRole, ThreadStatus,
    };
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "loopdiff-{label}-{}-{nonce}.md",
            std::process::id()
        ))
    }

    fn session() -> review::Review {
        review::empty(DiffIdentity {
            from: DiffEndpoint {
                kind: EndpointKind::Commit,
                label: "HEAD".into(),
                oid: Some("a".repeat(40)),
            },
            to: DiffEndpoint {
                kind: EndpointKind::Worktree,
                label: "worktree".into(),
                oid: None,
            },
            patch_sha256: "b".repeat(64),
        })
    }

    fn thread(text: &str) -> Annotation {
        Annotation {
            id: "t-001".into(),
            path: "a.rs".into(),
            excerpt: "+answer".into(),
            old_start: None,
            old_end: None,
            new_start: Some(1),
            new_end: Some(1),
            anchor_old: None,
            anchor_new: Some(1),
            status: ThreadStatus::Open,
            messages: vec![Message {
                id: "m-001".into(),
                role: MessageRole::Assistant,
                author: Some("Nova".into()),
                text: text.into(),
            }],
        }
    }

    #[test]
    fn unchanged_session_does_not_rewrite_output() {
        let path = test_path("unchanged");
        let original = format!("{}\n", review::format_review(&session()).unwrap());
        fs::write(&path, &original).unwrap();

        persist_session(&path, &session(), Some(&original), false).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn external_edit_is_not_overwritten_by_local_edit() {
        let path = test_path("conflict");
        let original = review::format_review(&session()).unwrap();
        fs::write(&path, &original).unwrap();
        fs::write(&path, "agent response").unwrap();

        let error = persist_session(&path, &session(), Some(&original), true).unwrap_err();

        assert!(error.to_string().contains("refusing to overwrite"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "agent response");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn watcher_loads_valid_external_replies() {
        let path = test_path("watch");
        let original_session = session();
        let original = review::format_review(&original_session).unwrap();
        fs::write(&path, &original).unwrap();
        let files = model::parse_unified_diff(
            "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -0,0 +1 @@\n+answer\n",
        );
        let mut app = App::new(files, Vec::new());
        let mut external_session = session();
        external_session.threads.push(thread("Done"));
        fs::write(&path, review::format_review(&external_session).unwrap()).unwrap();
        let mut watch = OutputWatch {
            path: path.clone(),
            synced_contents: original.clone(),
            observed_contents: original,
            synced_threads: Vec::new(),
            last_check: Instant::now() - Duration::from_secs(1),
        };

        poll_output(&mut watch, &mut app);

        assert_eq!(app.notes, external_session.threads);
        assert_eq!(watch.synced_threads, app.notes);
        fs::remove_file(path).unwrap();
    }
}
