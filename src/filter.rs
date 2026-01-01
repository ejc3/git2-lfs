//! Git2 filter integration for LFS.
//!
//! This module provides two ways to use LFS with git2:
//!
//! 1. **Manual filtering** via [`LfsFilter`] - call `clean()` and `smudge()` explicitly
//! 2. **Automatic filtering** via [`register_lfs_filter`] - register once, all git2 operations use LFS
//!
//! ## Automatic Filtering (Recommended)
//!
//! ```ignore
//! use git2::Repository;
//! use git2_lfs::{register_lfs_filter, LfsClient};
//!
//! // Register LFS filter globally (once at startup)
//! let client = LfsClient::from_repo(&repo)?;
//! let _registration = register_lfs_filter(client)?;
//!
//! // Now all git2 operations automatically use LFS!
//! repo.checkout_head()?;  // LFS files are smudged
//! index.add_path("large.bin")?;  // LFS files are cleaned
//! ```

use git2::Repository;
use std::fs;
use std::path::Path;

use crate::{LfsClient, ObjectCache, Pointer, Result};

/// LFS filter helper for git2 repositories.
pub struct LfsFilter<'repo> {
    repo: &'repo Repository,
    client: LfsClient,
    cache: Option<ObjectCache>,
}

impl<'repo> LfsFilter<'repo> {
    /// Create a new LFS filter for a repository.
    ///
    /// Automatically derives the LFS endpoint from the remote URL and
    /// initializes the object cache at `.git/lfs/objects`.
    pub fn new(repo: &'repo Repository) -> Result<Self> {
        let remote_url = Self::get_remote_url(repo)?;
        let client = LfsClient::new(&remote_url)?;
        let cache = Some(ObjectCache::for_repo(repo.path()));
        Ok(LfsFilter { repo, client, cache })
    }

    /// Create a new LFS filter with a specific client.
    ///
    /// Initializes the object cache at `.git/lfs/objects`.
    pub fn with_client(repo: &'repo Repository, client: LfsClient) -> Self {
        let cache = Some(ObjectCache::for_repo(repo.path()));
        LfsFilter { repo, client, cache }
    }

    /// Create a new LFS filter without a cache.
    pub fn without_cache(repo: &'repo Repository, client: LfsClient) -> Self {
        LfsFilter { repo, client, cache: None }
    }

