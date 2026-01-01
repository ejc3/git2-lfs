# git2-lfs

Pure Rust Git LFS implementation for use with git2.

## Architecture

- `Pointer` - LFS pointer file parsing/generation (sha256 + size)
- `LfsClient` - HTTP client for LFS Batch API (upload/download)
- `LfsFilter` - Clean/smudge filter logic with cache integration
- `LfsRepo` - High-level wrapper making LFS automatic
- `ObjectCache` - Local cache for LFS objects (`.git/lfs/objects`)

## Key Files

- `src/pointer.rs` - Pointer format (matches git-lfs exactly)
- `src/client.rs` - HTTP client, URL derivation, auth, batch operations
- `src/filter.rs` - Filter logic, .gitattributes parsing, cache integration
- `src/repo.rs` - Automatic LFS handling via `LfsRepo`
- `src/cache.rs` - Local object cache with git-lfs standard layout

## Features

- **Spec-compliant pointer format** - Verified against git-lfs CLI
- **Batch API support** - Upload/download multiple objects in one request
- **Local caching** - Objects cached at `.git/lfs/objects/<oid[0:2]>/<oid[2:4]>/<oid>`
- **Ref field support** - Optional ref name in batch requests for access control
- **Automatic LFS** - `LfsRepo.add()` handles upload + pointer generation

## Testing

```bash
cargo test                                    # Unit tests only
cargo test --features git2-integration        # All tests including e2e
cargo test --features git2-integration --test e2e -- --nocapture  # E2E with output
```

## Test Strategy

- **e2e.rs** - Ultimate integration proof (git2 + LfsRepo + GitHub)
- **protocol_verification.rs** - External validation (matches git-lfs CLI, openssl SHA256)
- **integration.rs** - HTTP mock tests, edge cases
- Unit tests in each module for implementation details

## Dependencies

- Uses forked git2-rs from `github.com/ejc3/git2-rs` (has filter API)
- `git2-integration` feature enables git2 support

## LFS URL Format

GitHub requires `.git` in LFS URLs:
- Input: `https://github.com/owner/repo.git`
- LFS endpoint: `https://github.com/owner/repo.git/info/lfs/objects/batch`
