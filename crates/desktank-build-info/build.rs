//! Computes the short commit hash and hands it to the crate as `DESKTANK_COMMIT`.
//!
//! The rules match `scripts/build-web.ts`: the first 7 characters of
//! `GITHUB_SHA` when set, else `git rev-parse --short HEAD`; a `-dirty` suffix
//! when the working tree has changes; `unknown` when git can't say.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    println!("cargo:rerun-if-changed=build.rs");
    watch_git();
    println!("cargo:rustc-env=DESKTANK_COMMIT={}", label());
}

/// Runs `git` in this crate's directory; `None` if it fails or prints nothing.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn label() -> String {
    let short = match std::env::var("GITHUB_SHA") {
        Ok(sha) if !sha.trim().is_empty() => sha.trim().chars().take(7).collect(),
        _ => match git(&["rev-parse", "--short=7", "HEAD"]) {
            Some(short) => short,
            None => return "unknown".to_owned(),
        },
    };
    // `git status --porcelain` prints nothing for a clean tree, which `git`
    // reports as `None`; a failure to run it is treated as clean.
    if git(&["status", "--porcelain"]).is_some() {
        format!("{short}-dirty")
    } else {
        short
    }
}

/// Reruns the script when HEAD moves or the index changes. In a worktree
/// `.git` is a file, so the paths come from `git rev-parse`; refs and
/// `packed-refs` live in the common directory.
fn watch_git() {
    let dir = |flag| git(&["rev-parse", flag]).map(PathBuf::from);
    let (Some(git_dir), Some(common_dir)) = (dir("--absolute-git-dir"), dir("--git-common-dir"))
    else {
        return;
    };
    // `--git-common-dir` may be relative to this crate's directory.
    let common_dir = if common_dir.is_absolute() {
        common_dir
    } else {
        std::env::current_dir().map_or(common_dir.clone(), |cwd| cwd.join(&common_dir))
    };
    let watch = |path: &Path| println!("cargo:rerun-if-changed={}", path.display());
    watch(&git_dir.join("HEAD"));
    watch(&git_dir.join("index"));
    watch(&common_dir.join("packed-refs"));
    if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watch(&common_dir.join(reference));
    }
}
