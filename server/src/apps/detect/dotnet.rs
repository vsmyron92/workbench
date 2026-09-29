//! .NET solutions (build + test when a project references Microsoft.NET.Test.Sdk)
//! and `validate.*` data-validation scripts.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use super::{Ctx, on_path, scoped, source, tilde};
use crate::config::project::{Component, RunConfig, RunKind};
use crate::util::os::shell::Dialect;

static SLN_PROJECT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?m)^Project\("\{[^}]+\}"\)\s*=\s*"[^"]*",\s*"([^"]+\.csproj)""#).unwrap());

/// `Passed!  - Failed:     0, Passed:    12, Skipped: 0, Total: 12` (one line per test project, summed).
pub const DOTNET_TEST_RESULT: &str = r"^\s*(?P<status>Passed|Failed)!\s+-\s+Failed:\s+(?P<failed>\d+),\s+Passed:\s+(?P<passed>\d+)";

/// `dotnet` on PATH, else the user-local SDK as a `{dotnet}` toolchain.
fn dotnet_cmd(cx: &mut Ctx) -> String {
    if on_path("dotnet") {
        return "dotnet".into();
    }
    if let Some(local) = dirs::home_dir().map(|h| h.join(".dotnet/dotnet")).filter(|p| p.is_file()) {
        cx.pf.toolchains.entry("dotnet".into()).or_insert_with(|| tilde(&local));
        return "{dotnet}".into();
    }
    "dotnet".into()
}

pub fn detect(cx: &mut Ctx, sln: &Path) {
    let Some(dir) = sln.parent() else { return };
    // Unity regenerates a .sln at its project root; that one is not a .NET solution to build.
    if cx.is_file(&dir.join("ProjectSettings/ProjectVersion.txt")) {
        return;
    }
    let Some(text) = cx.read(sln) else { return };
    let cwd = cx.rel(dir);
    cx.tag("dotnet");
    cx.pf.components.push(Component { name: scoped("dotnet", &cwd), path: cwd.clone(), kind: "dotnet".into(), version: None });
    let has_tests = SLN_PROJECT
        .captures_iter(&text)
        .map(|c| c[1].replace('\\', "/"))
        .filter(|p| !p.contains("..") && crate::util::os::path::stays_inside(p))
        .any(|p| cx.read(&dir.join(&p)).is_some_and(|csproj| csproj.contains("Microsoft.NET.Test.Sdk")));
    let dotnet = dotnet_cmd(cx);
    cx.add_run(RunConfig {
        name: scoped("dotnet build", &cwd),
        kind: RunKind::Build,
        command: format!("{dotnet} build"),
        cwd: cwd.clone(),
        source: source(cx, sln, ""),
        group: Some("build".into()),
        ..Default::default()
    });
    if has_tests {
        cx.add_run(RunConfig {
            name: scoped("dotnet test", &cwd),
            kind: RunKind::Test,
            command: format!("{dotnet} test"),
            cwd,
            result_pattern: Some(DOTNET_TEST_RESULT.into()),
            source: source(cx, sln, " (Microsoft.NET.Test.Sdk)"),
            group: Some("test".into()),
            ..Default::default()
        });
    }
}

/// `validate.mjs` & co.: a data check that must exit 0, run in its own folder. Where the
/// run shell is PowerShell a `validate.sh` is not offered: `bash` there is WSL's, a Linux
/// system with other paths and tools (as `posix_only` leaves out `.sh` commands).
pub fn detect_validate_script(cx: &mut Ctx, f: &Path) {
    let Some(dir) = f.parent() else { return };
    let Some(name) = f.file_name().and_then(|n| n.to_str()) else { return };
    let interpreter = match f.extension().and_then(|e| e.to_str()) {
        Some("mjs" | "js" | "cjs") => "node".to_string(),
        Some("py") => super::python_words(),
        Some("sh") if super::dialect() == Dialect::Posix => "bash".to_string(),
        _ => return,
    };
    let cwd = cx.rel(dir);
    cx.add_run(RunConfig {
        name: scoped("validate", &cwd),
        kind: RunKind::Test,
        command: format!("{interpreter} {name}"),
        cwd,
        source: source(cx, f, ""),
        group: Some("test".into()),
        ..Default::default()
    });
}
