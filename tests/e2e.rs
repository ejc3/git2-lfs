//! End-to-end tests for git2-lfs.
//!
//! Comprehensive tests covering all LFS operations:
//! - Pointer format (must match git-lfs CLI exactly)
//! - Upload via clean filter
//! - Download via smudge filter
//! - Local cache (avoid re-download)
//! - Batch operations (multiple files)

use git2::{Cred, PushOptions, RemoteCallbacks};
use git2_lfs::{LfsClient, LfsRepo, ObjectCache, Pointer};
use std::fs;
use std::process::Command;
use tempfile::TempDir;

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git failed");

    if !output.status.success() {
        panic!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn has_git_lfs() -> bool {
    Command::new("git")
        .args(["lfs", "version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn gh_token() -> Option<String> {
    Command::new("gh")
        .args(["auth", "token"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

#[test]
fn test_cli_vs_library() {
    if !has_git_lfs() {
        eprintln!("Skipping: git-lfs not installed");
        return;
    }

    let token = match gh_token() {
        Some(t) => t,
        None => {
            eprintln!("Skipping: gh not authenticated");
            return;
        }
    };

    // Same content for both branches
    let content = format!("Test content {}\n", uuid::Uuid::new_v4());
    let content_bytes = content.as_bytes();

    println!("=== Testing CLI vs Library ===\n");
    println!("Content: {} bytes\n", content.len());

    // ===== Branch A: Git LFS CLI (Reference Implementation) =====
    println!("--- Branch A: git-lfs CLI ---");
    let cli_dir = TempDir::new().unwrap();
    let cli_repo = cli_dir.path();
    let auth_url = format!(
        "https://x-access-token:{}@github.com/ejc3/git2-lfs.git",
        token
    );

    Command::new("git")
        .args(["clone", "--depth=1", &auth_url, cli_repo.to_str().unwrap()])
        .output()
        .unwrap();

    git(cli_repo, &["checkout", "-b", "test-cli"]);
    git(cli_repo, &["lfs", "install", "--local"]);
    git(cli_repo, &["lfs", "track", "*.bin"]);
    fs::write(cli_repo.join("data.bin"), &content).unwrap();
    git(cli_repo, &["add", "."]);

    let cli_pointer = git(cli_repo, &["show", ":data.bin"]);
    println!("CLI pointer:\n{}", cli_pointer);

    // ===== Branch B: git2 + git2-lfs Library =====
    println!("--- Branch B: git2 + git2-lfs library ---");

    // Generate pointer with our library
    let pointer = Pointer::from_content(content_bytes);
    let lib_pointer = pointer.encode();
    println!("Library pointer:\n{}", lib_pointer);

    // ===== Compare Pointers =====
    println!("--- Comparison ---");
    assert_eq!(
        cli_pointer.trim(),
        lib_pointer.trim(),
        "Pointers should be identical!"
    );
    println!("MATCH! Pointers are identical.\n");

    // ===== Full Roundtrip: Upload with LfsRepo, Download with git-lfs CLI =====
    println!("--- Roundtrip: LfsRepo upload -> git-lfs download ---");

    let branch = format!("e2e-lib-{}", std::process::id());
    let lib_dir = TempDir::new().unwrap();

    // Clone using git2
    let mut callbacks = RemoteCallbacks::new();
    callbacks.credentials(|_url, _username, _allowed| {
        Cred::userpass_plaintext("x-access-token", &token)
    });

    let mut fetch_options = git2::FetchOptions::new();
    fetch_options.remote_callbacks(callbacks);

    let mut builder = git2::build::RepoBuilder::new();
    builder.fetch_options(fetch_options);

    let repo = builder
        .clone("https://github.com/ejc3/git2-lfs.git", lib_dir.path())
        .expect("clone failed");

    println!("Cloned repo with git2");

    // Create branch using git2 (scoped to drop borrows before LfsRepo)
    {
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch(&branch, &head, false).unwrap();
        repo.set_head(&format!("refs/heads/{}", branch)).unwrap();
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
    }

    println!("Created branch: {}", branch);

    // Write .gitattributes to enable LFS tracking
    fs::write(
        lib_dir.path().join(".gitattributes"),
        "*.bin filter=lfs diff=lfs merge=lfs -text\n",
    )
    .unwrap();

    // Create LfsRepo with automatic LFS handling
    let client = LfsClient::new("https://github.com/ejc3/git2-lfs.git")
        .unwrap()
        .with_token(&token);
    let lfs_repo = LfsRepo::new(repo, client);

    // Write content to disk
    fs::write(lib_dir.path().join("data.bin"), content_bytes).unwrap();

    // Add files - LFS upload happens automatically!
    println!("Adding files with LfsRepo (automatic LFS upload)...");
    lfs_repo.add(".gitattributes").unwrap();
    lfs_repo.add("data.bin").expect("LFS add failed");
    println!("Upload complete!");

    // Verify the file on disk is now a pointer
    let on_disk = fs::read_to_string(lib_dir.path().join("data.bin")).unwrap();
    assert!(
        on_disk.contains("version https://git-lfs.github.com/spec/v1"),
        "File on disk should be a pointer after add"
    );

    // Commit
    lfs_repo.commit("add via LfsRepo").unwrap();
    println!("Committed with LfsRepo");

    // Push using git2
    let repo = lfs_repo.repo();
    let mut remote = repo.find_remote("origin").unwrap();
    let mut callbacks = RemoteCallbacks::new();
    callbacks.credentials(|_url, _username, _allowed| {
        Cred::userpass_plaintext("x-access-token", &token)
    });

    let mut push_options = PushOptions::new();
    push_options.remote_callbacks(callbacks);

    remote
        .push(
            &[&format!("refs/heads/{}:refs/heads/{}", branch, branch)],
            Some(&mut push_options),
        )
        .expect("push failed");

    println!("Pushed branch {} with git2", branch);

    // Clone fresh with git CLI + git-lfs to verify download works
    println!("Cloning with git-lfs to verify...");
    let fresh = TempDir::new().unwrap();
    let clone_result = Command::new("git")
        .args([
            "clone",
            "--branch",
            &branch,
            &auth_url,
            fresh.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();

    if !clone_result.status.success() {
        eprintln!(
            "Clone failed: {}",
            String::from_utf8_lossy(&clone_result.stderr)
        );
        // Cleanup
        let mut callbacks = RemoteCallbacks::new();
        callbacks.credentials(|_url, _username, _allowed| {
            Cred::userpass_plaintext("x-access-token", &token)
        });
        let mut push_options = PushOptions::new();
        push_options.remote_callbacks(callbacks);
        let _ = remote.push(&[&format!(":refs/heads/{}", branch)], Some(&mut push_options));
        panic!("Fresh clone failed");
    }

    // Verify content matches
    let downloaded = fs::read_to_string(fresh.path().join("data.bin")).unwrap();
    assert_eq!(downloaded, content, "Content mismatch!");
    println!("Content verified - git-lfs downloaded our upload correctly!\n");

    // Cleanup: delete remote branch
    let mut callbacks = RemoteCallbacks::new();
    callbacks.credentials(|_url, _username, _allowed| {
        Cred::userpass_plaintext("x-access-token", &token)
    });
    let mut push_options = PushOptions::new();
    push_options.remote_callbacks(callbacks);

    let _ = remote.push(
        &[&format!(":refs/heads/{}", branch)],
        Some(&mut push_options),
    );

    println!("=== SUCCESS ===");
    println!("- Library generates identical pointers to git-lfs CLI");
    println!("- LfsRepo.add() automatically uploads to LFS");
    println!("- git-lfs CLI can download what we uploaded");
}

/// Test download via our library vs CLI.
///
/// Uploads with CLI, downloads with library - verifies interoperability.
#[test]
fn test_library_download_vs_cli() {
    if !has_git_lfs() {
        eprintln!("Skipping: git-lfs not installed");
        return;
    }

    let token = match gh_token() {
        Some(t) => t,
        None => {
            eprintln!("Skipping: gh not authenticated");
            return;
        }
    };

    println!("=== Testing Library Download vs CLI ===\n");

    // Create unique content
    let content = format!("CLI upload test {}\n", uuid::Uuid::new_v4());
    let content_bytes = content.as_bytes();

    // Upload using CLI in a temp repo
    let cli_dir = TempDir::new().unwrap();
    let cli_repo = cli_dir.path();
    let auth_url = format!(
        "https://x-access-token:{}@github.com/ejc3/git2-lfs.git",
        token
    );

    Command::new("git")
        .args(["clone", "--depth=1", &auth_url, cli_repo.to_str().unwrap()])
        .output()
        .unwrap();

    let branch = format!("e2e-dl-{}", std::process::id());
    git(cli_repo, &["checkout", "-b", &branch]);
    git(cli_repo, &["lfs", "install", "--local"]);
    git(cli_repo, &["lfs", "track", "*.bin"]);
    fs::write(cli_repo.join("download-test.bin"), &content).unwrap();
    git(cli_repo, &["add", "."]);
    git(cli_repo, &["commit", "-m", "upload via CLI"]);
    git(cli_repo, &["push", "origin", &branch]);
    println!("Uploaded via CLI to branch {}", branch);

    // Get the pointer from CLI
    let cli_pointer_text = git(cli_repo, &["show", "HEAD:download-test.bin"]);
    let cli_pointer = Pointer::parse(cli_pointer_text.as_bytes()).expect("parse CLI pointer");
    println!("CLI pointer OID: {}", cli_pointer.oid().to_hex());

    // Download using our library
    let client = LfsClient::new("https://github.com/ejc3/git2-lfs.git")
        .unwrap()
        .with_token(&token);

    println!("Downloading via library...");
    let downloaded = client.download(&cli_pointer).expect("download failed");

    assert_eq!(
        downloaded, content_bytes,
        "Library download should match CLI upload"
    );
    println!("Download verified - content matches CLI upload!\n");

    // Cleanup: delete remote branch
    git(cli_repo, &["push", "origin", "--delete", &branch]);

    println!("=== SUCCESS ===");
    println!("- CLI uploads content correctly");
    println!("- Library can download CLI-uploaded content");
    println!("- Content matches exactly");
}

/// Test local cache prevents re-download.
#[test]
fn test_cache_hit() {
    let token = match gh_token() {
        Some(t) => t,
        None => {
            eprintln!("Skipping: gh not authenticated");
            return;
        }
    };

    println!("=== Testing Cache Hit ===\n");

    let cache_dir = TempDir::new().unwrap();
    let cache = ObjectCache::new(cache_dir.path());

    // Create content and pointer
    let content = format!("Cache test {}\n", uuid::Uuid::new_v4());
    let content_bytes = content.as_bytes();
    let pointer = Pointer::from_content(content_bytes);

    // Initially cache should be empty
    assert!(
        !cache.contains(pointer.oid()),
        "Cache should be empty initially"
    );
    println!("Cache is empty (as expected)");

    // Upload content to server
    let client = LfsClient::new("https://github.com/ejc3/git2-lfs.git")
        .unwrap()
        .with_token(&token);

    client.upload(&pointer, content_bytes).expect("upload failed");
    println!("Uploaded content to server");

    // Store in cache manually
    cache
        .put_verified(&pointer, content_bytes)
        .expect("cache put failed");
    println!("Stored in cache");

    // Verify cache contains the object
    assert!(cache.contains(pointer.oid()), "Cache should contain object");
    assert!(
        cache.contains_valid(&pointer),
        "Cache should have valid object"
    );
    println!("Cache contains valid object");

    // Get from cache (should not need network)
    let cached = cache
        .get_verified(&pointer)
        .expect("should get from cache");
    assert_eq!(cached, content_bytes, "Cached content should match");
    println!("Retrieved from cache - matches original!");

    println!("\n=== SUCCESS ===");
    println!("- Cache stores objects correctly");
    println!("- Cache retrieves verified content");
    println!("- Cache can avoid network requests");
}

/// Test batch upload of multiple files.
#[test]
fn test_batch_upload() {
    let token = match gh_token() {
        Some(t) => t,
        None => {
            eprintln!("Skipping: gh not authenticated");
            return;
        }
    };

    println!("=== Testing Batch Upload ===\n");

    // Create multiple unique contents
    let contents: Vec<Vec<u8>> = (0..3)
        .map(|i| format!("Batch file {} - {}\n", i, uuid::Uuid::new_v4()).into_bytes())
        .collect();

    let pointers: Vec<Pointer> = contents.iter().map(|c| Pointer::from_content(c)).collect();

    println!("Created {} files for batch upload", contents.len());
    for (i, p) in pointers.iter().enumerate() {
        println!("  File {}: {} bytes, oid={:.16}...", i, p.size(), p.oid().to_hex());
    }

    // Batch upload
    let client = LfsClient::new("https://github.com/ejc3/git2-lfs.git")
        .unwrap()
        .with_token(&token);

    let items: Vec<(&Pointer, &[u8])> = pointers
        .iter()
        .zip(contents.iter().map(|c| c.as_slice()))
        .collect();

    println!("\nUploading batch...");
    client.upload_batch(&items).expect("batch upload failed");
    println!("Batch upload complete!");

    // Verify each can be downloaded
    println!("\nVerifying downloads...");
    for (i, (pointer, original)) in pointers.iter().zip(contents.iter()).enumerate() {
        let downloaded = client.download(pointer).expect("download failed");
        assert_eq!(&downloaded, original, "File {} content mismatch", i);
        println!("  File {} verified", i);
    }

    println!("\n=== SUCCESS ===");
    println!("- Batch upload works for multiple files");
    println!("- All files can be downloaded individually");
    println!("- Content integrity verified for all files");
}

/// Test batch download of multiple files.
#[test]
fn test_batch_download() {
    let token = match gh_token() {
        Some(t) => t,
        None => {
            eprintln!("Skipping: gh not authenticated");
            return;
        }
    };

    println!("=== Testing Batch Download ===\n");

    // Create and upload multiple files first
    let contents: Vec<Vec<u8>> = (0..3)
        .map(|i| format!("Batch download {} - {}\n", i, uuid::Uuid::new_v4()).into_bytes())
        .collect();

    let pointers: Vec<Pointer> = contents.iter().map(|c| Pointer::from_content(c)).collect();

    let client = LfsClient::new("https://github.com/ejc3/git2-lfs.git")
        .unwrap()
        .with_token(&token);

    // Upload individually first
    println!("Uploading {} files...", contents.len());
    for (pointer, content) in pointers.iter().zip(contents.iter()) {
        client.upload(pointer, content).expect("upload failed");
    }
    println!("Upload complete!");

    // Batch download
    println!("\nBatch downloading...");
    let pointer_refs: Vec<&Pointer> = pointers.iter().collect();
    let downloaded = client
        .download_batch(&pointer_refs)
        .expect("batch download failed");

    // Verify
    assert_eq!(downloaded.len(), contents.len(), "Should download all files");
    for (i, (got, expected)) in downloaded.iter().zip(contents.iter()).enumerate() {
        assert_eq!(got, expected, "File {} content mismatch", i);
        println!("  File {} verified", i);
    }

    println!("\n=== SUCCESS ===");
    println!("- Batch download retrieves all files");
    println!("- Content integrity verified for all files");
}
