//! Unity projects: `ProjectSettings/ProjectVersion.txt` → the editor toolchain
//! (`~/Unity/Hub/Editor/<ver>/Editor/Unity`, on Windows
//! `%ProgramFiles%\Unity\Hub\Editor\<ver>\Editor\Unity.exe`) and an Editor run; every
//! `[MenuItem("…")]` on a parameterless `public static void` in an `Editor/`
//! script → a batch-mode `-executeMethod` run.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use super::{Ctx, rel, scoped, source, tilde, walk_filtered};
use crate::config::project::{Component, RunConfig, RunKind};
use crate::util::os::shell::Dialect;

/// `m_EditorVersion: 6000.5.6f1`. Only a version's characters: it becomes part of the
/// editor's path, which commands insert as it is (`{unity}`).
static VERSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)m_EditorVersion:\s*([0-9A-Za-z._-]+)[ \t\r]*$").unwrap());
static MENU_ITEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"\[(?:UnityEditor\.)?MenuItem\(\s*"([^"]+)"\s*(,[^\]]*)?\)\]\s*(?:\[[^\]]*\]\s*)*public\s+static\s+void\s+(\w+)\s*\(\s*\)"#,
    )
    .unwrap()
});
static NAMESPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\s*namespace\s+([\w.]+)").unwrap());
static CLASS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(?:class|struct)\s+(\w+)").unwrap());
static BLANK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)//[^\n]*|/\*.*?\*/|@"(?:[^"]|"")*"|\$?"(?:\\.|[^"\\\n])*"|'(?:\\.|[^'\\])'"#).unwrap()
});
/// A result line a test writes, e.g. `"SMOKETEST_RESULT: "` → `^SMOKETEST_RESULT: (?P<name>…) = (?P<status>PASS|FAIL)`.
static RESULT_PREFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\$?"([A-Z][A-Z0-9_]*_RESULT):\s"#).unwrap());

/// Calls that make a menu item GUI-only (pointless in batch mode).
const GUI_ONLY: &[&str] = &["GetWindow", "ShowWindow", "ShowUtility", "Selection.", "EditorUtility.DisplayDialog", ".Show()"];
/// Editor scripts visited per Unity project.
const MAX_SCRIPTS_WALK: usize = 40_000;

pub fn detect(cx: &mut Ctx, version_file: &Path) {
    let Some(proj) = version_file.parent().and_then(|p| p.parent()) else { return };
    let Some(text) = cx.read(version_file) else { return };
    let Some(ver) = VERSION.captures(&text).map(|c| c[1].to_string()) else { return };
    let cwd = cx.rel(proj);
    cx.tag("unity");
    cx.pf.components.push(Component { name: scoped("unity", &cwd), path: cwd.clone(), kind: "unity".into(), version: Some(ver.clone()) });

    // One toolchain per editor version; the common single-version case is plain `{unity}`.
    let editor = tilde(&editor_path(&ver));
    let key = match cx.pf.toolchains.get("unity") {
        None => "unity".to_string(),
        Some(existing) if *existing == editor => "unity".to_string(),
        Some(_) => format!("unity-{ver}"),
    };
    cx.pf.toolchains.insert(key.clone(), editor);
    // `"$PWD"` is the working directory in PowerShell too; there the editor's path (with
    // a space: `Program Files`) is a quoted string started with the call operator (a
    // toolchain path set by the user must not contain `'`).
    let unity = match super::dialect() {
        Dialect::Posix => format!("{{{key}}}"),
        Dialect::PowerShell => format!("& '{{{key}}}'"),
    };

    cx.add_run(RunConfig {
        name: scoped("Unity Editor", &cwd),
        kind: RunKind::Editor,
        command: format!("{unity} -projectPath \"$PWD\""),
        cwd: cwd.clone(),
        source: source(cx, version_file, ""),
        group: Some("unity".into()),
        ..Default::default()
    });

    let assets = proj.join("Assets");
    // Unity ignores folders ending in `~` and hidden ones; so do we.
    let scripts = walk_filtered(&assets, 16, MAX_SCRIPTS_WALK, &|n| n.starts_with('.') || n.ends_with('~'));
    for cs in scripts {
        if cs.extension().is_none_or(|x| x != "cs") {
            continue;
        }
        let under_editor = cs.strip_prefix(&assets).is_ok_and(|r| r.components().any(|c| c.as_os_str() == "Editor"));
        if !under_editor {
            continue;
        }
        let Some(src) = cx.read(&cs) else { continue };
        if !src.contains("MenuItem") {
            continue;
        }
        let script_rel = rel(cx.root, &cs);
        for item in menu_items(&src) {
            let quit = if item.exits_itself { "" } else { " -quit" };
            let lower = format!("{} {}", item.menu, item.method).to_lowercase();
            let kind = if ["test", "validate", "smoke"].iter().any(|w| lower.contains(w)) {
                RunKind::Test
            } else if lower.contains("build") {
                RunKind::Build
            } else {
                RunKind::Task
            };
            let result_pattern = (kind == RunKind::Test).then(|| item.result_pattern.clone()).flatten();
            cx.add_run(RunConfig {
                name: scoped(&format!("Unity: {}", item.menu), &cwd),
                kind,
                command: format!("{unity} -batchmode -nographics{quit} -projectPath \"$PWD\" -executeMethod {} -logFile -", item.execute_method),
                cwd: cwd.clone(),
                result_pattern,
                source: Some(format!("detected:{script_rel} [MenuItem(\"{}\")]", item.menu)),
                group: Some("unity".into()),
                ..Default::default()
            });
        }
    }
}

