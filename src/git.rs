use crate::review::{DiffEndpoint, DiffIdentity, EndpointKind};
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::{fmt::Write as _, path::Path, process::Command};

pub struct LoadedDiff {
    pub raw: String,
    pub identity: DiffIdentity,
}

pub fn user_name() -> Option<String> {
    let output = Command::new("git")
        .args(["config", "--get", "user.name"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&output.stdout)
        .trim()
        .replace(['\n', '\r', '*'], "");
    (!name.is_empty()).then_some(name)
}

pub fn load_diff(base: Option<&str>, target: Option<&str>, staged: bool) -> Result<LoadedDiff> {
    load_diff_in(Path::new("."), base, target, staged)
}

fn load_diff_in(
    repository: &Path,
    base: Option<&str>,
    target: Option<&str>,
    staged: bool,
) -> Result<LoadedDiff> {
    if staged && (base.is_some() || target.is_some()) {
        bail!("--staged cannot be combined with revision arguments");
    }
    if target.is_some() && base.is_none() {
        bail!("a second file requires a first file");
    }
    if let (Some(first), Some(second)) = (base, target) {
        return load_file_diff(first, second);
    }
    let mut command = Command::new("git");
    command
        .current_dir(repository)
        .arg("diff")
        .arg("--no-ext-diff")
        .arg("--no-color");
    let (from, to) = if staged {
        command.arg("--cached");
        (
            commit_endpoint_in(repository, "HEAD")?,
            special_endpoint(EndpointKind::Index, "index"),
        )
    } else if let Some(base) = base {
        if let Some((from, to)) = split_revision_range(base)? {
            command.arg(from).arg(to);
            (
                commit_endpoint_in(repository, from)?,
                commit_endpoint_in(repository, to)?,
            )
        } else {
            command.arg(base);
            (
                commit_endpoint_in(repository, base)?,
                special_endpoint(EndpointKind::Worktree, "working tree"),
            )
        }
    } else {
        (
            commit_endpoint_in(repository, "HEAD")?,
            special_endpoint(EndpointKind::Worktree, "working tree"),
        )
    };
    let output = command.output().context("could not run git diff")?;
    if !output.status.success() {
        bail!(
            "git diff failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let raw = String::from_utf8_lossy(&output.stdout).into_owned();
    Ok(LoadedDiff {
        identity: DiffIdentity {
            from,
            to,
            patch_sha256: patch_sha256(&raw),
        },
        raw,
    })
}

fn load_file_diff(first: &str, second: &str) -> Result<LoadedDiff> {
    for path in [first, second] {
        if !Path::new(path).is_file() {
            bail!("file {path:?} does not exist or is not a regular file");
        }
    }
    let output = Command::new("git")
        .arg("diff")
        .arg("--no-ext-diff")
        .arg("--no-color")
        .arg("--no-index")
        .arg("--")
        .arg(first)
        .arg(second)
        .output()
        .context("could not run git diff --no-index")?;
    if !matches!(output.status.code(), Some(0 | 1)) {
        bail!(
            "file diff failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let raw = String::from_utf8_lossy(&output.stdout).into_owned();
    Ok(LoadedDiff {
        identity: DiffIdentity {
            from: special_endpoint(EndpointKind::File, first),
            to: special_endpoint(EndpointKind::File, second),
            patch_sha256: patch_sha256(&raw),
        },
        raw,
    })
}

fn split_revision_range(revision: &str) -> Result<Option<(&str, &str)>> {
    if revision.contains("...") {
        bail!("three-dot ranges are not supported yet; pass two revisions or use FROM..TO");
    }
    let Some((from, to)) = revision.split_once("..") else {
        return Ok(None);
    };
    if from.is_empty() || to.is_empty() || to.contains("..") {
        bail!("invalid revision range {revision:?}; expected FROM..TO");
    }
    Ok(Some((from, to)))
}

pub fn stdin_diff(raw: String) -> LoadedDiff {
    LoadedDiff {
        identity: DiffIdentity {
            from: special_endpoint(EndpointKind::Stdin, "stdin"),
            to: special_endpoint(EndpointKind::Stdin, "stdin"),
            patch_sha256: patch_sha256(&raw),
        },
        raw,
    }
}

fn commit_endpoint_in(repository: &Path, revision: &str) -> Result<DiffEndpoint> {
    let output = Command::new("git")
        .current_dir(repository)
        .arg("rev-parse")
        .arg("--verify")
        .arg(format!("{revision}^{{commit}}"))
        .output()
        .context("could not resolve Git revision")?;
    if !output.status.success() {
        bail!(
            "could not resolve revision {revision:?}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(DiffEndpoint {
        kind: EndpointKind::Commit,
        label: revision.into(),
        oid: Some(String::from_utf8_lossy(&output.stdout).trim().into()),
    })
}

fn special_endpoint(kind: EndpointKind, label: &str) -> DiffEndpoint {
    DiffEndpoint {
        kind,
        label: label.into(),
        oid: None,
    }
}

fn patch_sha256(raw: &str) -> String {
    let digest = Sha256::digest(raw.as_bytes());
    let mut hexadecimal = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(hexadecimal, "{byte:02x}").expect("writing to a String cannot fail");
    }
    hexadecimal
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };

    struct TempRepo(std::path::PathBuf);

    impl TempRepo {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("loopdiff-git-{unique}"));
            fs::create_dir(&path).unwrap();
            git(&path, &["init", "--quiet"]);
            git(&path, &["config", "user.name", "Loopdiff Test"]);
            git(&path, &["config", "user.email", "loopdiff@example.test"]);
            Self(path)
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn git(repository: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(repository)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn native_diff(repository: &Path, args: &[&str]) -> String {
        let mut full_args = vec!["diff", "--no-ext-diff", "--no-color"];
        full_args.extend_from_slice(args);
        git(repository, &full_args)
    }

    #[test]
    fn stdin_identity_is_stable() {
        let loaded = stdin_diff("diff".into());
        assert_eq!(loaded.identity.patch_sha256.len(), 64);
        assert_eq!(loaded.identity.from.kind, EndpointKind::Stdin);
    }

    #[test]
    fn splits_standard_two_dot_revision_range() {
        assert_eq!(
            split_revision_range("HEAD..e7f539f").unwrap(),
            Some(("HEAD", "e7f539f"))
        );
        assert_eq!(split_revision_range("HEAD^").unwrap(), None);
        assert!(split_revision_range("HEAD...main").is_err());
    }

    #[test]
    fn compares_two_files_without_git_history() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let first = std::env::temp_dir().join(format!("loopdiff-{unique}-old.txt"));
        let second = std::env::temp_dir().join(format!("loopdiff-{unique}-new.txt"));
        fs::write(&first, "old\n").unwrap();
        fs::write(&second, "new\n").unwrap();

        let loaded = load_diff(first.to_str(), second.to_str(), false).unwrap();

        assert_eq!(loaded.identity.from.kind, EndpointKind::File);
        assert_eq!(loaded.identity.to.kind, EndpointKind::File);
        assert!(loaded.raw.contains("-old"));
        assert!(loaded.raw.contains("+new"));
        fs::remove_file(first).unwrap();
        fs::remove_file(second).unwrap();
    }

    #[test]
    fn git_modes_match_native_git_diff_semantics() {
        let repository = TempRepo::new();
        let file = repository.0.join("example.txt");
        fs::write(&file, "base\n").unwrap();
        git(&repository.0, &["add", "example.txt"]);
        git(&repository.0, &["commit", "--quiet", "-m", "base"]);
        fs::write(&file, "committed\n").unwrap();
        git(&repository.0, &["add", "example.txt"]);
        git(&repository.0, &["commit", "--quiet", "-m", "second"]);
        fs::write(&file, "working tree\n").unwrap();

        let working = load_diff_in(&repository.0, None, None, false).unwrap();
        assert_eq!(working.raw, native_diff(&repository.0, &[]));
        assert_eq!(working.identity.to.kind, EndpointKind::Worktree);

        let revision = load_diff_in(&repository.0, Some("HEAD^"), None, false).unwrap();
        assert_eq!(revision.raw, native_diff(&repository.0, &["HEAD^"]));
        assert_eq!(revision.identity.from.kind, EndpointKind::Commit);
        assert_eq!(revision.identity.to.kind, EndpointKind::Worktree);

        let range = load_diff_in(&repository.0, Some("HEAD^..HEAD"), None, false).unwrap();
        assert_eq!(range.raw, native_diff(&repository.0, &["HEAD^", "HEAD"]));
        assert_eq!(range.identity.from.kind, EndpointKind::Commit);
        assert_eq!(range.identity.to.kind, EndpointKind::Commit);

        git(&repository.0, &["add", "example.txt"]);
        let staged = load_diff_in(&repository.0, None, None, true).unwrap();
        assert_eq!(staged.raw, native_diff(&repository.0, &["--cached"]));
        assert_eq!(staged.identity.to.kind, EndpointKind::Index);
    }
}
