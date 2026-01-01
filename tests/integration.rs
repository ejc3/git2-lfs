//! Integration tests for git2-lfs.
//!
//! These tests verify the full LFS workflow including HTTP client operations.

use git2_lfs::{BatchRequest, BatchRequestObject, LfsClient, Pointer};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;

/// Mock LFS server for testing.
///
/// Listens on a random port and handles batch API requests.
struct MockLfsServer {
    port: u16,
    shutdown_tx: mpsc::Sender<()>,
    handle: Option<thread::JoinHandle<Vec<MockRequest>>>,
}

#[derive(Debug)]
struct MockRequest {
    method: String,
    path: String,
    body: String,
}

impl MockLfsServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (shutdown_tx, shutdown_rx) = mpsc::channel();

        // Set socket to non-blocking for graceful shutdown
        listener.set_nonblocking(true).unwrap();

        let handle = thread::spawn(move || {
            let mut requests = Vec::new();

            loop {
                // Check for shutdown signal
                if shutdown_rx.try_recv().is_ok() {
                    break;
                }

                // Try to accept a connection
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();

                        let mut buffer = [0u8; 4096];
                        let n = stream.read(&mut buffer).unwrap_or(0);
                        let request = String::from_utf8_lossy(&buffer[..n]).to_string();

                        // Parse request
                        let lines: Vec<&str> = request.lines().collect();
                        if let Some(first_line) = lines.first() {
                            let parts: Vec<&str> = first_line.split_whitespace().collect();
                            if parts.len() >= 2 {
                                let method = parts[0].to_string();
                                let path = parts[1].to_string();

                                // Find body (after empty line)
                                let body = if let Some(pos) = request.find("\r\n\r\n") {
                                    request[pos + 4..].to_string()
                                } else {
                                    String::new()
                                };

                                // Send response based on path
                                let response = if path.contains("/objects/batch") {
                                    mock_batch_response()
                                } else {
                                    mock_404_response()
                                };

                                let _ = stream.write_all(response.as_bytes());

                                requests.push(MockRequest { method, path, body });
                            }
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }

            requests
        });

        MockLfsServer {
            port,
            shutdown_tx,
            handle: Some(handle),
        }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/test/repo.git", self.port)
    }

    fn stop(mut self) -> Vec<MockRequest> {
        let _ = self.shutdown_tx.send(());
        if let Some(handle) = self.handle.take() {
            handle.join().unwrap_or_default()
        } else {
            vec![]
        }
    }
}

fn mock_batch_response() -> String {
    let body = r#"{
        "transfer": "basic",
        "objects": [{
            "oid": "4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393",
            "size": 12345,
            "authenticated": true,
            "actions": {
                "download": {
                    "href": "https://example.com/download/4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393",
                    "header": {},
                    "expires_in": 3600
                }
            }
        }]
    }"#;

    format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: application/vnd.git-lfs+json\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {}",
        body.len(),
        body
    )
}

fn mock_404_response() -> String {
    "HTTP/1.1 404 Not Found\r\n\
     Content-Length: 0\r\n\
     \r\n"
        .to_string()
}

#[test]
fn test_client_batch_request() {
    let server = MockLfsServer::start();
    let client = LfsClient::new(&server.url()).unwrap();

    // Create a batch request
    let batch_req = BatchRequest::download(vec![
        BatchRequestObject::new(
            "4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393",
            12345,
        ),
    ]);

    // Send batch request
    let result = client.batch(&batch_req);

    // Stop server and get captured requests
    let requests = server.stop();

    // Verify request was made
    assert!(!requests.is_empty(), "Server should have received requests");

    let req = &requests[0];
    assert_eq!(req.method, "POST");
    assert!(req.path.contains("/objects/batch"));

    // Verify response was parsed
    match result {
        Ok(response) => {
            assert_eq!(response.transfer, "basic");
            assert_eq!(response.objects.len(), 1);
            assert_eq!(
                response.objects[0].oid,
                "4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393"
            );
        }
        Err(e) => panic!("Batch request failed: {:?}", e),
    }
}

#[test]
fn test_client_derives_lfs_url() {
    // Test various URL formats
    let test_cases = vec![
        (
            "https://github.com/owner/repo.git",
            "https://github.com/owner/repo/info/lfs",
        ),
        (
            "https://github.com/owner/repo",
            "https://github.com/owner/repo/info/lfs",
        ),
        (
            "git@github.com:owner/repo.git",
            "https://github.com/owner/repo/info/lfs",
        ),
        (
            "https://gitlab.com/group/project.git",
            "https://gitlab.com/group/project/info/lfs",
        ),
    ];

    for (input, expected) in test_cases {
        let client = LfsClient::new(input).unwrap();
        assert_eq!(
            client.lfs_url().as_str(),
            expected,
            "Failed for input: {}",
            input
        );
    }
}

#[test]
fn test_pointer_workflow() {
    // Simulate clean/smudge workflow
    let original_content = b"This is the original large file content that would be stored in LFS.";

    // Clean: Generate pointer from content
    let pointer = Pointer::from_content(original_content);

    // Verify pointer metadata
    assert_eq!(pointer.size(), original_content.len() as u64);
    assert!(!pointer.oid().to_hex().is_empty());

    // Encode pointer (what gets stored in git)
    let encoded = pointer.encode();
    assert!(encoded.contains("version https://git-lfs.github.com/spec/v1"));
    assert!(encoded.contains(&format!("oid sha256:{}", pointer.oid().to_hex())));
    assert!(encoded.contains(&format!("size {}", pointer.size())));

    // Smudge: Parse pointer back
    let parsed = Pointer::parse(encoded.as_bytes()).unwrap();
    assert_eq!(parsed.oid(), pointer.oid());
    assert_eq!(parsed.size(), pointer.size());

    // Verify is_pointer detection
    assert!(Pointer::is_pointer(encoded.as_bytes()));
    assert!(!Pointer::is_pointer(original_content));
}

#[test]
fn test_client_clone() {
    let client1 = LfsClient::new("https://github.com/test/repo.git").unwrap();
    let client2 = client1.clone();

    // Both should have the same LFS URL
    assert_eq!(client1.lfs_url(), client2.lfs_url());
}

#[test]
fn test_batch_request_json_format() {
    let request = BatchRequest::upload(vec![
        BatchRequestObject::new("abc123", 1024),
        BatchRequestObject::new("def456", 2048),
    ]);

    let json = serde_json::to_value(&request).unwrap();

    // Verify structure matches LFS spec
    assert_eq!(json["operation"], "upload");
    assert!(json["transfers"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("basic")));
    assert_eq!(json["objects"].as_array().unwrap().len(), 2);
    assert_eq!(json["objects"][0]["oid"], "abc123");
    assert_eq!(json["objects"][0]["size"], 1024);
    assert_eq!(json["objects"][1]["oid"], "def456");
    assert_eq!(json["objects"][1]["size"], 2048);
}

#[test]
fn test_pointer_edge_cases() {
    // Empty content
    let empty = Pointer::from_content(b"");
    assert_eq!(empty.size(), 0);

    // Very large size (simulated)
    let pointer_text = b"version https://git-lfs.github.com/spec/v1\n\
        oid sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n\
        size 9999999999999\n";
    let parsed = Pointer::parse(pointer_text).unwrap();
    assert_eq!(parsed.size(), 9999999999999);

    // Old hawser spec version (should still parse)
    let hawser_pointer = b"version https://hawser.github.com/spec/v1\n\
        oid sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n\
        size 100\n";
    assert!(Pointer::parse(hawser_pointer).is_ok());
}
