//! JVM builds → runs.
//!
//! * **Gradle** (the shallowest `settings.gradle[.kts]` or `build.gradle[.kts]`,
//!   through `./gradlew` when the wrapper exists, `.\gradlew.bat` on Windows): `build`
//!   and `test`; Spring Boot `bootRun` (server, `server.port` or 8080), Quarkus
//!   `quarkusDev`, the `application` plugin's `run`; Android apps `assembleDebug`.
//!   Subprojects (`include("app")`) with those plugins get `:app:bootRun` & co.
//! * **Maven** (the shallowest `pom.xml`, through `./mvnw` when the wrapper exists,
//!   `.\mvnw.cmd` on Windows): `test` and `package`; Spring Boot `spring-boot:run`,
//!   Quarkus `quarkus:dev`, Micronaut `mn:run` (in the module that declares the plugin,
//!   `-pl <module>`).

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use super::{Ctx, scoped, sh, source};
use crate::config::project::{Component, Ready, RunConfig, RunKind};
use crate::util::os::shell::Dialect;

/// Spring Boot (`Started App in 2.3 seconds`, `Tomcat started on port 8080`), Quarkus
/// (`Listening on: http://localhost:8080`) and Micronaut (`Server Running: http://…`).
const JVM_READY: &str = r"(?:Tomcat|Netty|Jetty|Undertow) started on port|Started \S+ in [\d.]+ seconds|Listening on: (https?://\S+)|Server Running: (https?://\S+)";

static INCLUDE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?m)^\s*include\s*\(?([^)\n]*)\)?"#).unwrap());
static QUOTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"["']:?([\w.:-]+)["']"#).unwrap());
static MODULE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<module>\s*([^<\s]+)\s*</module>").unwrap());
static XML_COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").unwrap());
static PLUGIN_MANAGEMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<pluginManagement>.*?</pluginManagement>").unwrap());
static SERVER_PORT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*(?:server\.port\s*[=:]\s*|port:\s*)\$?\{?(?:[A-Z_]+:)?(\d{2,5})\}?\s*$").unwrap());

/// The project's build wrapper in `dir` as a command's first word: the script
/// (`gradlew`, `./gradlew`), or on Windows (`super::dialect`) its batch file
/// (`gradlew.bat`, `.\gradlew.bat`). `None` without one.
fn wrapper(cx: &Ctx, dir: &Path, posix: (&str, &'static str), windows: (&str, &'static str)) -> Option<&'static str> {
    let (file, command) = match super::dialect() {
        Dialect::Posix => posix,
        Dialect::PowerShell => windows,
    };
    cx.is_file(&dir.join(file)).then_some(command)
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum App {
    SpringBoot,
    Quarkus,
    Micronaut,
    Application,
    Android,
}

fn gradle_app(src: &str) -> Option<App> {
    // `id("org.springframework.boot") version "3.3.0" apply false` only declares the plugin.
    let src: String = src.lines().filter(|l| !l.contains("apply false") && !l.contains("apply(false)")).collect::<Vec<_>>().join("\n");
    if src.contains("org.springframework.boot") {
        Some(App::SpringBoot)
    } else if src.contains("io.quarkus") {
        Some(App::Quarkus)
    } else if src.contains("com.android.application") {
        Some(App::Android)
    } else if src.contains("io.micronaut.application") {
        Some(App::Micronaut)
    } else if src.contains("application") && (src.contains("mainClass") || src.contains("id(\"application\")") || src.contains("id 'application'") || src.contains("`application`") || src.contains("io.ktor.plugin")) {
        Some(App::Application)
    } else {
        None
    }
}

/// `server.port` from `src/main/resources/application.{properties,yml,yaml}` of a module.
fn spring_port(cx: &mut Ctx, module: &Path) -> u16 {
    for n in ["application.properties", "application.yml", "application.yaml"] {
        let p = module.join("src/main/resources").join(n);
        if cx.is_file(&p) {
            if let Some(port) = cx.read(&p).and_then(|t| {
                // YAML: only a `port:` under a `server:` block counts.
                if n.ends_with("properties") {
                    SERVER_PORT.captures(&t).and_then(|c| c[1].parse::<u16>().ok())
                } else {
                    let i = t.find("server:")?;
                    SERVER_PORT.captures(&t[i..]).and_then(|c| c[1].parse::<u16>().ok())
                }
            }) {
                return port;
            }
        }
    }
    8080
}