/// Where Unity Hub installs editor `ver`: `~/Unity/Hub/Editor/<ver>/Editor/Unity`; on
/// Windows (`super::dialect`) `%ProgramFiles%\Unity\Hub\Editor\<ver>\Editor\Unity.exe`.
fn editor_path(ver: &str) -> std::path::PathBuf {
    match super::dialect() {
        Dialect::Posix => dirs::home_dir().unwrap_or_default().join(format!("Unity/Hub/Editor/{ver}/Editor/Unity")),
        Dialect::PowerShell => {
            let pf = std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into());
            std::path::PathBuf::from(format!(r"{pf}\Unity\Hub\Editor\{ver}\Editor\Unity.exe"))
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MenuItem {
    /// Menu path without the shortcut suffix: `MyGame/Build Everything`.
    pub menu: String,
    pub method: String,
    /// `Ns.Class.Method` (nested classes joined with `+`).
    pub execute_method: String,
    /// The script calls `EditorApplication.Exit` itself, so `-quit` must not be passed.
    pub exits_itself: bool,
    pub result_pattern: Option<String>,
}

/// Batch-runnable menu items of one C# source file.
pub fn menu_items(src: &str) -> Vec<MenuItem> {
    let clean = BLANK.replace_all(src, |m: &regex::Captures| blank_like(&m[0])).into_owned();
    let exits_itself = clean.contains("EditorApplication.Exit");
    let result_pattern = RESULT_PREFIX.captures(src).map(|c| {
        format!(r"^{}: (?P<name>.+?) = (?P<status>PASS|FAIL|OK|ERROR)\b(?:\s*::\s*(?P<detail>.*))?$", regex::escape(&c[1]))
    });
    let mut out = vec![];
    for c in MENU_ITEM.captures_iter(src) {
        let Some(whole) = c.get(0) else { continue };
        // A match inside a comment is blanked in `clean`.
        if !clean.get(whole.start()..).is_some_and(|s| s.starts_with('[')) {
            continue;
        }
        // [MenuItem("X", true)] marks a validation function, not an action.
        if c.get(2).is_some_and(|args| args.as_str().trim_start_matches(',').trim().starts_with("true")) {
            continue;
        }
        let menu = strip_shortcut(&c[1]);
        let method = c[3].to_string();
        if is_gui_only(&clean, whole.end()) {
            continue;
        }
        let Some(class_path) = enclosing_classes(&clean, whole.start()) else { continue };
        let ns = NAMESPACE
            .captures_iter(&clean)
            .take_while(|n| n.get(0).is_some_and(|m| m.start() < whole.start()))
            .last()
            .map(|n| format!("{}.", &n[1]))
            .unwrap_or_default();
        out.push(MenuItem {
            menu,
            execute_method: format!("{ns}{class_path}.{method}"),
            method,
            exits_itself,
            result_pattern: result_pattern.clone(),
        });
    }
    out
}

/// Same length as `s`, newlines kept (so byte offsets stay valid), everything else blank.
fn blank_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch == '\n' {
            out.push('\n');
        } else {
            // A multi-byte char becomes as many spaces as it had bytes.
            (0..ch.len_utf8()).for_each(|_| out.push(' '));
        }
    }
    out
}

/// `Tools/Do It %#d` → `Tools/Do It` (Unity shortcut suffixes start with %, #, & or _).
fn strip_shortcut(menu: &str) -> String {
    match menu.rsplit_once(' ') {
        Some((head, last)) if last.starts_with(['%', '#', '&', '_']) && last.len() <= 6 => head.trim().to_string(),
        _ => menu.trim().to_string(),
    }
}

/// Whether the method whose signature ends at `sig_end` opens windows or needs a selection.
fn is_gui_only(clean: &str, sig_end: usize) -> bool {
    let rest = clean.get(sig_end..).unwrap_or("");
    let body = match rest.trim_start().strip_prefix("=>") {
        Some(expr) => expr.split(';').next().unwrap_or(""),
        None => match super::text::block_at(clean, sig_end) {
            Some((a, b)) => &clean[a..b],
            None => return false,
        },
    };
    GUI_ONLY.iter().any(|g| body.contains(g))
}

/// Classes (outermost first, joined by `+`) whose braces enclose byte offset `pos`
/// of comment- and string-blanked source.
pub fn enclosing_classes(clean: &str, pos: usize) -> Option<String> {
    let decls: Vec<(usize, String)> =
        CLASS.captures_iter(clean).filter_map(|c| Some((c.get(0)?.end(), c[1].to_string()))).collect();
    let mut stack: Vec<(String, usize)> = vec![]; // (name, depth of its body)
    let mut pending: Option<String> = None;
    let mut depth = 0usize;
    let mut di = 0;
    for (i, ch) in clean.char_indices() {
        if i >= pos {
            break;
        }
        while di < decls.len() && decls[di].0 <= i {
            pending = Some(decls[di].1.clone());
            di += 1;
        }
        match ch {
            '{' => {
                depth += 1;
                if let Some(n) = pending.take() {
                    stack.push((n, depth));
                }
            }
            '}' => {
                if stack.last().is_some_and(|s| s.1 == depth) {
                    stack.pop();
                }
                depth = depth.saturating_sub(1);
            }
            ';' => pending = None,
            _ => {}
        }
    }
    (!stack.is_empty()).then(|| stack.iter().map(|s| s.0.as_str()).collect::<Vec<_>>().join("+"))
}
