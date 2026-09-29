//! `[lsp]` in config.toml: built-in language server presets, overrides and servers of
//! your own, and where each command is found.
//!
//! Server commands come only from here (config.toml), never from repository config:
//! a project's `.workbench.toml` has no `[lsp]` and cannot add, change or enable one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::util::os::exe;

/// `[lsp]` in config.toml.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LspConfig {
    /// Minutes a server keeps running with no open document and no request (default 10).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idle_minutes: Option<u64>,
    /// Overrides of the built-in presets (`[lsp.servers.rust-analyzer] command = …`) and
    /// servers of your own (`[lsp.servers.zls] command = "zls", extensions = ["zig"]`).
    /// Must stay the last field: TOML tables follow plain values.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub servers: BTreeMap<String, ServerConfig>,
}

/// `[lsp.servers.<id>]`. Every field is optional for a preset; a server of your own
/// needs `command` and `languages` or `extensions`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ServerConfig {
    /// The executable (on PATH, absolute or `~/…`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// Monaco language ids the server handles (`rust`, `typescript`, `python`…).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub languages: Option<Vec<String>>,
    /// File extensions without the dot (`rs`, `tsx`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Vec<String>>,
    /// Files whose presence marks a project the server is for (`Cargo.toml`, `*.csproj`).
    #[serde(skip_serializing_if = "Option::is_none", alias = "rootMarkers")]
    pub root_markers: Option<Vec<String>>,
    /// `false` turns the server (or a preset) off everywhere.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// How to install the command, shown while it is missing.
    #[serde(skip_serializing_if = "Option::is_none", alias = "installHint")]
    pub install_hint: Option<String>,
    /// Plain environment values for the process (`~/` is expanded). No secrets here.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// `initializationOptions` sent with `initialize` (a TOML table, as JSON).
    #[serde(skip_serializing_if = "Option::is_none", alias = "initializationOptions")]
    pub initialization_options: Option<Value>,
    /// Answers to `workspace/configuration` (`{ "rust-analyzer" = { check = { command = "clippy" } } }`),
    /// also sent with `workspace/didChangeConfiguration`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<Value>,
}

/// A server as Workbench runs it: a preset merged with its override, or a server of
/// the user's own.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSpec {
    pub id: String,
    pub label: String,
    pub command: String,
    pub args: Vec<String>,
    pub languages: Vec<String>,
    /// Lowercase, without the dot.
    pub extensions: Vec<String>,
    pub root_markers: Vec<String>,
    pub enabled: bool,
    pub install_hint: String,
    /// A built-in preset (possibly overridden).
    pub preset: bool,
    #[serde(skip)]
    pub env: BTreeMap<String, String>,
    #[serde(skip)]
    pub initialization_options: Option<Value>,
    #[serde(skip)]
    pub settings: Option<Value>,
    /// The rustup component that provides the command (rust-analyzer): a rustup proxy
    /// on PATH counts only when `rustup which` finds the component.
    #[serde(skip)]
    pub rustup_component: Option<String>,
}

impl ServerSpec {
    /// Whether this server handles a file (by extension, else by Monaco language id).
    pub fn handles(&self, ext: &str, language: Option<&str>) -> bool {
        (!ext.is_empty() && self.extensions.iter().any(|e| e == ext))
            || language.is_some_and(|l| !l.is_empty() && l != "plaintext" && self.languages.iter().any(|x| x == l))
    }
}

/// Server ids appear in URLs, events and file names.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 48
        && id.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_' | b'.'))
}

struct Preset {
    id: &'static str,
    label: &'static str,
    command: &'static str,
    args: &'static [&'static str],
    languages: &'static [&'static str],
    extensions: &'static [&'static str],
    root_markers: &'static [&'static str],
    install_hint: &'static str,
    rustup_component: Option<&'static str>,
}

