//! Schema lock for the MCP `tools/list` manifest.
//!
//! Every tool we ship must expose both an `inputSchema` and an
//! `outputSchema` so that MCP clients can structurally type their
//! responses without parsing free-form text. The set of tools is also
//! pinned here so a renamed / removed tool fails CI loudly.

use serde_json::Value;

const EXPECTED_TOOLS: &[&str] = &["ls", "read", "grep", "find_symbol", "search", "delegate"];

fn check_schema(schema: &Value, tool: &str, kind: &str) {
    let obj = schema
        .as_object()
        .unwrap_or_else(|| panic!("{tool}.{kind} is not an object"));
    let ty = obj
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{tool}.{kind} is missing `type`"));
    assert_eq!(
        ty, "object",
        "{tool}.{kind} root type must be `object`, got {ty}"
    );
    let props = obj
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("{tool}.{kind} is missing `properties`"));
    assert!(!props.is_empty(), "{tool}.{kind} has empty `properties`");
}

#[test]
fn manifest_lists_every_advertised_tool() {
    let tools = as_server::mcp_stdio::tools_manifest();
    let names: Vec<String> = tools
        .iter()
        .map(|t| t.get("name").and_then(Value::as_str).unwrap().to_string())
        .collect();
    for want in EXPECTED_TOOLS {
        assert!(
            names.iter().any(|n| n == want),
            "MCP manifest missing tool {want:?}; current set = {names:?}"
        );
    }
    assert_eq!(
        names.len(),
        EXPECTED_TOOLS.len(),
        "MCP manifest has unexpected tools: {names:?}"
    );
}

#[test]
fn every_tool_has_input_and_output_schemas() {
    for tool in as_server::mcp_stdio::tools_manifest() {
        let name = tool.get("name").and_then(Value::as_str).unwrap();
        let desc = tool
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("");
        assert!(
            desc.len() > 20,
            "tool {name} has a thin description ({} chars); make it agent-readable",
            desc.len()
        );
        let input = tool
            .get("inputSchema")
            .unwrap_or_else(|| panic!("tool {name} has no inputSchema"));
        check_schema(input, name, "inputSchema");
        let output = tool
            .get("outputSchema")
            .unwrap_or_else(|| panic!("tool {name} has no outputSchema"));
        check_schema(output, name, "outputSchema");
    }
}
