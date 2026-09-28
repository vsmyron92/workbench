//! "Create devcontainer.json…": a starter config proposed from the detected stack.
//! It is only a proposal: the UI shows it in an editable preview, and it is written
//! into the repository only when the user confirms, never over an existing file.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use serde_json::{Value, json};

use crate::config::ProjectFile;
use crate::error::ApiError;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub path: String,
    pub content: String,
    /// Stacks found (`rust`, `node`…), primary first.
    pub stacks: Vec<String>,
    pub notes: Vec<String>,
    /// A config already exists there.
    pub exists: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Stack {
    Rust,
    Go,
    Java,
    Dotnet,
    Cpp,
    Python,
    Node,
}

impl Stack {
    fn name(self) -> &'static str {
        match self {
            Stack::Rust => "rust",
            Stack::Go => "go",
            Stack::Java => "java",
            Stack::Dotnet => "dotnet",
            Stack::Cpp => "cpp",
            Stack::Python => "python",
            Stack::Node => "node",
        }
    }
    fn image(self) -> &'static str {
        match self {
            Stack::Rust => "mcr.microsoft.com/devcontainers/rust:1-bookworm",
            Stack::Go => "mcr.microsoft.com/devcontainers/go:1-bookworm",
            Stack::Java => "mcr.microsoft.com/devcontainers/java:21-bookworm",
            Stack::Dotnet => "mcr.microsoft.com/devcontainers/dotnet:8.0-bookworm",
            Stack::Cpp => "mcr.microsoft.com/devcontainers/cpp:1-bookworm",
            Stack::Python => "mcr.microsoft.com/devcontainers/python:1-3.12-bookworm",
            Stack::Node => "mcr.microsoft.com/devcontainers/javascript-node:1-22-bookworm",
        }
    }
    /// The feature adding this stack to another stack's image.
    fn feature(self) -> Option<(&'static str, Value)> {
        Some(match self {
            Stack::Rust => ("ghcr.io/devcontainers/features/rust:1", json!({})),
            Stack::Go => ("ghcr.io/devcontainers/features/go:1", json!({})),
            Stack::Java => ("ghcr.io/devcontainers/features/java:1", json!({ "version": "21" })),
            Stack::Dotnet => ("ghcr.io/devcontainers/features/dotnet:2", json!({})),
            Stack::Python => ("ghcr.io/devcontainers/features/python:1", json!({ "version": "3.12" })),
            Stack::Node => ("ghcr.io/devcontainers/features/node:1", json!({ "version": "22" })),
            Stack::Cpp => return None,
        })
    }
}

const SKIP: &[&str] = &["node_modules", "target", ".git", ".venv", "venv", "dist", "build", "vendor", ".cache", "out", "bin", "obj"];

