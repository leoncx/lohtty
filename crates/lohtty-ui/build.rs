//! Build information for the About section of the settings: the commit, the
//! version's release date (from CHANGELOG.md) and, for a build that isn't the
//! tagged release, how far past it the commit is.

use std::path::Path;
use std::process::Command;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let version = env!("CARGO_PKG_VERSION");
    let tag = format!("v{version}");

    rerun_on_git_changes(&root);
    println!(
        "cargo:rerun-if-changed={}",
        root.join("CHANGELOG.md").display()
    );
    println!("cargo:rerun-if-env-changed=GITHUB_REF_NAME");

    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(&root)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .filter(|s| !s.is_empty())
    };

    let commit = git(&["rev-parse", "--short=10", "HEAD"]).unwrap_or_default();

    // The release job builds the tag from a shallow checkout without tags,
    // so it is trusted to name what it builds.
    let release = std::env::var("GITHUB_REF_NAME").is_ok_and(|r| r == tag)
        || git(&["describe", "--tags", "--exact-match", "HEAD"]).is_some_and(|t| t == tag);
    let dev = if release {
        String::new()
    } else {
        match git(&["rev-list", "--count", &format!("{tag}..HEAD")]) {
            Some(n) if n == "1" => "+1 commit".to_owned(),
            Some(n) => format!("+{n} commits"),
            // No git, or the tag isn't here: nothing to say for sure.
            None if commit.is_empty() => String::new(),
            None => "dev".to_owned(),
        }
    };

    let changelog = std::fs::read_to_string(root.join("CHANGELOG.md")).unwrap_or_default();
    let date = release_date(&changelog, version).unwrap_or_default();

    println!("cargo:rustc-env=LOHTTY_COMMIT={commit}");
    println!("cargo:rustc-env=LOHTTY_RELEASE_DATE={date}");
    println!("cargo:rustc-env=LOHTTY_DEV_BUILD={dev}");
}

/// The date of `## [version] - date` in the changelog.
fn release_date(changelog: &str, version: &str) -> Option<String> {
    let heading = format!("## [{version}] - ");
    changelog
        .lines()
        .find_map(|l| l.strip_prefix(&heading))
        .map(|d| d.trim().to_owned())
}

/// Reruns when HEAD moves: a commit, a checkout, or a new tag.
fn rerun_on_git_changes(root: &Path) {
    let git_dir = root.join(".git");
    if !git_dir.is_dir() {
        return;
    }
    let mut paths = vec!["HEAD".to_owned(), "packed-refs".into(), "refs/tags".into()];
    if let Ok(head) = std::fs::read_to_string(git_dir.join("HEAD"))
        && let Some(branch) = head.trim().strip_prefix("ref: ")
    {
        paths.push(branch.to_owned());
    }
    // A missing path would rerun this on every build.
    for path in paths.iter().map(|p| git_dir.join(p)).filter(|p| p.exists()) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}