/// Built-in presets, in preference order: the first enabled and installed server that
/// handles a file serves it (pyright before basedpyright before pylsp).
const PRESETS: &[Preset] = &[
    Preset {
        id: "rust-analyzer",
        label: "rust-analyzer",
        command: "rust-analyzer",
        args: &[],
        languages: &["rust"],
        extensions: &["rs"],
        root_markers: &["Cargo.toml", "rust-project.json"],
        install_hint: "rustup component add rust-analyzer (or the release binary from github.com/rust-lang/rust-analyzer/releases on PATH)",
        rustup_component: Some("rust-analyzer"),
    },
    Preset {
        id: "typescript",
        label: "TypeScript",
        command: "typescript-language-server",
        args: &["--stdio"],
        languages: &["typescript", "javascript", "typescriptreact", "javascriptreact"],
        extensions: &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"],
        root_markers: &["tsconfig.json", "jsconfig.json", "package.json"],
        install_hint: "npm install -g typescript typescript-language-server",
        rustup_component: None,
    },
    Preset {
        id: "pyright",
        label: "Pyright",
        command: "pyright-langserver",
        args: &["--stdio"],
        languages: &["python"],
        extensions: &["py", "pyi"],
        root_markers: &["pyproject.toml", "setup.py", "setup.cfg", "requirements.txt", "pyrightconfig.json", "Pipfile"],
        install_hint: "npm install -g pyright (or pip install pyright)",
        rustup_component: None,
    },
    Preset {
        id: "basedpyright",
        label: "basedpyright",
        command: "basedpyright-langserver",
        args: &["--stdio"],
        languages: &["python"],
        extensions: &["py", "pyi"],
        root_markers: &["pyproject.toml", "setup.py", "setup.cfg", "requirements.txt", "pyrightconfig.json", "Pipfile"],
        install_hint: "pip install basedpyright",
        rustup_component: None,
    },
    Preset {
        id: "pylsp",
        label: "Python LSP Server",
        command: "pylsp",
        args: &[],
        languages: &["python"],
        extensions: &["py", "pyi"],
        root_markers: &["pyproject.toml", "setup.py", "setup.cfg", "requirements.txt", "Pipfile"],
        install_hint: "pip install python-lsp-server",
        rustup_component: None,
    },
    Preset {
        id: "gopls",
        label: "gopls",
        command: "gopls",
        args: &[],
        languages: &["go"],
        extensions: &["go"],
        root_markers: &["go.mod", "go.work"],
        install_hint: "go install golang.org/x/tools/gopls@latest",
        rustup_component: None,
    },
    Preset {
        id: "clangd",
        label: "clangd",
        command: "clangd",
        args: &[],
        languages: &["c", "cpp"],
        extensions: &[
            "c", "h", "cc", "cpp", "cxx", "c++", "hpp", "hh", "hxx", "h++", "ipp", "tpp", "txx", "inl", "ixx", "cppm", "m", "mm", "cu", "cuh",
        ],
        root_markers: &["compile_commands.json", "CMakeLists.txt", ".clangd", "compile_flags.txt", "meson.build"],
        install_hint: "install clangd from your distribution (apt install clangd) or LLVM",
        rustup_component: None,
    },
    Preset {
        id: "verible",
        label: "Verible (SystemVerilog)",
        command: "verible-verilog-ls",
        // Lint with the project's `.rules.verible_lint` (a list of rules) where it has one.
        args: &["--rules_config_search"],
        languages: &["verilog", "systemverilog"],
        extensions: &["v", "vh", "sv", "svh"],
        root_markers: &["verible.filelist", ".rules.verible_lint"],
        install_hint: "download verible from github.com/chipsalliance/verible/releases and put its bin/ on PATH",
        rustup_component: None,
    },
    Preset {
        id: "vhdl_ls",
        label: "VHDL (vhdl_ls)",
        command: "vhdl_ls",
        args: &[],
        languages: &["vhdl"],
        extensions: &["vhd", "vhdl", "vho", "vht"],
        root_markers: &["vhdl_ls.toml"],
        // vhdl_ls stops when it finds no IEEE libraries next to its binary, so `cargo install` alone is not enough.
        install_hint: "download vhdl_ls from github.com/VHDL-LS/rust_hdl/releases and put its bin/ on PATH, keeping vhdl_libraries/ next to bin/",
        rustup_component: None,
    },
    Preset {
        id: "bash",
        label: "Bash",
        command: "bash-language-server",
        args: &["start"],
        languages: &["shell"],
        extensions: &["sh", "bash"],
        root_markers: &[],
        install_hint: "npm install -g bash-language-server",
        rustup_component: None,
    },
    Preset {
        id: "yaml",
        label: "YAML",
        command: "yaml-language-server",
        args: &["--stdio"],
        languages: &["yaml"],
        extensions: &["yml", "yaml"],
        root_markers: &[],
        install_hint: "npm install -g yaml-language-server",
        rustup_component: None,
    },
    Preset {
        id: "json",
        label: "JSON",
        command: "vscode-json-language-server",
        args: &["--stdio"],
        languages: &["json"],
        extensions: &["json", "jsonc"],
        root_markers: &[],
        install_hint: "npm install -g vscode-langservers-extracted",
        rustup_component: None,
    },
    Preset {
        id: "taplo",
        label: "Taplo (TOML)",
        command: "taplo",
        args: &["lsp", "stdio"],
        languages: &["toml"],
        extensions: &["toml"],
        root_markers: &[],
        install_hint: "cargo install taplo-cli --locked --features lsp",
        rustup_component: None,
    },
    Preset {
        id: "marksman",
        label: "Marksman (Markdown)",
        command: "marksman",
        args: &["server"],
        languages: &["markdown"],
        extensions: &["md", "markdown"],
        root_markers: &[".marksman.toml"],
        install_hint: "download marksman from github.com/artempyanykh/marksman/releases",
        rustup_component: None,
    },
    Preset {
        id: "csharp-ls",
        label: "C# (csharp-ls)",
        command: "csharp-ls",
        args: &[],
        languages: &["csharp"],
        extensions: &["cs"],
        root_markers: &["*.sln", "*.csproj"],
        install_hint: "dotnet tool install --global csharp-ls",
        rustup_component: None,
    },
    Preset {
        id: "lua",
        label: "Lua",
        command: "lua-language-server",
        args: &[],
        languages: &["lua"],
        extensions: &["lua"],
        root_markers: &[".luarc.json", ".luarc.jsonc"],
        install_hint: "install lua-language-server from your distribution or github.com/LuaLS/lua-language-server/releases",
        rustup_component: None,
    },
];

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn norm_exts(v: Vec<String>) -> Vec<String> {
    v.into_iter().map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase()).filter(|e| !e.is_empty()).collect()
}

