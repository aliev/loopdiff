use crate::review::{DiffEndpoint, DiffIdentity, EndpointKind};
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::{path::Path, process::Command};

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
    command.arg("diff").arg("--no-ext-diff").arg("--no-color");
    let (from, to) = if staged {
        command.arg("--cached");
        (
            commit_endpoint("HEAD")?,
            special_endpoint(EndpointKind::Index, "index"),
        )
    } else if let Some(base) = base {
        let (base, target) = match split_revision_range(base)? {
            Some((from, to)) => (from, to),
            None => (base, "HEAD"),
        };
        command.arg(base).arg(target);
        (commit_endpoint(base)?, commit_endpoint(target)?)
    } else {
        (
            commit_endpoint("HEAD")?,
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

fn commit_endpoint(revision: &str) -> Result<DiffEndpoint> {
    let output = Command::new("git")
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
    format!("{:x}", Sha256::digest(raw.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

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
}