fn server_run(name: String, command: String, cwd: &str, port: u16, src: Option<String>) -> RunConfig {
    RunConfig {
        name,
        kind: RunKind::Server,
        command,
        cwd: cwd.to_string(),
        port: Some(port),
        // Cold Gradle/Maven builds take minutes before the app logs anything.
        ready: Some(Ready { log: Some(JVM_READY.into()), http: None, timeout_s: 600 }),
        preview: Some(format!("http://localhost:{port}/")),
        source: src,
        group: Some("dev".into()),
        ..Default::default()
    }
}

pub fn detect_gradle(cx: &mut Ctx, f: &Path) {
    let Some(dir) = f.parent() else { return };
    // One project per build root: `settings.gradle` and `build.gradle` of one
    // directory, and the subprojects below it.
    if cx.ancestor_marked("gradle:", dir) || !cx.mark(format!("gradle:{}", dir.display())) {
        return;
    }
    let cwd = cx.rel(dir);
    let gradle = wrapper(cx, dir, ("gradlew", "./gradlew"), ("gradlew.bat", r".\gradlew.bat")).unwrap_or("gradle");
    let settings = ["settings.gradle.kts", "settings.gradle"].iter().map(|n| dir.join(n)).find(|p| cx.is_file(p));
    let build = ["build.gradle.kts", "build.gradle"].iter().map(|n| dir.join(n)).find(|p| cx.is_file(p));
    let manifest = build.clone().or(settings.clone()).unwrap_or_else(|| f.to_path_buf());
    cx.tag("gradle");
    cx.tag("jvm");
    cx.pf.components.push(Component { name: scoped("gradle", &cwd), path: cwd.clone(), kind: "gradle".into(), version: None });

    // (task path prefix, module dir, build file text)
    let mut modules: Vec<(String, PathBuf, String)> = vec![];
    if let Some(b) = &build {
        modules.push((String::new(), dir.to_path_buf(), cx.read(b).unwrap_or_default()));
    }
    if let Some(s) = &settings {
        let text = cx.read(s).unwrap_or_default();
        let mut names: Vec<String> = vec![];
        for inc in INCLUDE.captures_iter(&text) {
            for q in QUOTED.captures_iter(&inc[1]) {
                let n = q[1].trim_start_matches(':').to_string();
                if !n.is_empty() && !names.contains(&n) && names.len() < 20 {
                    names.push(n);
                }
            }
        }
        for n in names {
            let rel = n.replace(':', "/");
            if !crate::util::os::path::stays_inside(&rel) {
                continue;
            }
            let mdir = dir.join(rel);
            let Some(mb) = ["build.gradle.kts", "build.gradle"].iter().map(|x| mdir.join(x)).find(|p| cx.is_file(p)) else { continue };
            cx.mark(format!("gradle:{}", mdir.display()));
            modules.push((format!(":{n}:"), mdir, cx.read(&mb).unwrap_or_default()));
        }
    }
    let android = modules.iter().any(|m| gradle_app(&m.2) == Some(App::Android));
    let simple = |name: &str, kind: RunKind, task: &str, group: &str| RunConfig {
        name: scoped(name, &cwd),
        kind,
        command: format!("{gradle} {task}"),
        cwd: cwd.clone(),
        group: Some(group.into()),
        ..Default::default()
    };
    let mut base = vec![
        simple("gradle test", RunKind::Test, "test", "test"),
        if android { simple("gradle assembleDebug", RunKind::Build, "assembleDebug", "build") } else { simple("gradle build", RunKind::Build, "build", "build") },
    ];
    for r in &mut base {
        r.source = source(cx, &manifest, "");
    }
    for r in base {
        cx.add_run(r);
    }
    let mut apps = 0;
    for (prefix, mdir, text) in &modules {
        if apps >= 6 {
            break;
        }
        let Some(app) = gradle_app(text) else { continue };
        let label = if prefix.is_empty() { String::new() } else { format!(" {}", prefix.trim_matches(':')) };
        let src = source(cx, &mdir.join(if cx.is_file(&mdir.join("build.gradle.kts")) { "build.gradle.kts" } else { "build.gradle" }), "");
        let run = match app {
            App::SpringBoot => {
                let port = spring_port(cx, mdir);
                server_run(scoped(&format!("bootRun{label}"), &cwd), format!("{gradle} {prefix}bootRun"), &cwd, port, src)
            }
            App::Quarkus => server_run(scoped(&format!("quarkusDev{label}"), &cwd), format!("{gradle} {prefix}quarkusDev"), &cwd, 8080, src),
            App::Micronaut => server_run(scoped(&format!("gradle run{label}"), &cwd), format!("{gradle} {prefix}run"), &cwd, 8080, src),
            App::Application => {
                let ktor = text.contains("io.ktor");
                let mut r = RunConfig {
                    name: scoped(&format!("gradle run{label}"), &cwd),
                    kind: if ktor { RunKind::Server } else { RunKind::Task },
                    command: format!("{gradle} {prefix}run"),
                    cwd: cwd.clone(),
                    source: src,
                    group: Some("dev".into()),
                    ..Default::default()
                };
                if ktor {
                    r.port = Some(8080);
                    r.preview = Some("http://localhost:8080/".into());
                }
                r
            }
            App::Android => continue,
        };
        cx.add_run(run);
        apps += 1;
    }
}

