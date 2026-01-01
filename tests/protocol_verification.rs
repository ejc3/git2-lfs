//! Protocol verification tests.
//!
//! These tests verify our implementation matches the official git-lfs behavior.

use git2_lfs::Pointer;

/// Verify our pointer output exactly matches git-lfs CLI output.
#[test]
fn test_pointer_matches_git_lfs_cli() {
    // Test content
    let content = b"Hello, World!";

    // Generate pointer with our implementation
    let pointer = Pointer::from_content(content);
    let our_output = pointer.encode();

    // Expected from git-lfs (verified via `git-lfs pointer --file`)
    let expected = "version https://git-lfs.github.com/spec/v1\n\
                    oid sha256:dffd6021bb2bd5b0af676290809ec3a53191dd81c7f70a4b28688a362182986f\n\
                    size 13\n";

    assert_eq!(
        our_output, expected,
        "Our pointer format doesn't match git-lfs!\n\nOurs:\n{}\n\nExpected:\n{}",
        our_output, expected
    );
}

/// Test pointer parsing matches what git-lfs produces.
#[test]
fn test_parse_git_lfs_pointer() {
    // Pointer as produced by git-lfs
    let git_lfs_pointer = b"version https://git-lfs.github.com/spec/v1\n\
oid sha256:4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393\n\
size 12345\n";

    let parsed = Pointer::parse(git_lfs_pointer).expect("Failed to parse valid pointer");
    assert_eq!(
        parsed.oid().to_hex(),
        "4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393"
    );
    assert_eq!(parsed.size(), 12345);
}

// NOTE: test_roundtrip_various_sizes removed - e2e test proves roundtrip works,
// and test_pointer_matches_git_lfs_cli verifies format correctness

/// Verify SHA256 computation matches openssl.
#[test]
fn test_sha256_matches_openssl() {
    // These hashes were verified with: echo -n "..." | openssl sha256
    let test_cases = vec![
        (
            "",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        (
            "test",
            "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
        ),
        (
            "Hello, World!",
            "dffd6021bb2bd5b0af676290809ec3a53191dd81c7f70a4b28688a362182986f",
        ),
    ];

    for (input, expected_hash) in test_cases {
        let pointer = Pointer::from_content(input.as_bytes());
        assert_eq!(
            pointer.oid().to_hex(),
            expected_hash,
            "Hash mismatch for input: {:?}",
            input
        );
    }
}

// NOTE: Batch request/response tests are in src/batch.rs unit tests
// and test_client_batch_request in integration.rs tests the full flow
