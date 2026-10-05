//! Publishes a local file as a GitHub Release asset on a `research/`
//! library's own git remote, giving a PDF you only have a personal copy of
//! a stable, network-fetchable URL instead of a machine-local `file://`
//! path. Shells out to the `gh` CLI (must already be authenticated) — the
//! same "PAX understands papers, an existing tool understands the rest"
//! split as [`crate::nix`]'s relationship with the `nix` binary.
//!
//! This is deliberately the only piece that's new: once a paper's
//! `source_url` is a release-asset URL, *fetching* it back is just a plain
//! `https://` request — [`crate::nix::prefetch_file`] already handles that
//! with no changes. Only getting a local file *out* to a URL in the first
//! place needed new code.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::error::PaxError;
use crate::process::run_with_timeout;

/// How long any single `gh` invocation (repo lookup, release creation,
/// asset upload) is allowed to run before being killed — `gh` talks to the
/// GitHub API over the network with no timeout of its own, the same risk
/// `nix`'s subprocess calls have.
const GH_TIMEOUT: Duration = Duration::from_secs(120);

/// The one release every uploaded PDF is attached to as an asset, so
/// publishing a second paper (or re-publishing the same one) just adds or
/// replaces an asset on this release instead of accumulating a release per
/// paper.
const RELEASE_TAG: &str = "papers";

/// Uploads `local_path` as a GitHub Release asset named `<citation_key>.pdf`
/// on `root`'s own `origin` remote, creating the shared `papers` release
/// first if it doesn't exist yet. Returns the asset's stable download URL.
pub fn publish_to_github_release(
    root: &Path,
    citation_key: &str,
    local_path: &Path,
) -> Result<String, PaxError> {
    if !local_path.is_file() {
        return Err(PaxError::Upload(format!(
            "{}: not a file",
            local_path.display()
        )));
    }

    let repo = gh_repo(root)?;
    ensure_release(&repo)?;
    upload_asset(&repo, citation_key, local_path)?;

    Ok(format!(
        "https://github.com/{repo}/releases/download/{RELEASE_TAG}/{citation_key}.pdf"
    ))
}

/// Resolves the `owner/repo` GitHub slug for `root`'s git remote via `gh`
/// itself, rather than hand-parsing `git remote -v` (which has to handle
/// both the `git@github.com:...` and `https://github.com/...` remote
/// forms) — `gh` already does that resolution correctly.
fn gh_repo(root: &Path) -> Result<String, PaxError> {
    let output = run_with_timeout(
        Command::new("gh")
            .args([
                "repo",
                "view",
                "--json",
                "nameWithOwner",
                "-q",
                ".nameWithOwner",
            ])
            .current_dir(root),
        GH_TIMEOUT,
        PaxError::Upload,
    )?;
    if !output.status.success() {
        return Err(PaxError::Upload(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Creates the shared `papers` release if it doesn't already exist yet — a
/// missing release (detected via `gh release view`'s exit status) is the
/// expected, common case on a library's first upload, not an error.
fn ensure_release(repo: &str) -> Result<(), PaxError> {
    let exists = run_with_timeout(
        Command::new("gh").args(["release", "view", RELEASE_TAG, "--repo", repo]),
        GH_TIMEOUT,
        PaxError::Upload,
    )?
    .status
    .success();
    if exists {
        return Ok(());
    }

    let output = run_with_timeout(
        Command::new("gh").args([
            "release",
            "create",
            RELEASE_TAG,
            "--repo",
            repo,
            "--title",
            "PAX paper artifacts",
            "--notes",
            "Binary storage for lazypax-managed PDF sources.",
        ]),
        GH_TIMEOUT,
        PaxError::Upload,
    )?;
    if !output.status.success() {
        return Err(PaxError::Upload(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(())
}

/// Uploads `local_path` as an asset literally named `<citation_key>.pdf` —
/// staged into a temp file under that name first rather than relying on
/// `gh release upload`'s `file#label` syntax, which only sets a display
/// label in GitHub's UI, not the asset's actual stored filename (and the
/// download URL `publish_to_github_release` promises is built from the real
/// filename). `--clobber` makes re-uploading for the same citation key
/// (e.g. swapping in a better copy later) replace the existing asset
/// instead of failing on a name collision.
fn upload_asset(repo: &str, citation_key: &str, local_path: &Path) -> Result<(), PaxError> {
    let staging_dir = std::env::temp_dir().join(format!("pax-upload-{}", std::process::id()));
    std::fs::create_dir_all(&staging_dir).map_err(|e| PaxError::Upload(e.to_string()))?;
    let staged_path = staging_dir.join(format!("{citation_key}.pdf"));
    let staged =
        std::fs::copy(local_path, &staged_path).map_err(|e| PaxError::Upload(e.to_string()));

    let result = staged.and_then(|_| {
        run_with_timeout(
            Command::new("gh")
                .arg("release")
                .arg("upload")
                .arg(RELEASE_TAG)
                .arg(&staged_path)
                .arg("--clobber")
                .arg("--repo")
                .arg(repo),
            GH_TIMEOUT,
            PaxError::Upload,
        )
    });

    let _ = std::fs::remove_dir_all(&staging_dir);

    let output = result?;
    if !output.status.success() {
        return Err(PaxError::Upload(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_rejects_a_path_that_is_not_a_file() {
        let root = std::env::temp_dir();
        let missing = root.join("pax-github-test-does-not-exist.pdf");
        let result = publish_to_github_release(&root, "somekey2020", &missing);
        assert!(matches!(result, Err(PaxError::Upload(msg)) if msg.contains("not a file")));
    }
}