    /// Get the object cache if available.
    pub fn cache(&self) -> Option<&ObjectCache> {
        self.cache.as_ref()
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
    ///
    /// Parses .gitattributes to find patterns with `filter=lfs`.
    pub fn is_tracked(&self, path: &str) -> bool {
        let workdir = match self.repo.workdir() {
            Some(w) => w,
            None => return false,
        };

        let gitattributes = workdir.join(".gitattributes");
        self.path_matches_lfs_pattern(path, &gitattributes)
    }

    /// Check if a path matches any LFS pattern in the given .gitattributes file.
    fn path_matches_lfs_pattern(&self, path: &str, gitattributes: &Path) -> bool {
        let content = match fs::read_to_string(gitattributes) {
            Ok(c) => c,
            Err(_) => return false,
        };

        for line in content.lines() {
            let line = line.trim();

            // Skip comments and empty lines
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            // Check if this line has filter=lfs
            if !line.contains("filter=lfs") {
                continue;
            }

            // Extract the pattern (first whitespace-separated token)
            let pattern = match line.split_whitespace().next() {
                Some(p) => p,
                None => continue,
            };

            // Match pattern against path
            if Self::pattern_matches(pattern, path) {
                return true;
            }
        }

        false
    }

    /// Simple glob pattern matching for gitattributes patterns.
    fn pattern_matches(pattern: &str, path: &str) -> bool {
        // Handle simple cases
        if pattern == path {
            return true;
        }

        // Handle *.ext patterns (most common for LFS)
        if pattern.starts_with("*.") {
            let ext = &pattern[1..]; // ".ext"
            return path.ends_with(ext);
        }

        // Handle **/pattern (matches in any directory)
        if let Some(suffix) = pattern.strip_prefix("**/") {
            // Match at root or in any subdirectory
            return path == suffix || path.ends_with(&format!("/{}", suffix));
        }

        // Handle other wildcards with simple fnmatch-like behavior
        if pattern.contains('*') {
            return Self::glob_match(pattern, path);
        }

        // Direct path match
        pattern == path
    }

    /// Simple glob matching (handles * and **)
    fn glob_match(pattern: &str, path: &str) -> bool {
        let parts: Vec<&str> = pattern.split('*').collect();

        if parts.len() == 1 {
            // No wildcards
            return pattern == path;
        }

        let mut pos = 0;
        for (i, part) in parts.iter().enumerate() {
            if part.is_empty() {
                continue;
            }

            if i == 0 {
                // Must start with this part
                if !path.starts_with(part) {
                    return false;
                }
                pos = part.len();
            } else if i == parts.len() - 1 {
                // Must end with this part
                if !path[pos..].ends_with(part) {
                    return false;
                }
            } else {
                // Must contain this part after current position
                match path[pos..].find(part) {
                    Some(idx) => pos += idx + part.len(),
                    None => return false,
                }
            }
        }

        true
    }

    /// Clean content (working tree -> ODB).
    ///
    /// If the file is tracked by LFS, this generates an LFS pointer,
    /// uploads the content to the LFS server, and stores in local cache.
    pub fn clean(&self, path: &str, content: &[u8]) -> Result<Vec<u8>> {
        if !self.is_tracked(path) {
            return Ok(content.to_vec());
        }

        // Generate pointer
        let pointer = Pointer::from_content(content);

        // Store in cache before upload (for later smudge without network)
        if let Some(cache) = &self.cache {
            let _ = cache.put_verified(&pointer, content);
        }

        // Upload to LFS server
        self.client.upload(&pointer, content)?;

        // Return pointer content
        Ok(pointer.encode_bytes())
    }

    /// Smudge content (ODB -> working tree).
    ///
    /// If the content is an LFS pointer, this checks the local cache first,
    /// then downloads from the LFS server if not cached.
    pub fn smudge(&self, _path: &str, content: &[u8]) -> Result<Vec<u8>> {
        // Check if content is an LFS pointer
        if !Pointer::is_pointer(content) {
            return Ok(content.to_vec());
        }

        // Parse pointer
        let pointer = Pointer::parse(content)?;

        // Check cache first
        if let Some(cache) = &self.cache {
            if let Some(cached) = cache.get_verified(&pointer) {
                return Ok(cached);
            }
        }

        // Download from LFS server
        let downloaded = self.client.download(&pointer)?;

        // Store in cache for future use
        if let Some(cache) = &self.cache {
            let _ = cache.put_verified(&pointer, &downloaded);
        }

        Ok(downloaded)
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

// ============================================================================
// Automatic Filter Registration
// ============================================================================

use git2::{Filter, FilterCheck, FilterMode, FilterSource};

/// Global LFS filter that implements git2's Filter trait.
///
/// This filter is registered with libgit2 and automatically invoked
/// for all files matching `filter=lfs` in `.gitattributes`.
struct GlobalLfsFilter {
    client: LfsClient,
    /// Cache directory path (we can't hold ObjectCache directly due to thread safety)
    cache_path: Option<std::path::PathBuf>,
}

impl GlobalLfsFilter {
    fn get_cache(&self) -> Option<ObjectCache> {
        self.cache_path.as_ref().map(|p| ObjectCache::new(p))
    }
}

impl Filter for GlobalLfsFilter {
    fn check(&self, _src: &FilterSource<'_>) -> std::result::Result<FilterCheck, git2::Error> {
        // Always apply - libgit2 already matched filter=lfs in .gitattributes
        Ok(FilterCheck::Apply)
    }

    fn apply(
        &self,
        src: &FilterSource<'_>,
        input: &[u8],
    ) -> std::result::Result<Vec<u8>, git2::Error> {
        let result = match src.mode() {
            FilterMode::ToOdb => self.clean(input),
            FilterMode::ToWorktree => self.smudge(input),
        };

        result.map_err(|e| git2::Error::from_str(&e.to_string()))
    }
}

impl GlobalLfsFilter {
    /// Clean: working tree -> repository (upload to LFS, return pointer)
    fn clean(&self, content: &[u8]) -> Result<Vec<u8>> {
        // Already a pointer? Pass through
        if Pointer::is_pointer(content) {
            return Ok(content.to_vec());
        }

        // Generate pointer
        let pointer = Pointer::from_content(content);

        // Store in cache
        if let Some(cache) = self.get_cache() {
            let _ = cache.put_verified(&pointer, content);
        }

        // Upload to LFS server
        self.client.upload(&pointer, content)?;

        // Return pointer bytes
        Ok(pointer.encode_bytes())
    }

    /// Smudge: repository -> working tree (download from LFS)
    fn smudge(&self, content: &[u8]) -> Result<Vec<u8>> {
        // Not a pointer? Pass through
        if !Pointer::is_pointer(content) {
            return Ok(content.to_vec());
        }

        // Parse pointer
        let pointer = Pointer::parse(content)?;

        // Check cache first
        if let Some(cache) = self.get_cache() {
            if let Some(cached) = cache.get_verified(&pointer) {
                return Ok(cached);
            }
        }

        // Download from LFS server
        let downloaded = self.client.download(&pointer)?;

        // Store in cache
        if let Some(cache) = self.get_cache() {
            let _ = cache.put_verified(&pointer, &downloaded);
        }

        Ok(downloaded)
    }
}

/// Handle to a registered LFS filter.
///
/// When dropped, the filter is unregistered from libgit2.
/// Keep this handle alive for as long as you want LFS filtering to be active.
pub struct LfsFilterRegistration {
    _inner: git2::FilterRegistration,
}

/// Register an LFS filter globally with libgit2.
///
/// After calling this function, all git2 operations on repositories with
/// `filter=lfs` in `.gitattributes` will automatically use LFS.
///
/// # Arguments
///
/// * `client` - The LFS client to use for uploads/downloads
///
/// # Returns
///
/// A registration handle. The filter remains active until this handle is dropped.
///
/// # Example
///
/// ```ignore
/// use git2::Repository;
/// use git2_lfs::{register_lfs_filter, LfsClient};
///
/// let repo = Repository::open(".")?;
/// let client = LfsClient::from_repo(&repo)?
///     .with_token(&token);
///
/// // Register once at startup
/// let _reg = register_lfs_filter(client)?;
///
/// // Now all git2 operations automatically use LFS
/// repo.checkout_head()?;  // Smudges LFS files
/// ```
pub fn register_lfs_filter(client: LfsClient) -> Result<LfsFilterRegistration> {
    register_lfs_filter_with_cache(client, None)
}

/// Register an LFS filter with a specific cache directory.
///
/// # Arguments
///
/// * `client` - The LFS client to use for uploads/downloads
/// * `cache_path` - Optional path to the cache directory. If None, caching is disabled.
///
/// # Example
///
/// ```ignore
/// use git2_lfs::{register_lfs_filter_with_cache, LfsClient};
/// use std::path::PathBuf;
///
/// let client = LfsClient::new("https://github.com/owner/repo.git")?;
/// let cache = PathBuf::from("/path/to/.git/lfs/objects");
///
/// let _reg = register_lfs_filter_with_cache(client, Some(cache))?;
/// ```
pub fn register_lfs_filter_with_cache(
    client: LfsClient,
    cache_path: Option<std::path::PathBuf>,
) -> Result<LfsFilterRegistration> {
    let filter = GlobalLfsFilter { client, cache_path };

    let registration = git2::filter_register(
        "lfs",
        "filter=lfs",
        git2::filter_priority::DRIVER,
        filter,
    )
    .map_err(|e| crate::Error::Http(format!("failed to register filter: {}", e)))?;

    Ok(LfsFilterRegistration { _inner: registration })
}

/// Register an LFS filter for a specific repository.
///
/// This is a convenience function that reads the LFS configuration from the
/// repository and sets up the cache in the standard location.
///
/// # Arguments
///
/// * `repo` - The repository to configure LFS for
///
/// # Example
///
/// ```ignore
/// use git2::Repository;
/// use git2_lfs::register_lfs_filter_for_repo;
///
/// let repo = Repository::open(".")?;
/// let _reg = register_lfs_filter_for_repo(&repo)?;
///
/// // LFS is now active for all operations
/// ```
pub fn register_lfs_filter_for_repo(repo: &Repository) -> Result<LfsFilterRegistration> {
    let client = LfsClient::from_repo(repo)?;
    let cache_path = Some(repo.path().join("lfs").join("objects"));

    register_lfs_filter_with_cache(client, cache_path)
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
            writeln!(file, "*.png filter=lfs diff=lfs merge=lfs -text").unwrap();
            writeln!(file, "assets/*.dat filter=lfs diff=lfs merge=lfs -text").unwrap();
        }

        let client = LfsClient::new("https://github.com/test/repo.git").unwrap();
        let filter = LfsFilter::with_client(&repo, client);

        // .bin files should be tracked
        assert!(filter.is_tracked("test.bin"));
        assert!(filter.is_tracked("data/large.bin"));

        // .png files should be tracked
        assert!(filter.is_tracked("image.png"));

        // Non-LFS files should not be tracked
        assert!(!filter.is_tracked("readme.txt"));
        assert!(!filter.is_tracked("src/main.rs"));
    }

    #[test]
    fn test_pattern_matching() {
        // Test *.ext patterns
        assert!(LfsFilter::pattern_matches("*.bin", "test.bin"));
        assert!(LfsFilter::pattern_matches("*.bin", "path/to/file.bin"));
        assert!(!LfsFilter::pattern_matches("*.bin", "test.txt"));

        // Test direct path match
        assert!(LfsFilter::pattern_matches("data.bin", "data.bin"));
        assert!(!LfsFilter::pattern_matches("data.bin", "other.bin"));

        // Test directory patterns
        assert!(LfsFilter::pattern_matches("assets/*", "assets/image.png"));
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

    #[test]
    fn test_global_filter_registration() {
        // Test that we can register the global LFS filter
        let client = LfsClient::new("https://github.com/test/repo.git").unwrap();

        // Register the filter
        let registration = register_lfs_filter(client);
        assert!(registration.is_ok(), "Filter registration should succeed");

        // Registration should stay alive
        let _reg = registration.unwrap();

        // Create a second client and try to register with cache
        let client2 = LfsClient::new("https://github.com/test/repo2.git").unwrap();
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join("lfs").join("objects");

        // This should fail because 'lfs' filter is already registered
        let result2 = register_lfs_filter_with_cache(client2, Some(cache_path));
        assert!(result2.is_err(), "Duplicate registration should fail");
    }

    #[test]
    fn test_global_filter_clean_smudge() {
        // Test that GlobalLfsFilter clean/smudge work correctly
        let client = LfsClient::new("https://github.com/test/repo.git").unwrap();
        let filter = GlobalLfsFilter {
            client,
            cache_path: None,
        };

        // Smudge non-pointer content should pass through
        let content = b"regular file content";
        let result = filter.smudge(content).unwrap();
        assert_eq!(result, content);

        // Clean content that's already a pointer should pass through
        let pointer_content = b"version https://git-lfs.github.com/spec/v1\noid sha256:abc123\nsize 1234\n";
        let result = filter.clean(pointer_content).unwrap();
        assert_eq!(result, pointer_content);
    }
}