impl LspConfig {
    /// Every server Workbench knows, in preference order: servers of the user's own
    /// first, then the presets. A server of the user's own without a command or
    /// without languages/extensions is skipped with a warning.
    pub fn specs(&self) -> (Vec<ServerSpec>, Vec<String>) {
        let mut out = vec![];
        let mut warnings = vec![];
        for (id, c) in &self.servers {
            if PRESETS.iter().any(|p| p.id == id) {
                continue;
            }
            if !valid_id(id) {
                warnings.push(format!("[lsp.servers.{id}]: ids are lowercase letters, digits, '-', '_' and '.'"));
                continue;
            }
            let Some(command) = c.command.clone().filter(|c| !c.trim().is_empty()) else {
                warnings.push(format!("[lsp.servers.{id}] needs a command"));
                continue;
            };
            let languages = c.languages.clone().unwrap_or_default();
            let extensions = norm_exts(c.extensions.clone().unwrap_or_default());
            if languages.is_empty() && extensions.is_empty() {
                warnings.push(format!("[lsp.servers.{id}] needs languages or extensions"));
                continue;
            }
            out.push(ServerSpec {
                id: id.clone(),
                label: c.label.clone().unwrap_or_else(|| id.clone()),
                command,
                args: c.args.clone().unwrap_or_default(),
                languages,
                extensions,
                root_markers: c.root_markers.clone().unwrap_or_default(),
                enabled: c.enabled.unwrap_or(true),
                install_hint: c.install_hint.clone().unwrap_or_default(),
                preset: false,
                env: c.env.clone(),
                initialization_options: c.initialization_options.clone(),
                settings: c.settings.clone(),
                rustup_component: None,
            });
        }
        for p in PRESETS {
            let o = self.servers.get(p.id).cloned().unwrap_or_default();
            let command_overridden = o.command.as_ref().is_some_and(|c| !c.trim().is_empty());
            out.push(ServerSpec {
                id: p.id.into(),
                label: o.label.unwrap_or_else(|| p.label.into()),
                command: o.command.filter(|c| !c.trim().is_empty()).unwrap_or_else(|| p.command.into()),
                args: o.args.unwrap_or_else(|| strings(p.args)),
                languages: o.languages.unwrap_or_else(|| strings(p.languages)),
                extensions: norm_exts(o.extensions.unwrap_or_else(|| strings(p.extensions))),
                root_markers: o.root_markers.unwrap_or_else(|| strings(p.root_markers)),
                enabled: o.enabled.unwrap_or(true),
                install_hint: o.install_hint.unwrap_or_else(|| p.install_hint.into()),
                preset: true,
                env: o.env,
                initialization_options: o.initialization_options,
                settings: o.settings,
                // An explicit command is taken as given.
                rustup_component: if command_overridden { None } else { p.rustup_component.map(str::to_string) },
            });
        }
        (out, warnings)
    }

