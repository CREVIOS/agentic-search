//! End-to-end MCP JSON-RPC conformance over the released CLI binary.
//!
//! Drives `agentic-search serve --mcp` with a fixed transcript:
//!   1. `initialize`                  → must get `result.protocolVersion`
//!   2. `notifications/initialized`   → must NOT produce any response
//!   3. `tools/list`                  → must list every shipped tool
//!   4. `tools/call grep`             → must return `structuredContent.spans`
//!   5. unknown method                → must error with -32601
//!
//! Skipped unless `AS_MCP_E2E=1` and the release binary is built. CI
//! sets the env in a dedicated job after `cargo build --release`.

use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn enabled() -> bool {
    std::env::var("AS_MCP_E2E").ok().as_deref() == Some("1")
}

fn bin_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target/release/agentic-search")
}

fn make_corpus() -> PathBuf {
    let dir = std::env::temp_dir().join("as-mcp-transcript");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("a.py"),
        "def alpha(x):\n    return x + 1\n\ndef beta(x):\n    # TODO: optimize beta\n    return x * 2\n",
    )
    .unwrap();
    dir
}

#[test]
fn mcp_transcript_conforms_to_jsonrpc_2_0() {
    if !enabled() {
        eprintln!("AS_MCP_E2E not set; skipping");
        return;
    }
    let bin = bin_path();
    if !bin.exists() {
        panic!(
            "binary missing: {}; run `cargo build --release -p as-cli` first",
            bin.display()
        );
    }
    let corpus = make_corpus();
    let uri = format!("file://{}", corpus.display());

    let mut child = Command::new(&bin)
        .args(["serve", "--mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn mcp server");

    let mut stdin = child.stdin.take().expect("stdin");
    let messages = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}),
        // Notification: no `id`. MUST NOT receive a response.
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {
                "name": "grep",
                "arguments": {"uri": uri, "pattern": "TODO", "max_hits": 5}
            }
        }),
        // Unknown method.
        json!({"jsonrpc": "2.0", "id": 4, "method": "nonexistent/method"}),
    ];
    for msg in &messages {
        writeln!(stdin, "{}", msg).unwrap();
    }
    drop(stdin);

    let output = child.wait_with_output().expect("wait mcp server");
    let stdout = String::from_utf8(output.stdout).expect("utf-8");
    let responses: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("response is json"))
        .collect();

    // We expect exactly 4 responses for the 4 *requests*; the
    // `notifications/initialized` payload MUST NOT generate one.
    assert_eq!(
        responses.len(),
        4,
        "expected 4 responses (no reply for the notification); got {:#?}",
        responses
    );

    // 1) initialize
    let init = &responses[0];
    assert_eq!(init["jsonrpc"], "2.0");
    assert_eq!(init["id"], 1);
    let proto = init["result"]["protocolVersion"].as_str().unwrap();
    assert!(
        proto.starts_with("20"),
        "protocolVersion looks wrong: {proto}"
    );

    // 2) tools/list
    let list = &responses[1];
    assert_eq!(list["id"], 2);
    let tools = list["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for expected in &["ls", "read", "grep", "find_symbol", "search", "delegate"] {
        assert!(
            names.contains(expected),
            "tools/list missing {expected}: {names:?}"
        );
    }
    for t in tools {
        assert!(
            t.get("inputSchema").is_some(),
            "tool {} missing inputSchema",
            t["name"]
        );
        assert!(
            t.get("outputSchema").is_some(),
            "tool {} missing outputSchema",
            t["name"]
        );
    }

    // 3) tools/call grep
    let call = &responses[2];
    assert_eq!(call["id"], 3);
    let structured = &call["result"]["structuredContent"];
    let spans = structured["spans"].as_array().unwrap();
    assert!(!spans.is_empty(), "expected at least one TODO span");
    let first = &spans[0];
    assert!(first["uri"].as_str().unwrap().ends_with("a.py"));
    assert_eq!(first["line_range"][0], 5);

    // 4) unknown method → -32601
    let err = &responses[3];
    assert_eq!(err["id"], 4);
    let code = err["error"]["code"].as_i64().expect("error code");
    assert_eq!(
        code, -32601,
        "unknown method should map to -32601 (Method not found); got {code}"
    );
}
