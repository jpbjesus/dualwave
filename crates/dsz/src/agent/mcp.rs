//! Minimal stdio MCP adapter; stdout contains JSON-RPC messages only.
use std::io::{self, BufRead, Write};
use serde_json::{json, Value};

#[derive(Default)]
struct Session { initialized: bool, ready: bool }

impl Session {
    fn handle(&mut self, request: Value, call: &mut impl FnMut(&str,Value)->Result<Value,String>) -> Option<Value> {
        let id = request.get("id").cloned();
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        if request.get("jsonrpc").and_then(Value::as_str) != Some("2.0") || method.is_empty() || request.is_array() {
            return Some(error(id.unwrap_or(Value::Null),-32600,"Invalid Request"));
        }
        if id.is_none() {
            if method == "notifications/initialized" && self.initialized { self.ready = true; }
            return None;
        }
        let id = id.unwrap();
        let params = request.get("params").cloned().unwrap_or(json!({}));
        let result = match method {
            "initialize" => {
                if self.initialized { return Some(error(id,-32600,"Already initialized")); }
                if params.get("protocolVersion").and_then(Value::as_str).is_none()
                    || !params.get("clientInfo").is_some_and(Value::is_object)
                    || !params.get("capabilities").is_some_and(Value::is_object) {
                    return Some(error(id,-32602,"Expected protocolVersion, clientInfo and capabilities"));
                }
                self.initialized = true;
                let requested = params["protocolVersion"].as_str().unwrap();
                let version = if ["2024-11-05","2025-03-26","2025-06-18","2025-11-25"].contains(&requested) { requested } else { "2025-11-25" };
                json!({"protocolVersion":version,"capabilities":{"tools":{}},
                    "serverInfo":{"name":"dualwave","version":env!("CARGO_PKG_VERSION")},
                    "instructions":"Configure the running DualWave app. Enable Agent access in Preferences → Integrations. Read a profile and its revision before changing it. describe provides defaults and valid button/key values. Never overwrite unrelated mappings."})
            }
            "ping" => json!({}),
            _ if !self.ready => return Some(error(id,-32002,"Initialize the MCP session first")),
            "tools/list" => json!({"tools":super::tools()}),
            "tools/call" => {
                let Some(name) = params.get("name").and_then(Value::as_str) else { return Some(error(id,-32602,"Expected tool name")); };
                if !super::tools().iter().any(|t|t["name"] == name) { return Some(error(id,-32602,"Unknown tool")); }
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                if !args.is_object() { return Some(error(id,-32602,"Arguments must be an object")); }
                match call(name,args) {
                    Ok(value) => json!({"content":[{"type":"text","text":value.to_string()}],"isError":false}),
                    Err(message) => json!({"content":[{"type":"text","text":message}],"isError":true}),
                }
            }
            _ => return Some(error(id,-32601,"Method not found")),
        };
        Some(json!({"jsonrpc":"2.0","id":id,"result":result}))
    }
}

fn error(id: Value, code: i32, message: &str) -> Value { json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}}) }

pub(super) fn run(reader: &mut impl BufRead, writer: &mut impl Write, mut call: impl FnMut(&str,Value)->Result<Value,String>) -> io::Result<()> {
    let mut session = Session::default();
    loop {
        let request = match super::transport::line(reader) {
            Ok(Some(value)) => value,
            Ok(None) => return Ok(()),
            Err(e) => {
                super::transport::write(writer,&error(Value::Null,-32700,&e.to_string()))?;
                // An oversized or truncated frame cannot be resynchronized safely.
                return Ok(());
            }
        };
        if let Some(response) = session.handle(request,&mut call) { super::transport::write(writer,&response)?; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lifecycle_discovery_calls_and_notifications() {
        let input = [
            json!({"jsonrpc":"2.0","id":0,"method":"tools/list"}),
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_profiles"}}),
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"get_profile","arguments":{"profile":"missing"}}}),
            json!({"jsonrpc":"2.0","id":5,"method":"unknown"}),
        ].iter().map(|v|format!("{v}\n")).collect::<String>();
        let mut output = Vec::new();
        run(&mut input.as_bytes(),&mut output,|name,_|if name == "list_profiles" {Ok(json!({"profiles":["Xbox"]}))} else {Err("Profile not found".into())}).unwrap();
        let responses: Vec<Value> = String::from_utf8(output).unwrap().lines().map(|l|serde_json::from_str(l).unwrap()).collect();
        assert_eq!(responses.len(),6);
        assert_eq!(responses[0]["error"]["code"],-32002);
        assert_eq!(responses[1]["result"]["protocolVersion"],"2025-11-25");
        assert_eq!(responses[2]["result"]["tools"].as_array().unwrap().len(),super::super::tools().len());
        assert_eq!(responses[3]["result"]["isError"],false);
        assert_eq!(responses[4]["result"]["isError"],true);
        assert_eq!(responses[5]["error"]["code"],-32601);
    }
}