pub fn detect_maven(cx: &mut Ctx, f: &Path) {
    let Some(dir) = f.parent() else { return };
    if cx.ancestor_marked("maven:", dir) || !cx.mark(format!("maven:{}", dir.display())) {
        return;
    }
    let Some(raw) = cx.read(f) else { return };
    let pom = XML_COMMENT.replace_all(&raw, "").into_owned();
    if !pom.contains("<project") {
        return;
    }
    let cwd = cx.rel(dir);
    let mvn = wrapper(cx, dir, ("mvnw", "./mvnw"), ("mvnw.cmd", r".\mvnw.cmd")).unwrap_or("mvn");
    cx.tag("maven");
    cx.tag("jvm");
    cx.pf.components.push(Component { name: scoped("maven", &cwd), path: cwd.clone(), kind: "maven".into(), version: None });
    for (name, kind, goal, group) in [("mvn test", RunKind::Test, "test", "test"), ("mvn package", RunKind::Build, "package", "build")] {
        cx.add_run(RunConfig {
            name: scoped(name, &cwd),
            kind,
            command: format!("{mvn} {goal}"),
            cwd: cwd.clone(),
            source: source(cx, f, ""),
            group: Some(group.into()),
            ..Default::default()
        });
    }
    // The root and its modules (one level of `<modules>`).
    let mut modules: Vec<(Option<String>, PathBuf, PathBuf, String)> = vec![(None, dir.to_path_buf(), f.to_path_buf(), pom.clone())];
    for m in MODULE.captures_iter(&pom).take(20) {
        let name = m[1].trim_end_matches('/').to_string();
        if name.contains("..") || !crate::util::os::path::stays_inside(&name) {
            continue;
        }
        let mdir = dir.join(&name);
        let mp = mdir.join("pom.xml");
        cx.mark(format!("maven:{}", mdir.display()));
        if let Some(t) = cx.read(&mp) {
            modules.push((Some(name), mdir, mp, XML_COMMENT.replace_all(&t, "").into_owned()));
        }
    }
    let mut apps = 0;
    for (module, mdir, mp, text) in &modules {
        if apps >= 6 {
            break;
        }
        // An aggregator (`<packaging>pom</packaging>`) runs nothing itself, and
        // `<pluginManagement>` only configures plugins for the modules.
        if text.contains("<packaging>pom</packaging>") {
            continue;
        }
        let text = PLUGIN_MANAGEMENT.replace_all(text, "");
        let (label, goal, port) = if text.contains("spring-boot-maven-plugin") {
            ("spring-boot:run", "spring-boot:run", spring_port(cx, mdir))
        } else if text.contains("quarkus-maven-plugin") {
            ("quarkus:dev", "quarkus:dev", 8080)
        } else if text.contains("micronaut-maven-plugin") {
            ("mn:run", "mn:run", 8080)
        } else {
            continue;
        };
        let (name, pl) = match module {
            Some(m) => (format!("{label} {m}"), format!(" -pl {}", sh(m))),
            None => (label.to_string(), String::new()),
        };
        let src = source(cx, mp, "");
        cx.add_run(server_run(scoped(&name, &cwd), format!("{mvn}{pl} {goal}"), &cwd, port, src));
        apps += 1;
    }
}
