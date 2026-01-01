# git2-lfs

Pure Rust Git LFS implementation for use with [git2](https://github.com/rust-lang/git2-rs).

## Overview

This library provides Git LFS (Large File Storage) support for Rust applications using git2. Instead of shelling out to the `git-lfs` CLI, it implements the LFS protocol natively, giving you programmatic control over LFS operations.

## Features

### Implemented

| Feature | Status | Notes |
|---------|--------|-------|
| **Pointer format** | ✅ Complete | Spec-compliant, verified against git-lfs CLI |
| **Batch API** | ✅ Complete | Upload/download multiple objects per request |
| **Streaming I/O** | ✅ Complete | `upload_file()`, `download_to_file()` for large files |
| **Object cache** | ✅ Complete | CLI-compatible layout at `.git/lfs/objects/` |
| **Clean/smudge filter** | ✅ Complete | Transforms content ↔ pointer |
| **Config discovery** | ✅ Complete | Reads `.lfsconfig`, git config, derives from remote |
| **Authentication** | ✅ Complete | Bearer token, basic auth |
| **Ref field** | ✅ Complete | For server-side access control |
| **Cache integration** | ✅ Complete | Filter checks cache before network |

### Not Implemented

| Feature | Priority | Notes |
|---------|----------|-------|
| **Locking API** | Medium | File locking for team collaboration (`POST /locks`) |
| **Verify callback** | Low | POST to verify endpoint after upload |
| **SSH authentication** | Medium | SSH-based auth (we only support HTTPS) |
| **Transfer adapters** | Low | Custom backends (S3, Azure, etc.) |
| **Retry/resume** | Medium | Automatic retry on transient failures |
| **Pre-push hook** | Low | CLI concern, not library |

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
git2-lfs = { git = "https://github.com/ejc3/git2-lfs", features = ["git2-integration"] }
```

Note: Requires the forked git2-rs with filter API support.

## Usage

### High-Level: LfsRepo

For automatic LFS handling based on `.gitattributes`:

```rust
use git2::Repository;
use git2_lfs::LfsRepo;

// Open repository with LFS support
let repo = Repository::open(".")?;
let lfs = LfsRepo::open(&repo)?;  // Auto-discovers LFS config

// Add files - automatically handles LFS based on .gitattributes
lfs.add("large-model.bin")?;   // → uploads to LFS, stores pointer
lfs.add("README.md")?;         // → normal git (not tracked by LFS)

// Commit
lfs.commit("Add model")?;

// Checkout - download LFS files
lfs.smudge_all()?;
```

### Mid-Level: LfsFilter

For manual clean/smudge operations:

```rust
use git2::Repository;
use git2_lfs::{LfsClient, LfsFilter};

let repo = Repository::open(".")?;
let client = LfsClient::from_repo(&repo)?;  // Reads .lfsconfig
let filter = LfsFilter::with_client(&repo, client);

// Clean: content → pointer (on add)
let pointer_bytes = filter.clean("model.bin", &large_content)?;

// Smudge: pointer → content (on checkout)
let content = filter.smudge("model.bin", &pointer_bytes)?;
```

### Low-Level: LfsClient

For direct LFS server operations:

```rust
use git2_lfs::{LfsClient, Pointer};

// Create client
let client = LfsClient::new("https://github.com/owner/repo.git")?
    .with_token(&github_token);

// Upload
let pointer = Pointer::from_content(&data);
client.upload(&pointer, &data)?;

// Download
let content = client.download(&pointer)?;

// Streaming (for large files)
let pointer = client.upload_file("huge-file.bin")?;
client.download_to_file(&pointer, "output.bin")?;

// Batch operations
client.upload_batch(&[(&ptr1, &data1), (&ptr2, &data2)])?;
let contents = client.download_batch(&[&ptr1, &ptr2])?;
```

## How It Works

Git LFS uses a **filter** mechanism to intercept file content:

```
              CLEAN (git add)                    SMUDGE (git checkout)
Working Dir ──────────────────► Repository ──────────────────────► Working Dir
 (50 MB file)                   (133 byte pointer)                  (50 MB file)
                                      │
                                      ▼
                              ┌──────────────┐
                              │  LFS Server  │
                              │  (stores the │
                              │   50 MB)     │
                              └──────────────┘
```

**Pointer file** (what git stores):
```
version https://git-lfs.github.com/spec/v1
oid sha256:4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393
size 52428800
```

**Tracking** is configured in `.gitattributes`:
```
*.bin filter=lfs diff=lfs merge=lfs -text
*.psd filter=lfs diff=lfs merge=lfs -text
```

## Configuration

The library reads LFS configuration automatically (in precedence order):

1. `lfs.url` in `.git/config` (local override)
2. `lfs.url` in `.lfsconfig` (repository-level)
3. `remote.<name>.lfsurl` (per-remote)
4. Derived from remote URL (append `/info/lfs`)

Example `.lfsconfig`:
```ini
[lfs]
    url = https://my-lfs-server.example.com/storage
```

## Testing

```bash
# Unit tests only
cargo test

# All tests including e2e (requires git-lfs CLI + GitHub auth)
cargo test --features git2-integration

# E2E with output
cargo test --features git2-integration --test e2e -- --nocapture
```

### E2E Test Requirements

- `git-lfs` CLI installed
- GitHub authentication via `gh auth login`
- Network access to GitHub LFS

The e2e tests verify our implementation matches git-lfs CLI behavior exactly:
- Pointer format byte-for-byte identical
- Cache layout matches CLI
- Upload/download interoperability

## Architecture

```
src/
├── lib.rs          # Public API exports
├── pointer.rs      # LFS pointer parsing/encoding
├── oid.rs          # SHA256 OID + HashingWriter
├── client.rs       # HTTP client, batch API, config discovery
├── cache.rs        # Local object cache (.git/lfs/objects/)
├── filter.rs       # Clean/smudge filter logic
├── repo.rs         # High-level LfsRepo wrapper
├── batch.rs        # Batch request/response types
└── error.rs        # Error types
```

## Limitations

### No Transparent git2 Integration

Unlike the git CLI (which spawns `git-lfs` as a filter process), git2/libgit2 doesn't automatically run filters. You must explicitly use `LfsRepo` or `LfsFilter` - there's no way to make `repo.index().add_path()` automatically handle LFS.

### HTTPS Only

SSH-based LFS authentication is not implemented. Use HTTPS URLs with token auth.

### No Locking

File locking API (`git lfs lock`) is not implemented. This is mainly needed for team workflows with binary files that can't be merged.

## License

MIT

## Related Projects

- [git-lfs](https://git-lfs.com/) - Official Git LFS implementation (Go)
- [git2-rs](https://github.com/rust-lang/git2-rs) - Rust bindings to libgit2
- [git2-rs fork](https://github.com/ejc3/git2-rs) - Fork with filter API support
