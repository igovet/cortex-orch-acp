//! The side chat's read-only project files: a minimal stdio MCP server for one
//! side-chat engine session, rooted at the task's checkout.
//!
//! Started by the session's App Server (Codex) from the service binary itself
//! (`<service> --side-chat-files <checkout>`), as the one MCP server of that
//! session ([`entry`]). It has no runtime task, channel, credential or store:
//! it offers `read_file`, `list_directory` and `search_files`, and nothing that
//! writes, deletes or runs anything. Every path is project-relative and checked
//! the way a channel's `project_read` checks it ([`crate::native_project::safe`]:
//! no absolute path, `..`, `.git`, `.cortex`, `.cortex-dev` or dev owner file),
//! then resolved with symlinks and kept inside the checkout, and refused when
//! the native path rule ([`crate::native_engine_policy::path_class`]) calls it
//! Cortex private state or credential material, or its name is a private file
//! name. Listings and searches leave the same entries out. Reads are bounded.
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

pub const MODE: &str = "--side-chat-files";
/// The server's name in the session; its tools reach the model as `mcp__project_files__<tool>`.
pub const SERVER: &str = "project_files";
const MAX_FILE: u64 = 2 * 1024 * 1024;
const MAX_LINES: usize = 2000;
const MAX_REPLY: usize = 120_000;
const MAX_LISTED: usize = 500;
const MAX_SEARCHED_FILES: usize = 5000;
const MAX_SEARCH_FILE: u64 = 1024 * 1024;
const MAX_MATCHES: usize = 200;
const MAX_MATCH_LINE: usize = 300;

/// The MCP server entry a side-chat Codex launch hands its App Server, or `None`
/// when the service binary or the checkout cannot be named.
pub fn entry(checkout: &Path) -> Option<cortex_engine::broker::RelayEntry> {
    let service = std::env::current_exe().ok()?;
    let checkout = checkout.canonicalize().ok().filter(|c| c.is_dir())?;
    cortex_engine::broker::RelayEntry::stdio(
        SERVER,
        &service,
        vec![MODE.to_owned(), checkout.to_str()?.to_owned()],
    )
    .ok()
}

/// Called first in `main`: serves the session's files and exits when this
/// process was started as the server.
pub fn dispatch() {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() == Some(std::ffi::OsStr::new(MODE)) {
        let root = args.next().map(PathBuf::from);
        std::process::exit(match (root, args.next()) {
            (Some(root), None) => serve(&root, std::io::stdin().lock(), std::io::stdout().lock()),
            _ => 64,
        });
    }
}

/// Newline-delimited JSON-RPC over the given streams until the input ends.
pub fn serve(root: &Path, input: impl BufRead, mut output: impl Write) -> i32 {
    let Some(root) = root.canonicalize().ok().filter(|r| r.is_dir() && r.is_absolute()) else {
        return 66;
    };
    for line in input.lines() {
        let Ok(line) = line else { return 1 };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(request) => handle(&root, &request),
            Err(_) => Some(json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"parse error"}})),
        };
        if let Some(reply) = reply {
            if writeln!(output, "{reply}").and_then(|_| output.flush()).is_err() {
                return 1;
            }
        }
    }
    0
}