    pub fn idle(&self) -> Duration {
        Duration::from_secs(self.idle_minutes.unwrap_or(10).clamp(1, 24 * 60) * 60)
    }
}

/// The LSP `languageId` of a file: from its extension (the protocol's identifiers:
/// `typescriptreact` for `.tsx`, `shellscript`…), else the editor's language.
pub fn language_id(ext: &str, monaco: Option<&str>) -> String {
    let by_ext = match ext {
        "rs" => "rust",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "py" | "pyi" => "python",
        "go" => "go",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "c++" | "hpp" | "hh" | "hxx" | "h++" | "ipp" | "tpp" | "txx" | "inl" | "ixx" | "cppm" | "ino"
        | "cu" | "cuh" => "cpp",
        "v" | "vh" => "verilog",
        "sv" | "svh" => "systemverilog",
        "vhd" | "vhdl" | "vho" | "vht" => "vhdl",
        "m" => "objective-c",
        "mm" => "objective-cpp",
        "sh" | "bash" | "zsh" => "shellscript",
        "yml" | "yaml" => "yaml",
        "json" => "json",
        "jsonc" => "jsonc",
        "toml" => "toml",
        "md" | "markdown" => "markdown",
        "cs" => "csharp",
        "lua" => "lua",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "rb" => "ruby",
        "php" => "php",
        "swift" => "swift",
        "zig" => "zig",
        "html" | "htm" => "html",
        "css" => "css",
        "scss" => "scss",
        "less" => "less",
        "sql" => "sql",
        "dart" => "dart",
        "ex" | "exs" => "elixir",
        "hs" => "haskell",
        "ml" | "mli" => "ocaml",
        "scala" => "scala",
        "vue" => "vue",
        "svelte" => "svelte",
        _ => "",
    };
    if !by_ext.is_empty() {
        return by_ext.into();
    }
    match monaco {
        Some("shell") => "shellscript".into(),
        Some(l) if !l.is_empty() => l.into(),
        _ => "plaintext".into(),
    }
}

/// Lowercase extension of a path (`""` for none; dotfiles have none).
pub fn extension(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rfind('.') {
        Some(i) if i > 0 => name[i + 1..].to_ascii_lowercase(),
        _ => String::new(),
    }
}

// ---------------------------------------------------------------- availability

/// Where a server's command is on this machine, or why it is not.
pub type Located = Result<PathBuf, String>;

/// Remembered `locate` results (PATH lookups and `rustup which` are cheap, not free).
#[derive(Default)]
pub struct Availability {
    cache: Mutex<BTreeMap<(String, Option<String>), (Instant, Located)>>,
}

const AVAIL_TTL: Duration = Duration::from_secs(30);

