//! LFS-aware repository wrapper for git2.
//!
//! Provides automatic LFS filtering for git operations.

use git2::{Repository, Signature};
use std::fs;
use std::path::Path;

use crate::{LfsClient, LfsFilter, Pointer, Result};

/// LFS-aware repository wrapper.
///
/// Wraps a git2 Repository and automatically handles LFS operations:
/// - `add()` runs clean filter and uploads to LFS server
/// - `checkout_lfs()` downloads from LFS and runs smudge filter
///
/// # Example
///
/// ```no_run
/// use git2::Repository;
/// use git2_lfs::{LfsRepo, LfsClient};
///
/// let repo = Repository::open(".").unwrap();
/// let client = LfsClient::new("https://github.com/owner/repo.git")
///     .unwrap()
///     .with_token("your-token");
///
/// let lfs = LfsRepo::new(repo, client);
///
/// // Write content and add to index - LFS handled automatically
/// std::fs::write("large.bin", b"large content").unwrap();
/// lfs.add("large.bin").unwrap();
/// ```
pub struct LfsRepo {
    repo: Repository,
    filter: LfsFilter<'static>,
    // We need 'static because LfsFilter borrows Repository,
    // but we own both. Use unsafe to extend lifetime.
    _repo_box: Box<Repository>,
}

impl LfsRepo {
    /// Create a new LFS-aware repository wrapper.
    pub fn new(repo: Repository, client: LfsClient) -> Self {
        // Box the repo so it has a stable address
        let repo_box = Box::new(repo);

        // Create filter with 'static lifetime (safe because we own the repo)
        let filter = unsafe {
            let repo_ref: &'static Repository = &*(&*repo_box as *const Repository);
            LfsFilter::with_client(repo_ref, client)
        };

        // Re-open the repo from its path
        let repo_path = repo_box.path();
        let repo = Repository::open(repo_path).expect("failed to reopen repository");

        LfsRepo {
            repo,
            filter,
            _repo_box: repo_box,
        }
    }

    /// Open an existing repository with LFS support.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let repo = Repository::open(path.as_ref())
            .map_err(|e| crate::Error::InvalidUrl(e.to_string()))?;

        let client = LfsFilter::get_remote_url_static(&repo)
            .and_then(|url| LfsClient::new(&url).ok())
            .unwrap_or_else(|| LfsClient::new("https://example.com/repo.git").unwrap());