fn tools() -> Value {
    let read_only = json!({"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false});
    json!([
        {"name":"read_file","description":"Read one text file of the task's checkout, by project-relative path. Optional 1-based start line and line count.",
         "inputSchema":{"type":"object","properties":{
            "path":{"type":"string","description":"Project-relative file path."},
            "offset":{"type":"integer","minimum":1,"description":"First line to return, 1-based."},
            "limit":{"type":"integer","minimum":1,"maximum":MAX_LINES,"description":"Number of lines to return."}},
          "required":["path"],"additionalProperties":false},"annotations":read_only},
        {"name":"list_directory","description":"List one folder of the task's checkout. Folders end with '/'.",
         "inputSchema":{"type":"object","properties":{
            "path":{"type":"string","description":"Project-relative folder path; '.' is the checkout."}},
          "additionalProperties":false},"annotations":read_only},
        {"name":"search_files","description":"Find a literal text in the task's checkout's text files. Answers path:line: text.",
         "inputSchema":{"type":"object","properties":{
            "pattern":{"type":"string","minLength":1,"maxLength":200,"description":"Literal text to find."},
            "path":{"type":"string","description":"Project-relative folder or file to search; '.' is the checkout."},
            "ignore_case":{"type":"boolean","description":"Match regardless of case."}},
          "required":["pattern"],"additionalProperties":false},"annotations":read_only}
    ])
}

/// How a Codex model reaches this server, rendered from [`tools`] itself.
///
/// Codex's code mode (the `exec` tool, which the Luna and Sol models use) leaves
/// MCP tools out of its own description and lists them only in `ALL_TOOLS`, so a
/// model that is not told never learns that the server exists (observed with
/// Codex 0.166 and codex-acp 2.6.0: the tools answer when called, but the model
/// reports it has no file tools). Derived output, like the worker tool cheat
/// sheet; nothing here is authored per tool.
pub fn codex_cheat_sheet() -> String {
    let mut out = format!(
        "# Project files tools (code mode)\n\nThese read-only tools of the task's folder are \
         nested tools of `exec`; they are not listed in its description, but they exist. Use them \
         instead of any Read, Grep or Glob tool. Call them as \
         `await tools.mcp__{SERVER}__<name>({{...}})` and print the result with `text(...)`; \
         `ALL_TOOLS` lists them too. Paths are relative to the task's folder.\n"
    );
    for tool in tools().as_array().into_iter().flatten() {
        let name = tool["name"].as_str().unwrap_or("");
        let required: Vec<&str> = tool["inputSchema"]["required"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let mut args = Vec::new();
        let mut example = Vec::new();
        for (arg, spec) in tool["inputSchema"]["properties"].as_object().into_iter().flatten() {
            let kind = spec["type"].as_str().unwrap_or("string");
            let needed = required.contains(&arg.as_str());
            args.push(format!(
                "{arg} ({kind}, {}): {}",
                if needed { "required" } else { "optional" },
                spec["description"].as_str().unwrap_or("")
            ));
            if needed {
                example.push(match kind {
                    "string" => format!("{arg}: \"...\""),
                    "boolean" => format!("{arg}: true"),
                    _ => format!("{arg}: 1"),
                });
            }
        }
        out.push_str(&format!(
            "\n- `{name}`: {}\n  Arguments: {}\n  `text(JSON.stringify(await tools.mcp__{SERVER}__{name}({{{}}})))`\n",
            tool["description"].as_str().unwrap_or(""),
            args.join("; "),
            example.join(", ")
        ));
    }
    out
}

/// One JSON-RPC message; `None` for a notification.
pub fn handle(root: &Path, request: &Value) -> Option<Value> {
    let id = request.get("id").cloned()?;
    let ok = |result: Value| Some(json!({"jsonrpc":"2.0","id":id,"result":result}));
    match request["method"].as_str().unwrap_or("") {
        "initialize" => ok(json!({
            "protocolVersion": request["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "cortex-side-chat-files", "version": env!("CARGO_PKG_VERSION")},
        })),
        "ping" => ok(json!({})),
        "tools/list" => ok(json!({"tools": tools()})),
        "tools/call" => {
            let args = &request["params"]["arguments"];
            let outcome = match request["params"]["name"].as_str().unwrap_or("") {
                "read_file" => read_file(root, args),
                "list_directory" => list_directory(root, args),
                "search_files" => search_files(root, args),
                _ => Err("unknown_tool: this server only reads files".to_owned()),
            };
            let (text, error) = match outcome {
                Ok(text) => (text, false),
                Err(reason) => (format!("Refused: {reason}"), true),
            };
            ok(json!({"content":[{"type":"text","text":text}],"isError":error}))
        }
        _ => Some(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"method not found"}})),
    }
}

fn private_component(path: &str) -> bool {
    path.split('/')
        .any(|p| crate::machine::short_text::private_name(p) || crate::native_engine_policy::project_private(Path::new(p)))
}
/// The checked absolute path of `relative`, inside `root`.
pub fn resolve(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = relative.trim().trim_end_matches('/');
    let relative = if relative.is_empty() { "." } else { relative };
    crate::native_project::safe(relative).map_err(|_| "private_or_invalid_path".to_owned())?;
    if relative != "." && private_component(relative) {
        return Err("private_path".into());
    }
    let full = if relative == "." { root.to_owned() } else { root.join(relative) };
    let real = full.canonicalize().map_err(|_| "not_found".to_owned())?;
    if !real.starts_with(root) {
        return Err("outside_checkout".into());
    }
    if real != root {
        let inside = real.strip_prefix(root).map_err(|_| "outside_checkout".to_owned())?;
        if private_component(&inside.to_string_lossy()) {
            return Err("private_path".into());
        }
    }
    use cortex_engine::permission::PathClass;
    match crate::native_engine_policy::path_class(root, &real) {
        PathClass::Private | PathClass::LiveStore => Err("cortex_private".into()),
        PathClass::Credential => Err("credential".into()),
        _ => Ok(real),
    }
}
fn text_of(path: &Path, max: u64) -> Result<String, String> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| "unreadable".to_owned())?;
    let meta = file.metadata().map_err(|_| "unreadable".to_owned())?;
    if !meta.is_file() {
        return Err("not_a_file".into());
    }
    if meta.len() > max {
        return Err("too_large".into());
    }
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes).map_err(|_| "unreadable".to_owned())?;
    if bytes.contains(&0) {
        return Err("binary".into());
    }
    String::from_utf8(bytes).map_err(|_| "not_text".to_owned())
}
fn clip(mut text: String, max: usize) -> String {
    if text.len() > max {
        let mut end = max;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n[truncated]");
    }
    text
}
fn read_file(root: &Path, args: &Value) -> Result<String, String> {
    let path = args["path"].as_str().ok_or("path_required")?;
    let real = resolve(root, path)?;
    let text = text_of(&real, MAX_FILE)?;
    let lines: Vec<&str> = text.lines().collect();
    let start = args["offset"].as_u64().unwrap_or(1).max(1) as usize;
    let count = (args["limit"].as_u64().unwrap_or(MAX_LINES as u64) as usize).clamp(1, MAX_LINES);
    let shown: Vec<String> = lines
        .iter()
        .enumerate()
        .skip(start - 1)
        .take(count)
        .map(|(i, l)| format!("{:>6}\t{l}", i + 1))
        .collect();
    let end = (start - 1 + shown.len()).max(start - 1);
    Ok(clip(
        format!("{path}: lines {start}-{end} of {}\n{}", lines.len(), shown.join("\n")),
        MAX_REPLY,
    ))
}
fn listed(root: &Path, dir: &Path) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_owned();
            let relative = e.path().strip_prefix(root).ok()?.to_string_lossy().into_owned();
            if crate::native_project::safe(&relative).is_err() || private_component(&relative) {
                return None;
            }
            let kind = e.file_type().ok()?;
            Some((name, kind.is_dir()))
        })
        .collect();
    out.sort();
    out
}
fn list_directory(root: &Path, args: &Value) -> Result<String, String> {
    let path = args["path"].as_str().unwrap_or(".");
    let real = resolve(root, path)?;
    if !real.is_dir() {
        return Err("not_a_folder".into());
    }
    let entries = listed(root, &real);
    let mut text: Vec<String> = entries
        .iter()
        .take(MAX_LISTED)
        .map(|(n, dir)| if *dir { format!("{n}/") } else { n.clone() })
        .collect();
    if entries.len() > MAX_LISTED {
        text.push(format!("[{} more]", entries.len() - MAX_LISTED));
    }
    Ok(text.join("\n"))
}
fn search_files(root: &Path, args: &Value) -> Result<String, String> {
    let pattern = args["pattern"]
        .as_str()
        .filter(|p| !p.is_empty() && p.len() <= 200)
        .ok_or("pattern_required")?;
    let fold = args["ignore_case"] == true;
    let needle = if fold { pattern.to_lowercase() } else { pattern.to_owned() };
    let start = resolve(root, args["path"].as_str().unwrap_or("."))?;
    let (mut matches, mut files, mut stack) = (Vec::new(), 0usize, vec![start]);
    while let Some(next) = stack.pop() {
        if matches.len() >= MAX_MATCHES || files >= MAX_SEARCHED_FILES {
            break;
        }
        if next.is_dir() {
            // Symlinked folders are not followed; their files are never searched.
            if fs::symlink_metadata(&next).is_ok_and(|m| m.file_type().is_symlink()) {
                continue;
            }
            let mut children = listed(root, &next);
            children.reverse();
            stack.extend(children.into_iter().map(|(n, _)| next.join(n)));
            continue;
        }
        files += 1;
        let Ok(text) = text_of(&next, MAX_SEARCH_FILE) else { continue };
        let relative = next.strip_prefix(root).unwrap_or(&next).to_string_lossy().into_owned();
        for (i, line) in text.lines().enumerate() {
            let hay = if fold { line.to_lowercase() } else { line.to_owned() };
            if hay.contains(&needle) {
                matches.push(format!("{relative}:{}: {}", i + 1, clip(line.to_owned(), MAX_MATCH_LINE)));
                if matches.len() >= MAX_MATCHES {
                    break;
                }
            }
        }
    }
    if matches.is_empty() {
        return Ok("No matches.".into());
    }
    if matches.len() >= MAX_MATCHES {
        matches.push(format!("[stopped at {MAX_MATCHES} matches]"));
    }
    Ok(clip(matches.join("\n"), MAX_REPLY))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::DirBuilderExt;

    fn checkout() -> PathBuf {
        let base = std::env::temp_dir().join(format!("cw-side-files-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = fs::remove_dir_all(&base);
        fs::DirBuilder::new().mode(0o700).recursive(true).create(base.join("project/src")).unwrap();
        let root = base.join("project").canonicalize().unwrap();
        fs::write(root.join("src/lib.rs"), "fn upload() {}\nfn retry() {}\n").unwrap();
        fs::write(root.join(".env"), "TOKEN=x\n").unwrap();
        fs::write(base.join("outside.txt"), "outside\n").unwrap();
        fs::create_dir_all(root.join(".cortex")).unwrap();
        fs::write(root.join(".cortex/cortex.sqlite3"), "store").unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git/config"), "[core]").unwrap();
        std::os::unix::fs::symlink(base.join("outside.txt"), root.join("link.txt")).unwrap();
        root
    }
    fn call(root: &Path, name: &str, arguments: Value) -> (String, bool) {
        let reply = handle(root, &json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":arguments}})).unwrap();
        let r = &reply["result"];
        (r["content"][0]["text"].as_str().unwrap().to_owned(), r["isError"] == true)
    }

    #[test]
    fn the_codex_cheat_sheet_is_rendered_from_the_served_tools() {
        let sheet = codex_cheat_sheet();
        for tool in tools().as_array().unwrap() {
            let name = tool["name"].as_str().unwrap();
            assert!(sheet.contains(&format!("tools.mcp__{SERVER}__{name}(")), "{name}");
            assert!(sheet.contains(tool["description"].as_str().unwrap()));
        }
        assert!(sheet.contains("path (string, required)") && sheet.contains("offset (integer, optional)"));
        assert!(sheet.contains("read_file({path: \"...\"})") && sheet.contains("search_files({pattern: \"...\"})"));
        assert!(sheet.contains("ALL_TOOLS"));
    }

    #[test]
    fn reads_lists_and_searches_inside_the_checkout_only() {
        let root = checkout();
        let names: Vec<String> = handle(&root, &json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})).unwrap()["result"]["tools"]
            .as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_owned()).collect();
        assert_eq!(names, ["read_file", "list_directory", "search_files"]);
        let (text, error) = call(&root, "read_file", json!({"path":"src/lib.rs","offset":2}));
        assert!(!error && text.contains("fn retry()") && !text.contains("fn upload()"), "{text}");
        let (text, error) = call(&root, "list_directory", json!({"path":"."}));
        assert!(!error, "{text}");
        // A symlink is named, never followed (reading it is refused below).
        assert_eq!(text, "link.txt\nsrc/");
        let (text, _) = call(&root, "search_files", json!({"pattern":"RETRY","ignore_case":true}));
        assert_eq!(text, "src/lib.rs:2: fn retry() {}");
        let (text, _) = call(&root, "search_files", json!({"pattern":"TOKEN"}));
        assert_eq!(text, "No matches.");
        fs::remove_dir_all(root.parent().unwrap()).unwrap();
    }

    #[test]
    fn escapes_private_paths_credentials_and_writes_are_refused() {
        let root = checkout();
        for path in ["../outside.txt", "/etc/passwd", "link.txt", ".env", ".cortex/cortex.sqlite3", ".git/config", "src/../../outside.txt", "absent.txt"] {
            let (text, error) = call(&root, "read_file", json!({"path":path}));
            assert!(error && text.starts_with("Refused:"), "{path}: {text}");
        }
        for path in [".cortex", ".git", ".."] {
            assert!(call(&root, "list_directory", json!({"path":path})).1, "{path}");
        }
        for tool in ["write_file", "edit_file", "delete_file", "run_command", "project_write"] {
            let (text, error) = call(&root, tool, json!({"path":"src/lib.rs","content":"x"}));
            assert!(error && text.contains("unknown_tool"), "{tool}");
        }
        assert_eq!(fs::read_to_string(root.join("src/lib.rs")).unwrap(), "fn upload() {}\nfn retry() {}\n");
        // A notification gets no reply; an unknown method is an error.
        assert!(handle(&root, &json!({"jsonrpc":"2.0","method":"notifications/initialized"})).is_none());
        assert!(handle(&root, &json!({"jsonrpc":"2.0","id":2,"method":"resources/list"})).unwrap()["error"].is_object());
        fs::remove_dir_all(root.parent().unwrap()).unwrap();
    }

    #[test]
    fn serves_newline_json_rpc() {
        let root = checkout();
        let input = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-06-18\"}}\n\
                     {\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n\
                     {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"read_file\",\"arguments\":{\"path\":\"src/lib.rs\"}}}\n";
        let mut out = Vec::new();
        assert_eq!(serve(&root, input.as_bytes(), &mut out), 0);
        let lines: Vec<Value> = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["result"]["capabilities"]["tools"]["listChanged"], false);
        assert!(lines[1]["result"]["content"][0]["text"].as_str().unwrap().contains("fn upload()"));
        assert_eq!(serve(&root.join("absent"), "".as_bytes(), Vec::new()), 66);
        fs::remove_dir_all(root.parent().unwrap()).unwrap();
    }
}
