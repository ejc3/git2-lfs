//! Protocol verification tests.
//!
//! These tests verify our implementation matches the official git-lfs behavior.

use git2_lfs::{Pointer, BatchRequest, BatchRequestObject, BatchResponse};

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

    assert_eq!(our_output, expected,
        "Our pointer format doesn't match git-lfs!\n\nOurs:\n{}\n\nExpected:\n{}",
        our_output, expected);
}

/// Test pointer parsing matches what git-lfs produces.
#[test]
fn test_parse_git_lfs_pointer() {
    // Pointer as produced by git-lfs
    let git_lfs_pointer = b"version https://git-lfs.github.com/spec/v1\n\
oid sha256:4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393\n\
size 12345\n";

    let parsed = Pointer::parse(git_lfs_pointer).expect("Failed to parse valid pointer");
    assert_eq!(parsed.oid().to_hex(), "4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393");
    assert_eq!(parsed.size(), 12345);
}

/// Test roundtrip: content -> pointer -> encode -> parse
#[test]
fn test_roundtrip_various_sizes() {
    let test_cases = vec![
        vec![0u8; 0],           // Empty file
        vec![0u8; 1],           // 1 byte
        vec![0u8; 100],         // 100 bytes
        vec![0u8; 1024],        // 1KB
        vec![0u8; 1024 * 1024], // 1MB
    ];

    for content in test_cases {
        let pointer1 = Pointer::from_content(&content);
        let encoded = pointer1.encode_bytes();
        let pointer2 = Pointer::parse(&encoded).expect("Failed to parse our own pointer");

        assert_eq!(pointer1.oid().to_hex(), pointer2.oid().to_hex());
        assert_eq!(pointer1.size(), pointer2.size());
    }
}

/// Verify SHA256 computation matches openssl.
#[test]
fn test_sha256_matches_openssl() {
    // These hashes were verified with: echo -n "..." | openssl sha256
    let test_cases = vec![
        ("", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
        ("test", "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"),
        ("Hello, World!", "dffd6021bb2bd5b0af676290809ec3a53191dd81c7f70a4b28688a362182986f"),
    ];

    for (input, expected_hash) in test_cases {
        let pointer = Pointer::from_content(input.as_bytes());
        assert_eq!(pointer.oid().to_hex(), expected_hash,
            "Hash mismatch for input: {:?}", input);
    }
}

/// Test batch request JSON format matches spec.
#[test]
fn test_batch_request_format() {
    let request = BatchRequest::upload(vec![
        BatchRequestObject::new("abc123", 1024),
    ]);

    let json = serde_json::to_value(&request).unwrap();

    // Verify structure matches LFS Batch API spec
    assert_eq!(json["operation"], "upload");
    assert!(json["transfers"].as_array().unwrap().contains(&serde_json::json!("basic")));
    assert_eq!(json["objects"][0]["oid"], "abc123");
    assert_eq!(json["objects"][0]["size"], 1024);
}

/// Test batch response parsing.
#[test]
fn test_batch_response_parsing() {
    // Response format from GitHub LFS server
    let response_json = r#"{
        "transfer": "basic",
        "objects": [{
            "oid": "4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393",
            "size": 12345,
            "authenticated": true,
            "actions": {
                "download": {
                    "href": "https://github-cloud.githubusercontent.com/...",
                    "header": {
                        "Authorization": "RemoteAuth ..."
                    },
                    "expires_in": 3600
                }
            }
        }]
    }"#;

    let response: BatchResponse = serde_json::from_str(response_json).unwrap();
    assert_eq!(response.transfer, "basic");
    assert_eq!(response.objects.len(), 1);
    assert!(response.objects[0].download_action().is_some());
}