/// Marker files up to depth 3: `(stack, project-relative directory, install command)`.
fn scan(root: &Path) -> Vec<(Stack, String, Option<String>)> {
    let mut found = vec![];
    let mut stack: Vec<(std::path::PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    let mut visited = 0;
    while let Some((dir, depth)) = stack.pop() {
        visited += 1;
        if visited > 400 {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut names: Vec<(String, bool)> = rd
            .flatten()
            .filter_map(|e| Some((e.file_name().to_str()?.to_string(), e.file_type().ok()?.is_dir())))
            .collect();
        names.sort();
        let rel = dir.strip_prefix(root).map(|r| r.display().to_string()).unwrap_or_default();
        let has = |n: &str| names.iter().any(|(x, d)| x == n && !d);
        let cd = |cmd: &str| if rel.is_empty() { cmd.to_string() } else { format!("cd {} && {cmd}", super::sh_quote(&rel)) };
        // Crates below another Cargo.toml are its workspace members.
        let under = |r: &str| r.is_empty() || rel.starts_with(&format!("{r}/"));
        if has("Cargo.toml") && !found.iter().any(|(s, r, _): &(Stack, String, _)| *s == Stack::Rust && under(r)) {
            found.push((Stack::Rust, rel.clone(), Some(cd("cargo fetch"))));
        }
        if has("package.json") {
            let install = if has("pnpm-lock.yaml") {
                "pnpm install"
            } else if has("yarn.lock") {
                "yarn install"
            } else if has("bun.lockb") || has("bun.lock") {
                "bun install"
            } else if has("package-lock.json") {
                "npm ci"
            } else {
                "npm install"
            };
            found.push((Stack::Node, rel.clone(), Some(cd(install))));
        }
        if has("go.mod") {
            found.push((Stack::Go, rel.clone(), Some(cd("go mod download"))));
        }
        if has("pyproject.toml") || has("requirements.txt") || has("setup.py") || has("Pipfile") {
            let install = if has("uv.lock") {
                "uv sync"
            } else if has("poetry.lock") {
                "poetry install"
            } else if has("requirements.txt") {
                "pip install --user -r requirements.txt"
            } else {
                "pip install --user -e ."
            };
            found.push((Stack::Python, rel.clone(), Some(cd(install))));
        }
        if has("pom.xml") || has("build.gradle") || has("build.gradle.kts") || has("settings.gradle.kts") || has("settings.gradle") {
            found.push((Stack::Java, rel.clone(), None));
        }
        if names.iter().any(|(n, d)| !d && (n.ends_with(".sln") || n.ends_with(".csproj") || n.ends_with(".fsproj"))) {
            found.push((Stack::Dotnet, rel.clone(), Some(cd("dotnet restore"))));
        }
        if has("CMakeLists.txt") && rel.split('/').count() <= 1 {
            found.push((Stack::Cpp, rel.clone(), None));
        }
        if depth < 3 {
            for (n, is_dir) in names.iter().rev() {
                if *is_dir && !n.starts_with('.') && !SKIP.contains(&n.as_str()) {
                    stack.push((dir.join(n), depth + 1));
                }
            }
        }
    }
    // One install per directory and stack; nested Cargo members follow their workspace.
    found.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
    found
}

/// A JSON object that keeps the order keys were inserted in (the proposal reads like
/// the templates: name, image, features, ports, commands, user).
#[derive(Default)]
struct Ordered(Vec<(String, Value)>);

impl Ordered {
    fn insert(&mut self, k: String, v: Value) {
        self.0.push((k, v));
    }

    fn render(&self) -> String {
        let mut s = String::from("{\n");
        for (i, (k, v)) in self.0.iter().enumerate() {
            let val = serde_json::to_string_pretty(v).unwrap_or_else(|_| "null".into()).replace('\n', "\n  ");
            let comma = if i + 1 < self.0.len() { "," } else { "" };
            s.push_str(&format!("  {}: {val}{comma}\n", Value::String(k.clone())));
        }
        s.push('}');
        s
    }
}

/// Propose `.devcontainer/devcontainer.json` for the project at `root`.
pub fn propose(root: &Path, config: &ProjectFile) -> Proposal {
    let found = scan(root);
    let mut stacks: Vec<Stack> = found.iter().map(|(s, _, _)| *s).collect();
    stacks.sort();
    stacks.dedup();
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "project".into());
    let mut notes = vec![];
    let mut obj = Ordered::default();
    obj.insert("name".into(), json!(name));
    match stacks.first() {
        Some(primary) => {
            obj.insert("image".into(), json!(primary.image()));
            let mut features = serde_json::Map::new();
            for s in stacks.iter().skip(1) {
                match s.feature() {
                    Some((id, opts)) => {
                        features.insert(id.into(), opts);
                    }
                    None => notes.push(format!("{} is also used; add its tools to the image", s.name())),
                }
            }
            if !features.is_empty() {
                notes.push("Features are installed by the devcontainer CLI (npm i -g @devcontainers/cli, or [devcontainer] cli in config.toml).".into());
                obj.insert("features".into(), Value::Object(features));
            }
        }
        None => {
            obj.insert("image".into(), json!("mcr.microsoft.com/devcontainers/base:bookworm"));
            notes.push("No known stack was found: a plain Debian image.".into());
        }
    }
    // Ports of the project's servers.
    let mut ports: BTreeMap<u16, String> = BTreeMap::new();
    for r in &config.runs {
        if let Some(p) = r.port {
            if r.group.as_deref() != Some("deploy") && ports.len() < 8 {
                ports.entry(p).or_insert_with(|| r.name.clone());
            }
        }
    }
    if !ports.is_empty() {
        obj.insert("forwardPorts".into(), json!(ports.keys().collect::<Vec<_>>()));
        let attrs: serde_json::Map<String, Value> = ports.iter().map(|(p, n)| (p.to_string(), json!({ "label": n }))).collect();
        obj.insert("portsAttributes".into(), Value::Object(attrs));
    }
    let installs: Vec<(String, String)> = found
        .iter()
        .filter_map(|(s, rel, cmd)| cmd.clone().map(|c| (if rel.is_empty() { s.name().to_string() } else { rel.replace('/', "-") }, c)))
        .collect();
    match installs.len() {
        0 => {}
        1 => {
            obj.insert("postCreateCommand".into(), json!(installs[0].1));
        }
        _ => {
            let m: serde_json::Map<String, Value> = installs.into_iter().map(|(k, v)| (k, json!(v))).collect();
            obj.insert("postCreateCommand".into(), Value::Object(m));
        }
    }
    if stacks.first().is_some() {
        obj.insert("remoteUser".into(), json!("vscode"));
    }
    if config.components.iter().any(|c| c.kind == "compose") || root.join("docker-compose.yml").is_file() || root.join("compose.yaml").is_file() {
        notes.push("The project has a compose file: databases it defines are not part of this container (use dockerComposeFile to include them).".into());
    }
    let body = obj.render();
    let content = format!(
        "// Dev container for {name}, proposed by Workbench from the files in the repository.\n// Format: https://containers.dev/implementors/json_reference/\n{body}\n"
    );
    let path = ".devcontainer/devcontainer.json".to_string();
    Proposal {
        exists: root.join(&path).exists() || root.join(".devcontainer.json").exists(),
        path,
        content,
        stacks: stacks.iter().map(|s| s.name().to_string()).collect(),
        notes,
    }
}

/// Where a config may be written: `.devcontainer/devcontainer.json`,
/// `.devcontainer.json` or `.devcontainer/<name>/devcontainer.json`.
fn allowed_path(p: &str) -> bool {
    if p == ".devcontainer/devcontainer.json" || p == ".devcontainer.json" {
        return true;
    }
    let parts: Vec<&str> = p.split('/').collect();
    parts.len() == 3
        && parts[0] == ".devcontainer"
        && parts[2] == "devcontainer.json"
        && !parts[1].is_empty()
        && parts[1].chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
        && !parts[1].starts_with('.')
}

/// Write the confirmed config. Refuses to replace anything.
pub fn write(root: &Path, rel: &str, content: &str) -> Result<String, ApiError> {
    if !allowed_path(rel) {
        return Err(ApiError::bad_request("a devcontainer.json goes to .devcontainer/devcontainer.json, .devcontainer.json or .devcontainer/<name>/devcontainer.json"));
    }
    if content.len() > 256 * 1024 {
        return Err(ApiError::bad_request("the config is larger than 256 KB"));
    }
    match super::jsonc::parse(content) {
        Ok(v) if v.is_object() => {}
        Ok(_) => return Err(ApiError::bad_request("the config must be a JSON object")),
        Err(e) => return Err(ApiError::bad_request(format!("the config does not parse: {e}"))),
    }
    let abs = crate::util::paths::resolve_in_root(root, rel)?;
    if let Some(dir) = abs.parent() {
        std::fs::create_dir_all(dir)?;
        // The directory must still be inside the project (no symlink out).
        crate::util::paths::resolve_in_root(root, &dir.strip_prefix(root).map(|d| d.display().to_string()).unwrap_or_default())?;
    }
    use std::io::Write;
    let mut f = match std::fs::OpenOptions::new().write(true).create_new(true).open(&abs) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Err(ApiError::conflict(format!("{rel} already exists; Workbench never replaces it"))),
        Err(e) => return Err(e.into()),
    };
    f.write_all(content.as_bytes())?;
    Ok(rel.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_and_node_get_a_rust_image_and_the_node_feature() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        std::fs::create_dir_all(r.join("app/server/crates/core")).unwrap();
        std::fs::create_dir_all(r.join("app/web")).unwrap();
        std::fs::write(r.join("app/server/Cargo.toml"), "[workspace]\n").unwrap();
        std::fs::write(r.join("app/server/crates/core/Cargo.toml"), "[package]\n").unwrap();
        std::fs::write(r.join("app/web/package.json"), "{}").unwrap();
        std::fs::write(r.join("app/web/package-lock.json"), "{}").unwrap();
        let mut pf = ProjectFile::default();
        pf.runs.push(crate::config::project::RunConfig { name: "web".into(), port: Some(5173), ..Default::default() });
        pf.runs.push(crate::config::project::RunConfig { name: "api".into(), port: Some(8080), ..Default::default() });
        let p = propose(r, &pf);
        assert_eq!(p.stacks, vec!["rust", "node"]);
        let v = super::super::jsonc::parse(&p.content).unwrap();
        assert_eq!(v["image"], "mcr.microsoft.com/devcontainers/rust:1-bookworm");
        assert!(v["features"]["ghcr.io/devcontainers/features/node:1"].is_object());
        assert_eq!(v["forwardPorts"], json!([5173, 8080]));
        assert_eq!(v["portsAttributes"]["8080"]["label"], "api");
        assert_eq!(v["postCreateCommand"]["app-server"], "cd app/server && cargo fetch");
        assert_eq!(v["postCreateCommand"]["app-web"], "cd app/web && npm ci");
        assert!(v["postCreateCommand"].get("app-server-crates-core").is_none(), "{}", p.content);
        assert!(!p.exists);
    }

    #[test]
    fn writes_only_new_files_in_allowed_places() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        assert!(write(r, "Dockerfile", "{}").is_err());
        assert!(write(r, ".devcontainer/../x.json", "{}").is_err());
        assert!(write(r, ".devcontainer/devcontainer.json", "not json").is_err());
        assert_eq!(write(r, ".devcontainer/devcontainer.json", "// c\n{\"image\": \"x\",}").unwrap(), ".devcontainer/devcontainer.json");
        let e = write(r, ".devcontainer/devcontainer.json", "{}").unwrap_err();
        assert_eq!(e.code, "conflict");
        assert!(std::fs::read_to_string(r.join(".devcontainer/devcontainer.json")).unwrap().contains("\"x\""));
        assert!(write(r, ".devcontainer/py/devcontainer.json", "{}").is_ok());
    }
}
