//! URI mapping between the three places a file has a name:
//!
//! * **the browser**: Monaco model URIs. A project file is `file:///<projectId>/<rel>`
//!   (the files slice's models); a file outside the project that a language server
//!   pointed to (the Rust standard library, `~/.cargo/registry`, `node_modules`,
//!   site-packages) is `lsp-src://<projectId>/<absolute path on the server's side>`
//!   and is shown by the lsp slice's own read-only panel;
//! * **the host**: absolute paths;
//! * **the language server**: `file://` URIs of absolute paths on its side, which in a
//!   dev container are container paths (the workspace mount maps the project root).
//!
//! Only the URI fields of a message are mapped (`rewrite`): text that happens to be a
//! `file://` URI (a completion for a string literal, a message) is left as it is. Only
//! `lsp-src` URIs this project's server returned in such fields may be read back
//! (`AllowSet`), so the source route is not a way to read arbitrary files.

use std::collections::{HashMap, VecDeque};
use std::path::{Component, Path, PathBuf};

use serde_json::{Map, Value};

pub const SOURCE_SCHEME: &str = "lsp-src";

/// Characters kept as they are in a URI path (RFC 3986 unreserved, plus `/`).
fn keep(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/')
}

/// Percent-encode a path for a URI (uppercase hex, like VS Code and Monaco).
pub fn encode_path(p: &str) -> String {
    let mut out = String::with_capacity(p.len() + 8);
    for &b in p.as_bytes() {
        if keep(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn decode(s: &str) -> Option<String> {
    percent_encoding::percent_decode_str(s).decode_utf8().ok().map(|c| c.into_owned())
}

/// `file://` URI of an absolute path.
pub fn file_uri(path: &str) -> String {
    format!("file://{}", encode_path(path))
}

/// The absolute path of a `file://` URI (empty or `localhost` authority).
pub fn parse_file_uri(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let p = decode(rest)?;
    // Windows drive letters (`/c:/…`) never reach us on Linux; anything else absolute is fine.
    (!p.contains('\0')).then_some(p)
}

/// Lexically normalize a relative path: no `..` above the start, no absolute parts.
pub fn clean_rel(rel: &str) -> Option<String> {
    let mut parts: Vec<&str> = vec![];
    for c in Path::new(rel).components() {
        match c {
            Component::Normal(s) => parts.push(s.to_str()?),
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(parts.join("/"))
}

/// A browser URI, parsed.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientUri {
    /// `file:///<pid>/<rel>`: a project file.
    Project { pid: String, rel: String },
    /// `lsp-src://<pid>/<abs>`: a file on the server's side outside the project.
    Source { pid: String, path: String },
}

pub fn parse_client_uri(uri: &str) -> Option<ClientUri> {
    if let Some(rest) = uri.strip_prefix("file:///") {
        let rest = decode(rest.split(['?', '#']).next().unwrap_or(rest))?;
        let (pid, rel) = rest.split_once('/')?;
        if pid.is_empty() || pid.starts_with('~') {
            return None;
        }
        return Some(ClientUri::Project { pid: pid.to_string(), rel: clean_rel(rel)? });
    }
    let rest = uri.strip_prefix(SOURCE_SCHEME)?.strip_prefix("://")?;
    let slash = rest.find('/')?;
    let (pid, path) = rest.split_at(slash);
    let path = decode(path.split(['?', '#']).next().unwrap_or(path))?;
    if pid.is_empty() || path.contains('\0') {
        return None;
    }
    Some(ClientUri::Source { pid: pid.to_string(), path })
}

pub fn project_uri(pid: &str, rel: &str) -> String {
    format!("file:///{}/{}", encode_path(pid), encode_path(rel))
}

pub fn source_uri(pid: &str, path: &str) -> String {
    format!("{SOURCE_SCHEME}://{}{}", encode_path(pid), encode_path(path))
}

/// Where the language server runs, for path mapping.
#[derive(Debug, Clone, PartialEq)]
pub enum Side {
    Host,
    /// In the dev container: host directory ↔ container directory of the workspace mount.
    Container { host: PathBuf, container: String },
}

/// How a file outside the project is read back for the source panel.
#[derive(Debug, Clone, PartialEq)]
pub enum Origin {
    Host,
    Container { docker: String, container_id: String, user: Option<String> },
}

/// Host ↔ server path mapping for one server.
#[derive(Debug, Clone)]
pub struct PathMap {
    pub side: Side,
}

impl PathMap {
    pub fn host() -> Self {
        Self { side: Side::Host }
    }

    /// The server-side path of a host path (`None`: not reachable from the container).
    pub fn to_server(&self, host: &Path) -> Option<String> {
        match &self.side {
            Side::Host => Some(host.to_string_lossy().into_owned()),
            Side::Container { host: src, container } => {
                let rel = host.strip_prefix(src).ok()?.to_string_lossy().into_owned();
                Some(if rel.is_empty() { container.clone() } else { format!("{}/{rel}", container.trim_end_matches('/')) })
            }
        }
    }

    /// The host path of a server-side path (`None`: only exists in the container).
    pub fn to_host(&self, server: &str) -> Option<PathBuf> {
        match &self.side {
            Side::Host => Some(PathBuf::from(server)),
            Side::Container { host, container } => {
                let c = container.trim_end_matches('/');
                if server == c {
                    return Some(host.clone());
                }
                let rel = server.strip_prefix(c)?.strip_prefix('/')?;
                Some(host.join(clean_rel(rel)?))
            }
        }
    }
}

/// `lsp-src` paths this project's servers returned, bounded (oldest forgotten first).
#[derive(Debug)]
pub struct AllowSet {
    map: HashMap<String, Origin>,
    order: VecDeque<String>,
    cap: usize,
}

impl Default for AllowSet {
    fn default() -> Self {
        Self::new(50_000)
    }
}

impl AllowSet {
    pub fn new(cap: usize) -> Self {
        Self { map: HashMap::new(), order: VecDeque::new(), cap }
    }

    pub fn insert(&mut self, path: &str, origin: Origin) {
        if let Some(o) = self.map.get_mut(path) {
            *o = origin;
            return;
        }
        if self.map.len() >= self.cap {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
        self.map.insert(path.to_string(), origin);
        self.order.push_back(path.to_string());
    }

    pub fn get(&self, path: &str) -> Option<&Origin> {
        self.map.get(path)
    }
}

/// Maps URIs between the browser and one language server of a project.
pub struct Translator<'a> {
    pub pid: &'a str,
    /// The project root, and its canonical form (servers may resolve symlinks).
    pub root: &'a Path,
    pub root_canon: &'a Path,
    pub map: &'a PathMap,
    pub origin: &'a Origin,
}

impl Translator<'_> {
    /// Browser URI → server URI. `allowed` answers whether an `lsp-src` path was
    /// handed out before.
    pub fn to_server(&self, uri: &str, allowed: &dyn Fn(&str) -> bool) -> Option<String> {
        match parse_client_uri(uri)? {
            ClientUri::Project { pid, rel } if pid == self.pid => {
                let host = if rel.is_empty() { self.root.to_path_buf() } else { self.root.join(&rel) };
                Some(file_uri(&self.map.to_server(&host)?))
            }
            ClientUri::Source { pid, path } if pid == self.pid && allowed(&path) => Some(file_uri(&path)),
            _ => None,
        }
    }

    /// Server URI → browser URI. Paths outside the project are recorded in `allow`.
    pub fn to_client(&self, uri: &str, allow: &mut AllowSet) -> Option<String> {
        let server_path = parse_file_uri(uri)?;
        if let Some(host) = self.map.to_host(&server_path) {
            for root in [self.root, self.root_canon] {
                if let Ok(rel) = host.strip_prefix(root) {
                    let rel = rel.to_string_lossy();
                    if let Some(rel) = clean_rel(&rel) {
                        return Some(project_uri(self.pid, &rel));
                    }
                }
            }
            if matches!(self.map.side, Side::Host) {
                allow.insert(&server_path, Origin::Host);
                return Some(source_uri(self.pid, &server_path));
            }
        }
        allow.insert(&server_path, self.origin.clone());
        Some(source_uri(self.pid, &server_path))
    }
}

/// The LSP fields whose string value is a document URI: `Location.uri`,
/// `LocationLink.targetUri`, `TextDocumentIdentifier.uri`, `PublishDiagnosticsParams.uri`,
/// `CreateFile`/`DeleteFile.uri`, `RenameFile.oldUri`/`newUri`, `RelativePattern.baseUri`,
/// `WorkspaceSymbol.location.uri`, `CallHierarchyItem.uri`…
const URI_KEYS: &[&str] = &["uri", "targetUri", "oldUri", "newUri", "baseUri"];

/// Rewrite the document URIs in an LSP message: the string values of URI fields
/// (`URI_KEYS`) and the keys of `WorkspaceEdit.changes`, when they start with one of
/// `schemes`. Nothing else is touched: a completion label, an edit's `newText`, a hover
/// or a diagnostic message that happens to be a `file://` URI is text, and `data` is the
/// server's own, handed back verbatim in resolve requests. Strings `f` cannot map stay.
pub fn rewrite(v: &mut Value, schemes: &[&str], f: &mut dyn FnMut(&str) -> Option<String>) {
    let has_scheme = |s: &str| schemes.iter().any(|p| s.starts_with(p));
    match v {
        Value::Array(a) => a.iter_mut().for_each(|x| rewrite(x, schemes, f)),
        Value::Object(o) => {
            for (k, val) in o.iter_mut() {
                match (k.as_str(), val) {
                    ("data", _) => {}
                    (k, Value::String(s)) if URI_KEYS.contains(&k) => {
                        if has_scheme(s) {
                            if let Some(n) = f(s) {
                                *s = n;
                            }
                        }
                    }
                    ("changes", Value::Object(by_uri)) => {
                        let old = std::mem::take(by_uri);
                        let mut new = Map::with_capacity(old.len());
                        for (u, mut edits) in old {
                            rewrite(&mut edits, schemes, f);
                            let u = if has_scheme(&u) { f(&u).unwrap_or(u) } else { u };
                            new.insert(u, edits);
                        }
                        *by_uri = new;
                    }
                    (_, val) => rewrite(val, schemes, f),
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn file_uris_encode_and_decode() {
        assert_eq!(file_uri("/home/u/my proj/a#b.rs"), "file:///home/u/my%20proj/a%23b.rs");
        assert_eq!(parse_file_uri("file:///home/u/my%20proj/a%23b.rs").unwrap(), "/home/u/my proj/a#b.rs");
        assert_eq!(parse_file_uri("file://localhost/x/y").unwrap(), "/x/y");
        assert_eq!(parse_file_uri("file:///x/%C3%A9.rs").unwrap(), "/x/é.rs");
        assert_eq!(parse_file_uri("file:///x/%3A.rs").unwrap(), "/x/:.rs");
        assert_eq!(parse_file_uri("file:///x/y?q=1").unwrap(), "/x/y");
        assert!(parse_file_uri("file://server/share").is_none());
        assert!(parse_file_uri("https://x/y").is_none());
        assert!(parse_file_uri("file:///x/%00").is_none());
    }

    #[test]
    fn client_uris_parse_and_refuse_escapes() {
        assert_eq!(
            parse_client_uri("file:///api/src/main.rs"),
            Some(ClientUri::Project { pid: "api".into(), rel: "src/main.rs".into() })
        );
        // Monaco encodes like we do.
        assert_eq!(
            parse_client_uri("file:///api/dir%20x/%C3%A9.ts"),
            Some(ClientUri::Project { pid: "api".into(), rel: "dir x/é.ts".into() })
        );
        assert_eq!(parse_client_uri("file:///api/a/../b.rs"), Some(ClientUri::Project { pid: "api".into(), rel: "b.rs".into() }));
        assert_eq!(parse_client_uri("file:///api/../etc/passwd"), None);
        assert_eq!(parse_client_uri("file:///~abs/etc/passwd"), None);
        assert_eq!(
            parse_client_uri("lsp-src://api/home/u/.cargo/registry/x.rs"),
            Some(ClientUri::Source { pid: "api".into(), path: "/home/u/.cargo/registry/x.rs".into() })
        );
        assert_eq!(parse_client_uri("lsp-src://api"), None);
        assert_eq!(project_uri("api", "dir x/é.ts"), "file:///api/dir%20x/%C3%A9.ts");
        assert_eq!(source_uri("api", "/usr/lib/x y.rs"), "lsp-src://api/usr/lib/x%20y.rs");
    }

    fn host_tr<'a>(root: &'a Path, map: &'a PathMap) -> Translator<'a> {
        Translator { pid: "api", root, root_canon: root, map, origin: &Origin::Host }
    }

    #[test]
    fn host_servers_see_host_paths() {
        let root = PathBuf::from("/home/u/ws/api");
        let map = PathMap::host();
        let tr = host_tr(&root, &map);
        let mut allow = AllowSet::default();
        let none = |_: &str| false;
        assert_eq!(tr.to_server("file:///api/src/main.rs", &none).unwrap(), "file:///home/u/ws/api/src/main.rs");
        // Another project's model is not this server's business.
        assert_eq!(tr.to_server("file:///web/src/main.ts", &none), None);
        assert_eq!(tr.to_client("file:///home/u/ws/api/src/lib.rs", &mut allow).unwrap(), "file:///api/src/lib.rs");
        assert_eq!(tr.to_client("file:///home/u/ws/api", &mut allow).unwrap(), "file:///api/");
        // Outside: a source URI, and now allowed.
        let std = "file:///home/u/.rustup/toolchains/stable/lib/rustlib/src/rust/library/core/src/option.rs";
        let c = tr.to_client(std, &mut allow).unwrap();
        assert_eq!(c, "lsp-src://api/home/u/.rustup/toolchains/stable/lib/rustlib/src/rust/library/core/src/option.rs");
        assert_eq!(allow.get("/home/u/.rustup/toolchains/stable/lib/rustlib/src/rust/library/core/src/option.rs"), Some(&Origin::Host));
        let allowed = |p: &str| allow.get(p).is_some();
        assert_eq!(tr.to_server(&c, &allowed).unwrap(), std);
        // A source URI nobody handed out is refused.
        assert_eq!(tr.to_server("lsp-src://api/etc/shadow", &allowed), None);
        // A sibling directory with the root as a prefix is outside.
        assert!(tr.to_client("file:///home/u/ws/api-2/x.rs", &mut allow).unwrap().starts_with("lsp-src://"));
    }

    #[test]
    fn container_servers_see_container_paths_both_ways() {
        let root = PathBuf::from("/home/u/ws/api");
        let map = PathMap { side: Side::Container { host: PathBuf::from("/home/u/ws/api"), container: "/workspaces/api".into() } };
        let origin = Origin::Container { docker: "docker".into(), container_id: "abc".into(), user: Some("vscode".into()) };
        let tr = Translator { pid: "api", root: &root, root_canon: &root, map: &map, origin: &origin };
        let mut allow = AllowSet::default();
        let none = |_: &str| false;
        assert_eq!(tr.to_server("file:///api/src/main.rs", &none).unwrap(), "file:///workspaces/api/src/main.rs");
        assert_eq!(tr.to_server("file:///api/", &none).unwrap(), "file:///workspaces/api");
        assert_eq!(tr.to_client("file:///workspaces/api/src/lib.rs", &mut allow).unwrap(), "file:///api/src/lib.rs");
        assert!(tr.to_client("file:///workspaces/api/../etc/x", &mut allow).unwrap().starts_with("lsp-src://"));
        // Container-only files (the toolchain inside) are sources read from the container.
        let c = tr.to_client("file:///usr/local/rustup/lib/core/src/option.rs", &mut allow).unwrap();
        assert_eq!(c, "lsp-src://api/usr/local/rustup/lib/core/src/option.rs");
        assert_eq!(allow.get("/usr/local/rustup/lib/core/src/option.rs"), Some(&origin));
        // A mount of a parent directory: the project is a subfolder inside.
        let map = PathMap { side: Side::Container { host: PathBuf::from("/home/u/ws"), container: "/w/".into() } };
        assert_eq!(map.to_server(Path::new("/home/u/ws/api/x.rs")).unwrap(), "/w/api/x.rs");
        assert_eq!(map.to_host("/w/api/x.rs").unwrap(), PathBuf::from("/home/u/ws/api/x.rs"));
        assert_eq!(map.to_host("/w"), Some(PathBuf::from("/home/u/ws")));
        assert_eq!(map.to_host("/wx/y"), None);
        assert_eq!(map.to_server(Path::new("/elsewhere/x")), None);
    }

    fn map_a_to_p(v: &mut Value) -> Vec<String> {
        let mut seen = vec![];
        rewrite(v, &["file://", "lsp-src://"], &mut |s| {
            seen.push(s.to_string());
            s.strip_prefix("file:///a/").map(|r| format!("file:///p/{r}"))
        });
        seen
    }

    #[test]
    fn rewrite_maps_uri_fields_and_edit_keys() {
        let mut v = json!({
            "changes": { "file:///a/x.rs": [{ "range": {}, "newText": "file:///a/in-new-text" }] },
            "documentChanges": [
                { "textDocument": { "uri": "file:///a/y.rs", "version": 3 }, "edits": [] },
                { "kind": "rename", "oldUri": "file:///a/o.rs", "newUri": "file:///a/n.rs" },
            ],
            "links": [{ "targetUri": "file:///a/t.rs", "targetRange": {} }],
            "watchers": [{ "globPattern": { "baseUri": "file:///a/src", "pattern": "**/*.rs" } }],
            "diags": [{ "message": "m", "relatedInformation": [{ "location": { "uri": "file:///a/r.rs" }, "message": "file:///a/msg" }] }],
            "other": "text file:///a/z.rs",
            "unmappable": { "uri": "file:///elsewhere/q.rs" },
        });
        let seen = map_a_to_p(&mut v);
        assert!(v["changes"].get("file:///p/x.rs").is_some(), "{v}");
        assert_eq!(v["documentChanges"][0]["textDocument"]["uri"], "file:///p/y.rs");
        assert_eq!((v["documentChanges"][1]["oldUri"].as_str(), v["documentChanges"][1]["newUri"].as_str()), (Some("file:///p/o.rs"), Some("file:///p/n.rs")));
        assert_eq!(v["links"][0]["targetUri"], "file:///p/t.rs");
        assert_eq!(v["watchers"][0]["globPattern"]["baseUri"], "file:///p/src");
        assert_eq!(v["diags"][0]["relatedInformation"][0]["location"]["uri"], "file:///p/r.rs");
        assert_eq!(v["unmappable"]["uri"], "file:///elsewhere/q.rs");
        // Text is never a URI, even when it is exactly one.
        assert_eq!(v["changes"]["file:///p/x.rs"][0]["newText"], "file:///a/in-new-text");
        assert_eq!(v["diags"][0]["relatedInformation"][0]["message"], "file:///a/msg");
        assert_eq!(v["other"], "text file:///a/z.rs");
        assert!(!seen.iter().any(|s| s.contains("in-new-text") || s.contains("msg")), "{seen:?}");
    }

    #[test]
    fn rewrite_leaves_text_and_data_alone() {
        // A TypeScript completion for a string literal type `"file:///etc/hostname"`.
        let lit = "file:///a/etc/hostname";
        let mut completion = json!({ "isIncomplete": false, "items": [{
            "label": lit, "detail": lit, "filterText": lit, "sortText": lit, "insertText": lit,
            "labelDetails": { "description": lit },
            "textEdit": { "range": {}, "newText": lit },
            "additionalTextEdits": [{ "range": {}, "newText": lit }],
            "documentation": { "kind": "markdown", "value": lit },
            "command": { "title": lit, "command": "x", "arguments": [lit] },
            "data": { "uri": "file:///a/main.ts", "textDocument": { "uri": "file:///a/main.ts" } },
        }] });
        let before = completion.clone();
        assert!(map_a_to_p(&mut completion).is_empty());
        assert_eq!(completion, before);
        let mut hover = json!({ "contents": { "kind": "markdown", "value": lit } });
        let mut sig = json!({ "signatures": [{ "label": lit, "parameters": [{ "label": lit }] }] });
        let mut bare = json!([lit, { "value": lit }]);
        for v in [&mut hover, &mut sig, &mut bare] {
            let before = v.clone();
            assert!(map_a_to_p(v).is_empty());
            assert_eq!(*v, before);
        }
        // A diagnostic: its data stays the server's, its location is mapped.
        let mut d = json!([{ "message": lit, "data": { "uri": "file:///a/x" }, "relatedInformation": [{ "location": { "uri": "file:///a/y" }, "message": lit }] }]);
        map_a_to_p(&mut d);
        assert_eq!(d[0]["message"], lit);
        assert_eq!(d[0]["data"]["uri"], "file:///a/x");
        assert_eq!(d[0]["relatedInformation"][0]["location"]["uri"], "file:///p/y");
        // The browser → server direction is the same walk: a query or a new name is text.
        let mut params = json!({ "query": "file:///a/q", "newName": "lsp-src://a/x", "textDocument": { "uri": "file:///a/doc" } });
        map_a_to_p(&mut params);
        assert_eq!(params, json!({ "query": "file:///a/q", "newName": "lsp-src://a/x", "textDocument": { "uri": "file:///p/doc" } }));
    }

    #[test]
    fn only_uri_fields_enter_the_allow_set() {
        let root = PathBuf::from("/home/u/ws/api");
        let map = PathMap::host();
        let tr = host_tr(&root, &map);
        let mut allow = AllowSet::default();
        let mut v = json!({ "items": [{ "label": "file:///etc/hostname", "textEdit": { "newText": "file:///etc/hostname" }, "data": { "uri": "file:///etc/passwd" } }] });
        rewrite(&mut v, &["file://"], &mut |s| tr.to_client(s, &mut allow));
        assert_eq!(v["items"][0]["label"], "file:///etc/hostname");
        assert!(allow.get("/etc/hostname").is_none() && allow.get("/etc/passwd").is_none());
        let mut v = json!([{ "uri": "file:///usr/lib/x.rs", "range": {} }]);
        rewrite(&mut v, &["file://"], &mut |s| tr.to_client(s, &mut allow));
        assert_eq!(v[0]["uri"], "lsp-src://api/usr/lib/x.rs");
        assert!(allow.get("/usr/lib/x.rs").is_some());
    }

    #[test]
    fn allow_set_is_bounded() {
        let mut a = AllowSet::new(2);
        a.insert("/a", Origin::Host);
        a.insert("/b", Origin::Host);
        a.insert("/a", Origin::Host);
        a.insert("/c", Origin::Host);
        assert!(a.get("/a").is_none() && a.get("/b").is_some() && a.get("/c").is_some());
    }
}
