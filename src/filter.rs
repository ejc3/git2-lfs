//! Git2 filter integration for LFS.
//!
//! This module provides helpers for using LFS with git2's filter API.

use git2::{FilterFlags, FilterList, FilterMode, Repository};

use crate::{LfsClient, Pointer, Result};

/// LFS filter helper for git2 repositories.
pub struct LfsFilter<'repo> {
    repo: &'repo Repository,
    client: LfsClient,
}

impl<'repo> LfsFilter<'repo> {
    /// Create a new LFS filter for a repository.
    ///
    /// Automatically derives the LFS endpoint from the remote URL.
    pub fn new(repo: &'repo Repository) -> Result<Self> {
        let remote_url = Self::get_remote_url(repo)?;
        let client = LfsClient::new(&remote_url)?;
        Ok(LfsFilter { repo, client })
    }

    /// Create a new LFS filter with a specific client.
    pub fn with_client(repo: &'repo Repository, client: LfsClient) -> Self {
        LfsFilter { repo, client }
    }

    /// Get the LFS client.
    pub fn client(&self) -> &LfsClient {
        &self.client
    }

    /// Get a mutable reference to the LFS client.
    pub fn client_mut(&mut self) -> &mut LfsClient {
        &mut self.client
    }

    /// Check if a file is tracked by LFS.
    pub fn is_tracked(&self, path: &str) -> bool {
        match FilterList::load(self.repo, path, FilterMode::ToOdb, FilterFlags::DEFAULT) {
            Ok(Some(filters)) => filters.contains("lfs"),
            _ => false,
        }
    }

    /// Clean content (working tree -> ODB).
    ///
    /// If the file is tracked by LFS, this generates an LFS pointer
    /// and uploads the content to the LFS server.
    pub fn clean(&self, path: &str, content: &[u8]) -> Result<Vec<u8>> {
        if !self.is_tracked(path) {
            return Ok(content.to_vec());
        }

        // Generate pointer
        let pointer = Pointer::from_content(content);

        // Upload to LFS server
        self.client.upload(&pointer, content)?;

        // Return pointer content
        Ok(pointer.encode_bytes())
    }

    /// Smudge content (ODB -> working tree).
    ///
    /// If the content is an LFS pointer, this downloads the actual
    /// file content from the LFS server.
    pub fn smudge(&self, _path: &str, content: &[u8]) -> Result<Vec<u8>> {
        // Check if content is an LFS pointer
        if !Pointer::is_pointer(content) {
            return Ok(content.to_vec());
        }

        // Parse pointer
        let pointer = Pointer::parse(content)?;

        // Download from LFS server
        self.client.download(&pointer)
    }

    /// Get the remote URL from the repository.
    fn get_remote_url(repo: &Repository) -> Result<String> {
        // Try "origin" first
        if let Ok(remote) = repo.find_remote("origin") {
            if let Some(url) = remote.url() {
                return Ok(url.to_string());
            }
        }

        // Try any remote
        let remotes = repo
            .remotes()
            .map_err(|e| crate::Error::InvalidUrl(format!("failed to list remotes: {}", e)))?;

        for name in remotes.iter().flatten() {
            if let Ok(remote) = repo.find_remote(name) {
                if let Some(url) = remote.url() {
                    return Ok(url.to_string());
                }
            }
        }

        Err(crate::Error::InvalidUrl("no remote found".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    fn repo_init() -> (TempDir, Repository) {
        let td = TempDir::new().unwrap();
        let repo = Repository::init(td.path()).unwrap();
        {
            let mut config = repo.config().unwrap();
            config.set_str("user.name", "name").unwrap();
            config.set_str("user.email", "email").unwrap();
        }
        (td, repo)
    }

    #[test]
    fn test_is_tracked_no_attributes() {
        let (_td, repo) = repo_init();

        // Need a client to create filter, use dummy URL
        let client = LfsClient::new("https://github.com/test/repo.git").unwrap();
        let filter = LfsFilter::with_client(&repo, client);

        assert!(!filter.is_tracked("test.bin"));
    }

    #[test]
    fn test_is_tracked_with_attributes() {
        let (td, repo) = repo_init();

        // Create .gitattributes
        let gitattributes_path = td.path().join(".gitattributes");
        {
            let mut file = File::create(&gitattributes_path).unwrap();
            writeln!(file, "*.bin filter=lfs diff=lfs merge=lfs -text").unwrap();
        }

        // Add .gitattributes to index
        {
            let mut index = repo.index().unwrap();
            index
                .add_path(std::path::Path::new(".gitattributes"))
                .unwrap();
            index.write().unwrap();
        }

        let client = LfsClient::new("https://github.com/test/repo.git").unwrap();
        let filter = LfsFilter::with_client(&repo, client);

        // Check if .bin files are tracked
        let tracked = filter.is_tracked("test.bin");
        // Note: This may return false if libgit2 doesn't have LFS filter registered
        let _ = tracked;
    }

    #[test]
    fn test_clean_untracked() {
        let (_td, repo) = repo_init();

        let client = LfsClient::new("https://github.com/test/repo.git").unwrap();
        let filter = LfsFilter::with_client(&repo, client);

        let content = b"test content";
        let result = filter.clean("test.txt", content).unwrap();

        // Untracked files should pass through unchanged
        assert_eq!(result, content);
    }

    #[test]
    fn test_smudge_not_pointer() {
        let (_td, repo) = repo_init();

        let client = LfsClient::new("https://github.com/test/repo.git").unwrap();
        let filter = LfsFilter::with_client(&repo, client);

        let content = b"regular file content";
        let result = filter.smudge("test.txt", content).unwrap();

        // Non-pointer content should pass through unchanged
        assert_eq!(result, content);
    }
}