        Ok(Self::new(repo, client))
    }

    /// Get a reference to the underlying repository.
    pub fn repo(&self) -> &Repository {
        &self.repo
    }

    /// Get a mutable reference to the LFS client for auth setup.
    pub fn client_mut(&mut self) -> &mut LfsClient {
        self.filter.client_mut()
    }

    /// Set authentication token.
    pub fn with_token(mut self, token: &str) -> Self {
        *self.filter.client_mut() = self.filter.client().clone().with_token(token);
        self
    }

    /// Add a file to the index with automatic LFS handling.
    ///
    /// If the file is tracked by LFS (per .gitattributes):
    /// 1. Reads content from disk
    /// 2. Generates LFS pointer
    /// 3. Uploads content to LFS server
    /// 4. Adds pointer to index
    /// 5. Writes pointer to disk (so working dir matches index)
    ///
    /// If the file is NOT tracked by LFS, adds normally.
    pub fn add<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let path = path.as_ref();
        let path_str = path.to_string_lossy();

        let workdir = self.repo.workdir()
            .ok_or_else(|| crate::Error::InvalidUrl("bare repository".into()))?;
        let full_path = workdir.join(path);

        // Read content from disk
        let content = fs::read(&full_path)
            .map_err(|e| crate::Error::Io(e))?;

        // Apply clean filter (handles LFS upload if tracked)
        let cleaned = self.filter.clean(&path_str, &content)?;

        // If content was transformed (is a pointer), write it back
        if cleaned != content {
            fs::write(&full_path, &cleaned)
                .map_err(|e| crate::Error::Io(e))?;
        }

        // Add to index
        let mut index = self.repo.index()
            .map_err(|e| crate::Error::InvalidUrl(e.to_string()))?;
        index.add_path(path)
            .map_err(|e| crate::Error::InvalidUrl(e.to_string()))?;
        index.write()
            .map_err(|e| crate::Error::InvalidUrl(e.to_string()))?;

        Ok(())
    }

    /// Add multiple files to the index.
    pub fn add_all<P: AsRef<Path>>(&self, paths: &[P]) -> Result<()> {
        for path in paths {
            self.add(path)?;
        }
        Ok(())
    }

    /// Checkout and smudge LFS files.
    ///
    /// After a git checkout, call this to download LFS content.
    pub fn smudge_all(&self) -> Result<()> {
        let workdir = self.repo.workdir()
            .ok_or_else(|| crate::Error::InvalidUrl("bare repository".into()))?;

        // Find all files that are LFS pointers
        let index = self.repo.index()
            .map_err(|e| crate::Error::InvalidUrl(e.to_string()))?;

        for entry in index.iter() {
            let path_bytes = &entry.path;
            let path_str = String::from_utf8_lossy(path_bytes);
            let full_path = workdir.join(&*path_str);

            if full_path.exists() {
                let content = fs::read(&full_path)
                    .map_err(|e| crate::Error::Io(e))?;

                // Check if it's a pointer
                if Pointer::is_pointer(&content) {
                    // Smudge (download from LFS)
                    let smudged = self.filter.smudge(&path_str, &content)?;
                    fs::write(&full_path, &smudged)
                        .map_err(|e| crate::Error::Io(e))?;
                }
            }
        }

        Ok(())
    }

    /// Smudge a single file.
    pub fn smudge<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let path = path.as_ref();
        let path_str = path.to_string_lossy();

        let workdir = self.repo.workdir()
            .ok_or_else(|| crate::Error::InvalidUrl("bare repository".into()))?;
        let full_path = workdir.join(path);

        let content = fs::read(&full_path)
            .map_err(|e| crate::Error::Io(e))?;

        if Pointer::is_pointer(&content) {
            let smudged = self.filter.smudge(&path_str, &content)?;
            fs::write(&full_path, &smudged)
                .map_err(|e| crate::Error::Io(e))?;
        }

        Ok(())
    }

    /// Create a commit with the current index.
    pub fn commit(&self, message: &str) -> Result<git2::Oid> {
        let sig = self.repo.signature()
            .or_else(|_| Signature::now("git2-lfs", "git2-lfs@example.com"))
            .map_err(|e| crate::Error::InvalidUrl(e.to_string()))?;

        let mut index = self.repo.index()
            .map_err(|e| crate::Error::InvalidUrl(e.to_string()))?;
        let tree_id = index.write_tree()
            .map_err(|e| crate::Error::InvalidUrl(e.to_string()))?;
        let tree = self.repo.find_tree(tree_id)
            .map_err(|e| crate::Error::InvalidUrl(e.to_string()))?;

        let parent = self.repo.head()
            .ok()
            .and_then(|h| h.peel_to_commit().ok());

        let parents: Vec<&git2::Commit> = parent.iter().collect();

        let oid = self.repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            message,
            &tree,
            &parents,
        ).map_err(|e| crate::Error::InvalidUrl(e.to_string()))?;

        Ok(oid)
    }
}

impl LfsFilter<'_> {
    /// Get remote URL from a repository (static version for initialization).
    pub(crate) fn get_remote_url_static(repo: &Repository) -> Option<String> {
        repo.find_remote("origin")
            .ok()
            .and_then(|r| r.url().map(|s| s.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_lfs_repo_add_non_lfs() {
        let td = TempDir::new().unwrap();
        let repo = Repository::init(td.path()).unwrap();
        {
            let mut config = repo.config().unwrap();
            config.set_str("user.name", "test").unwrap();
            config.set_str("user.email", "test@test.com").unwrap();
        }

        let client = LfsClient::new("https://github.com/test/repo.git").unwrap();
        let lfs = LfsRepo::new(repo, client);

        // Write a non-LFS file
        fs::write(td.path().join("readme.txt"), "Hello").unwrap();

        // Add should work without LFS
        lfs.add("readme.txt").unwrap();

        // File should be unchanged (not a pointer)
        let content = fs::read_to_string(td.path().join("readme.txt")).unwrap();
        assert_eq!(content, "Hello");
    }
}