impl Availability {
    pub async fn locate(&self, command: &str, rustup_component: Option<&str>) -> Located {
        let key = (command.to_string(), rustup_component.map(str::to_string));
        if let Some((at, r)) = self.cache.lock().get(&key) {
            if at.elapsed() < AVAIL_TTL {
                return r.clone();
            }
        }
        let r = locate(command, rustup_component).await;
        self.cache.lock().insert(key, (Instant::now(), r.clone()));
        r
    }

    /// Forget every result (Settings saved, a Refresh in the UI).
    pub fn clear(&self) {
        self.cache.lock().clear();
    }
}

/// Find `command` on PATH (or as a path). A rustup proxy (`~/.cargo/bin/rust-analyzer`
/// → `rustup`) is only a server when the toolchain has the component: then the real
/// binary from `rustup which` is used.
pub async fn locate(command: &str, rustup_component: Option<&str>) -> Located {
    let cmd = command.to_string();
    let found = tokio::task::spawn_blocking(move || crate::util::which_path(&cmd))
        .await
        .ok()
        .flatten();
    let Some(path) = found else {
        return Err(format!("{command} was not found on PATH{}", exe::INSTALLED_SINCE));
    };
    if !exe::is_executable(&path) {
        return Err(format!("{} is not executable", path.display()));
    }
    let Some(rustup) = exe::rustup_proxy(&path) else {
        return Ok(path);
    };
    let component = rustup_component.unwrap_or(command);
    let out = crate::util::proc::run(
        &rustup.to_string_lossy(),
        &["which", component],
        Path::new("/"),
        Duration::from_secs(10),
    )
    .await;
    match out {
        Ok(o) if o.ok() => {
            let p = PathBuf::from(o.stdout.trim());
            if p.is_file() && exe::is_executable(&p) {
                Ok(p)
            } else {
                Err(format!("rustup names {} for {component}, which is not an executable file", p.display()))
            }
        }
        _ => Err(format!("{} is a rustup proxy, but the toolchain has no {component} component: rustup component add {component}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_merge_with_overrides_and_own_servers_come_first() {
        let text = r#"
            idle_minutes = 3
            [servers.rust-analyzer]
            command = "~/bin/ra"
            settings = { rust-analyzer = { check = { command = "clippy" } } }
            [servers.pyright]
            enabled = false
            [servers.zls]
            command = "zls"
            extensions = [".ZIG"]
            [servers.broken]
            extensions = ["x"]
            [servers."Bad Id"]
            command = "x"
            extensions = ["x"]
        "#;
        let cfg: LspConfig = toml::from_str(text).unwrap();
        assert_eq!(cfg.idle(), Duration::from_secs(180));
        let (specs, warnings) = cfg.specs();
        assert_eq!(specs[0].id, "zls");
        assert_eq!(specs[0].extensions, vec!["zig"]);
        assert!(!specs[0].preset);
        let ra = specs.iter().find(|s| s.id == "rust-analyzer").unwrap();
        assert_eq!(ra.command, "~/bin/ra");
        // An explicit command is used as given, not through rustup.
        assert_eq!(ra.rustup_component, None);
        assert_eq!(ra.settings.as_ref().unwrap()["rust-analyzer"]["check"]["command"], "clippy");
        assert!(!specs.iter().find(|s| s.id == "pyright").unwrap().enabled);
        assert!(specs.iter().any(|s| s.id == "basedpyright" && s.enabled));
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        // The default preset goes through rustup when it is a proxy.
        let (defaults, _) = LspConfig::default().specs();
        assert_eq!(defaults[0].rustup_component.as_deref(), Some("rust-analyzer"));
        assert_eq!(defaults.len(), PRESETS.len());
    }

    #[test]
    fn config_round_trips_through_global_config() {
        let text = r#"
            [lsp]
            idle_minutes = 5
            [lsp.servers.typescript]
            args = ["--stdio", "--log-level", "4"]
            env = { TSS_LOG = "-level verbose" }
            initialization_options = { preferences = { includeInlayParameterNameHints = "all" } }
        "#;
        let cfg: crate::config::GlobalConfig = toml::from_str(text).unwrap();
        let written = toml::to_string_pretty(&cfg).unwrap();
        assert_eq!(toml::from_str::<crate::config::GlobalConfig>(&written).unwrap(), cfg, "{written}");
        let plain = toml::to_string_pretty(&crate::config::GlobalConfig::default()).unwrap();
        assert!(!plain.contains("[lsp"), "{plain}");
    }

    #[test]
    fn files_pick_servers_by_extension_then_language() {
        let (specs, _) = LspConfig::default().specs();
        let pick = |ext: &str, lang: Option<&str>| specs.iter().find(|s| s.handles(ext, lang)).map(|s| s.id.as_str());
        assert_eq!(pick("rs", None), Some("rust-analyzer"));
        assert_eq!(pick("tsx", Some("typescript")), Some("typescript"));
        assert_eq!(pick("py", None), Some("pyright"));
        assert_eq!(pick("", Some("shell")), Some("bash"));
        assert_eq!(pick("txt", Some("plaintext")), None);
        assert_eq!(language_id("tsx", Some("typescript")), "typescriptreact");
        assert_eq!(language_id("sh", Some("shell")), "shellscript");
        assert_eq!(language_id("weird", Some("shell")), "shellscript");
        assert_eq!(language_id("weird", None), "plaintext");
        assert_eq!(language_id("c", Some("c")), "c");
        assert_eq!(language_id("inl", None), "cpp");
        assert_eq!(language_id("v", None), "verilog");
        assert_eq!(language_id("svh", None), "systemverilog");
        assert_eq!(language_id("vhd", None), "vhdl");
        assert_eq!(pick("inl", None), Some("clangd"));
        assert_eq!(pick("c", Some("c")), Some("clangd"));
        assert_eq!(pick("v", None), Some("verible"));
        assert_eq!(pick("sv", None), Some("verible"));
        assert_eq!(pick("", Some("systemverilog")), Some("verible"));
        assert_eq!(pick("vhd", None), Some("vhdl_ls"));
        assert_eq!(pick("", Some("vhdl")), Some("vhdl_ls"));
        assert_eq!(extension("src/Main.RS"), "rs");
        assert_eq!(extension(".bashrc"), "");
        assert_eq!(extension("a.b/c"), "");
    }

    #[test]
    fn ids_are_url_safe() {
        assert!(valid_id("rust-analyzer") && valid_id("csharp-ls") && valid_id("a.b_c"));
        assert!(!valid_id("") && !valid_id("-x") && !valid_id("A") && !valid_id("a/b") && !valid_id(&"a".repeat(49)));
    }

    #[tokio::test]
    #[cfg(unix)] // shell-script fakes; util::os::exe tests the Windows proxies
    async fn rustup_proxies_count_only_with_the_component() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path();
        // A fake rustup that knows no components, and a proxy named like the server.
        std::fs::write(bin.join("rustup"), "#!/bin/sh\necho \"error: unknown binary '$2'\" >&2\nexit 1\n").unwrap();
        std::fs::write(bin.join("real-ls"), "#!/bin/sh\nexit 0\n").unwrap();
        for f in ["rustup", "real-ls"] {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(bin.join(f), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        crate::util::os::fs::symlink(bin.join("rustup"), bin.join("fake-analyzer")).unwrap();
        let proxy = bin.join("fake-analyzer").display().to_string();
        let err = locate(&proxy, Some("rust-analyzer")).await.unwrap_err();
        assert!(err.contains("rustup component add rust-analyzer"), "{err}");
        // With the component, `rustup which` names the real binary.
        std::fs::write(bin.join("rustup"), format!("#!/bin/sh\necho {}\n", bin.join("real-ls").display())).unwrap();
        assert_eq!(locate(&proxy, Some("rust-analyzer")).await.unwrap(), bin.join("real-ls"));
        // Plain binaries are themselves.
        let real = bin.join("real-ls").display().to_string();
        assert_eq!(locate(&real, None).await.unwrap(), bin.join("real-ls"));
        assert!(locate("definitely-not-a-command-xyz", None).await.is_err());
    }
}
