//! Backing up and restoring local artifacts ([`crate::Artifact::is_local`]):
//! PDFs added with [`crate::fetch_file`], which exist only in this
//! machine's Nix store, since nothing can download them again.
//!
//! [`export_pdfs`] copies each one out of the store as
//! `<dir>/<citation_key>.pdf` and writes `<dir>/manifest.json`, mapping each
//! exported citation key to the hash its file must match.
//! [`sync_library_from`] is `sync` that first looks in such a directory:
//! a file whose hash is the one recorded in `papers.nix` is added back, one
//! that isn't is reported and skipped.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use crate::error::PaxError;
use crate::library::Library;
use crate::{FetchOutcome, SyncReport, fetch_paper, nix};

/// The manifest's file name in a backup directory.
pub const MANIFEST: &str = "manifest.json";

/// The file a backup directory holds for `citation_key`.
pub fn backup_file(dir: &Path, citation_key: &str) -> PathBuf {
    dir.join(format!("{citation_key}.pdf"))
}

/// One local artifact's copy, from [`export_pdfs`].
#[non_exhaustive]
pub struct PdfCopy {
    pub citation_key: String,
    /// The hash recorded in `papers.nix`, which the copy matches.
    pub hash: String,
    /// Where the copy was written, or why it wasn't:
    /// [`PaxError::LocalArtifactMissing`] when the artifact isn't in this
    /// machine's Nix store.
    pub result: Result<PathBuf, PaxError>,
}

/// What [`export_pdfs`] wrote.
#[non_exhaustive]
pub struct PdfExport {
    /// The backup directory.
    pub dir: PathBuf,
    /// `<dir>/manifest.json`.
    pub manifest: PathBuf,
    /// Every local artifact, in library order.
    pub papers: Vec<PdfCopy>,
}

/// Copies every local artifact in the library at `root` out of the Nix
/// store to `<dir>/<citation_key>.pdf`, creating `dir` if needed, and writes
/// `<dir>/manifest.json`: a JSON object from each exported citation key to
/// its hash. A file already there is replaced; other files are left alone.
///
/// One that isn't in this machine's store is reported in its [`PdfCopy`]
/// ([`PaxError::LocalArtifactMissing`]) and left out of the manifest, as is
/// one whose copy fails; neither stops the rest. Papers with a `source_url`
/// aren't exported: Nix can download them again.
pub fn export_pdfs(root: &Path, dir: &Path) -> Result<PdfExport, PaxError> {
    let library = Library::load(&nix::papers_path(root))?;
    std::fs::create_dir_all(dir).map_err(|source| PaxError::Backup {
        path: dir.to_path_buf(),
        source,
    })?;
    let mut manifest = BTreeMap::new();
    let mut papers = Vec::new();
    for paper in library.papers() {
        let (Some(hash), true) = (&paper.artifact.hash, paper.artifact.is_local()) else {
            continue;
        };
        let citation_key = &paper.local.citation_key;
        let result = nix::local_artifact_path(hash, citation_key).and_then(|store_path| {
            let store_path =
                store_path.ok_or_else(|| PaxError::LocalArtifactMissing(citation_key.clone()))?;
            let target = backup_file(dir, citation_key);
            copy(&store_path, &target)?;
            Ok(target)
        });
        if result.is_ok() {
            manifest.insert(citation_key.clone(), hash.clone());
        }
        papers.push(PdfCopy {
            citation_key: citation_key.clone(),
            hash: hash.clone(),
            result,
        });
    }
    let manifest_path = dir.join(MANIFEST);
    let json = serde_json::to_string_pretty(&manifest).expect("a map of strings serializes");
    std::fs::write(&manifest_path, format!("{json}\n")).map_err(|source| PaxError::Backup {
        path: manifest_path.clone(),
        source,
    })?;
    Ok(PdfExport {
        dir: dir.to_path_buf(),
        manifest: manifest_path,
        papers,
    })
}

/// Copies `from`'s bytes to `to`, which gets ordinary permissions rather
/// than the store's read-only ones, so a later export can replace it.
fn copy(from: &Path, to: &Path) -> Result<(), PaxError> {
    let backup = |path: &Path| {
        let path = path.to_path_buf();
        move |source| PaxError::Backup { path, source }
    };
    let mut source = File::open(from).map_err(backup(from))?;
    let mut target = File::create(to).map_err(backup(to))?;
    std::io::copy(&mut source, &mut target).map_err(backup(to))?;
    Ok(())
}

/// [`crate::sync_library`], restoring local artifacts from `dir`, a
/// directory [`export_pdfs`] wrote (or any holding `<citation_key>.pdf`
/// files): for each local artifact whose file is there, the file is added
/// back to the Nix store with a GC root, as [`crate::fetch_file`] would add
/// it, when its hash is the one recorded in `papers.nix`, and reported as
/// [`PaxError::BackupMismatch`] and skipped when it isn't; a recorded hash is
/// never replaced. Every other paper, and a local artifact with no file in
/// `dir`, goes through [`fetch_paper`] as in `sync`, so one still missing is
/// [`PaxError::LocalArtifactMissing`].
///
/// `dir` that isn't a directory is [`PaxError::NotADirectory`]. The
/// manifest isn't read: the hash each file must match is `papers.nix`'s.
pub fn sync_library_from(root: &Path, dir: &Path) -> Result<Vec<SyncReport>, PaxError> {
    let library = Library::load(&nix::papers_path(root))?;
    if !dir.is_dir() {
        return Err(PaxError::NotADirectory(dir.to_path_buf()));
    }
    Ok(library
        .papers()
        .iter()
        .map(|paper| {
            let citation_key = paper.local.citation_key.clone();
            let file = backup_file(dir, &citation_key);
            let result = match &paper.artifact.hash {
                Some(recorded) if paper.artifact.is_local() && file.is_file() => {
                    restore(root, &citation_key, recorded, &file)
                }
                _ => fetch_paper(&citation_key, root),
            };
            SyncReport {
                citation_key,
                result,
            }
        })
        .collect())
}

/// Adds `file` back as `citation_key`'s local artifact if its hash is
/// `recorded`.
fn restore(
    root: &Path,
    citation_key: &str,
    recorded: &str,
    file: &Path,
) -> Result<FetchOutcome, PaxError> {
    let actual = nix::hash_file(file)?;
    if actual != recorded {
        return Err(PaxError::BackupMismatch {
            citation_key: citation_key.to_string(),
            file: file.to_path_buf(),
            recorded: recorded.to_string(),
            actual,
        });
    }
    nix::add_local_artifact(root, file, citation_key)?;
    Ok(FetchOutcome::Fetched { hash: actual })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pax-backup-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("research")).unwrap();
        dir
    }

    /// A library whose only paper has a `source_url`: nothing to back up.
    fn downloaded_only(name: &str) -> PathBuf {
        let root = scratch(name);
        fs::write(
            nix::papers_path(&root),
            r#"{
  turing1936 = {
    doi = null;
    title = "On Computable Numbers";
    authors = [ "Alan Turing" ];
    year = 1936;
    source_url = "https://example.org/turing.pdf";
    hash = "sha256-abc123";
    tags = [ ];
    notes = null;
  };
}
"#,
        )
        .unwrap();
        root
    }

    #[test]
    fn export_skips_papers_nix_can_download_and_writes_an_empty_manifest() {
        let root = downloaded_only("export-none");
        let dir = root.join("backup").join("nested");
        let export = export_pdfs(&root, &dir).unwrap();
        assert!(export.papers.is_empty());
        assert_eq!(export.manifest, dir.join(MANIFEST));
        assert_eq!(fs::read_to_string(&export.manifest).unwrap(), "{}\n");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn sync_from_needs_a_directory() {
        let root = downloaded_only("sync-from-missing");
        let missing = root.join("no-such-dir");
        assert!(matches!(
            sync_library_from(&root, &missing),
            Err(PaxError::NotADirectory(path)) if path == missing
        ));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn sync_from_leaves_downloaded_papers_to_fetch() {
        let root = downloaded_only("sync-from-downloaded");
        let dir = root.join("backup");
        fs::create_dir_all(&dir).unwrap();
        // Even a file named after it isn't used: it isn't a local artifact.
        fs::write(backup_file(&dir, "turing1936"), "%PDF").unwrap();
        let reports = sync_library_from(&root, &dir).unwrap();
        assert_eq!(reports.len(), 1);
        assert!(matches!(
            &reports[0].result,
            Ok(FetchOutcome::AlreadyFetched { hash }) if hash == "sha256-abc123"
        ));
        fs::remove_dir_all(&root).unwrap();
    }
}
