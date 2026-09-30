//! The forms detection writes where the run shell is PowerShell (Windows), checked on
//! every OS through `detect_as`: the venv's `Scripts\python.exe`, `python` or `py -3`,
//! `.\build\Debug\app.exe` without `&&`, the build wrappers' batch files, Windows'
//! CMake presets, and repository commands in POSIX syntax left out.

use super::{detect_checked, detect_checked_as, has_run, names, run, tree, write};
use crate::util::os::exe::python_words;
use crate::util::os::shell::Dialect;

fn win(root: &std::path::Path) -> crate::config::ProjectFile {
    detect_checked_as(root, Dialect::PowerShell)
}

/// `first`, then `then` when it succeeded, in PowerShell (`Dialect::and_then`).
fn ps_and_then(first: &str, then: &str) -> String {
    Dialect::PowerShell.and_then(first, then)
}

#[test]
fn python_runs_use_the_windows_virtualenv_layout() {
    let d = tree(&[
        ("requirements.txt", "flask>=3\npytest\n"),
        ("app.py", "from flask import Flask\n\napp = Flask(__name__)\n\nif __name__ == \"__main__\":\n    app.run(port=5050)\n"),
        (".venv/pyvenv.cfg", "home = C:\\Python313\n"),
        (".venv/Scripts/python.exe", ""),
        ("tests/test_app.py", "def test_x(): pass\n"),
        ("tools/pyproject.toml", "[project]\nname = \"tools\"\n[project.scripts]\ngen-docs = \"tools.cli:main\"\n\"my tool\" = \"tools.cli:other\"\n[tool.pytest.ini_options]\n"),
    ]);
    let pf = win(d.path());
    assert_eq!(run(&pf, "flask run").command, r".venv\Scripts\python.exe -m flask --app app run --debug --port 5050");
    assert_eq!(run(&pf, "pytest").command, r".venv\Scripts\python.exe -m pytest");
    // A project in a folder uses the root's virtualenv; console scripts are its .exe files.
    assert_eq!(run(&pf, "pytest (tools)").command, r"..\.venv\Scripts\python.exe -m pytest");
    assert_eq!(run(&pf, "gen-docs (tools)").command, r"..\.venv\Scripts\gen-docs.exe");
    assert_eq!(run(&pf, "my tool (tools)").command, r"& '..\.venv\Scripts\my tool.exe'");
    // The same tree on Unix: a Windows virtualenv is not one to run there.
    let pf = detect_checked(d.path());
    assert_eq!(run(&pf, "pytest").command, "python3 -m pytest");
}

#[test]
fn python_without_a_virtualenv_is_python_or_the_launcher() {
    let d = tree(&[
        ("backend/requirements.txt", "django>=5\n"),
        ("backend/manage.py", "import os\nos.environ.setdefault(\"DJANGO_SETTINGS_MODULE\", \"blog.settings\")\n"),
        (".venv/pyvenv.cfg", "home = /usr/bin\n"),
        (".venv/bin/python", ""),
        ("data/validate.py", "print('ok')\n"),
        ("tasks/pyproject.toml", "[project]\nname = \"x\"\n[tool.pdm.scripts]\n\"it's\" = \"echo hi\"\n"),
    ]);
    let pf = win(d.path());
    let py = python_words();
    // The Unix virtualenv is not one Windows runs.
    assert_eq!(run(&pf, "runserver (backend)").command, format!("{py} manage.py runserver"));
    assert_eq!(run(&pf, "validate (data)").command, format!("{py} validate.py"));
    // Names from the repository are quoted for PowerShell.
    assert_eq!(run(&pf, "it's (tasks)").command, "pdm run 'it''s'");
    let pf = detect_checked(d.path());
    assert_eq!(run(&pf, "runserver (backend)").command, "../.venv/bin/python manage.py runserver");
    assert_eq!(run(&pf, "it's (tasks)").command, r"pdm run 'it'\''s'");
    assert_eq!(run(&pf, "validate (data)").command, "python3 validate.py");
}

#[test]
fn cmake_builds_and_runs_debug_executables_without_and_and() {
    let d = tree(&[
        ("CMakeLists.txt", "cmake_minimum_required(VERSION 3.20)\nproject(engine CXX)\nadd_subdirectory(src)\nenable_testing()\nadd_test(NAME t COMMAND engine)\n"),
        ("src/CMakeLists.txt", "add_executable(engine main.cpp)\n"),
    ]);
    let pf = win(d.path());
    assert_eq!(run(&pf, "cmake configure").command, "cmake -S . -B build -DCMAKE_BUILD_TYPE=Debug");
    assert_eq!(run(&pf, "cmake build").command, "cmake --build build --config Debug");
    assert_eq!(run(&pf, "ctest").command, "ctest --test-dir build -C Debug --output-on-failure");
    assert_eq!(run(&pf, "engine").command, ps_and_then("cmake --build build --config Debug --target engine", r".\build\src\Debug\engine.exe"));
    // Visual Studio puts a configuration folder below CMAKE_RUNTIME_OUTPUT_DIRECTORY too.
    let d = tree(&[(
        "CMakeLists.txt",
        "cmake_minimum_required(VERSION 3.16)\nproject(game)\nset(CMAKE_RUNTIME_OUTPUT_DIRECTORY ${CMAKE_BINARY_DIR}/bin)\nadd_executable(game main.cpp)\n",
    )]);
    let pf = win(d.path());
    assert_eq!(run(&pf, "game").command, ps_and_then("cmake --build build --config Debug --target game", r".\build\bin\Debug\game.exe"));
    // A build dir configured already keeps its generator: Ninja has no configuration folder.
    write(d.path(), "build/CMakeCache.txt", "# This is the CMakeCache file.\r\nCMAKE_GENERATOR:INTERNAL=Ninja\r\nCMAKE_BUILD_TYPE:STRING=Debug\r\n");
    let pf = win(d.path());
    assert_eq!(run(&pf, "game").command, ps_and_then("cmake --build build --target game", r".\build\bin\game.exe"));
    assert_eq!(run(&detect_checked(d.path()), "game").command, "cmake --build build --target game && ./build/bin/game");
}

const PRESETS: &str = r#"{
  "version": 6,
  "configurePresets": [
    { "name": "base", "hidden": true, "generator": "Ninja", "binaryDir": "${sourceDir}/out/build/${presetName}" },
    { "name": "linux-debug", "inherits": "base",
      "condition": { "type": "equals", "lhs": "${hostSystemName}", "rhs": "Linux" } },
    { "name": "windows-ninja", "inherits": "base",
      "condition": { "type": "equals", "lhs": "${hostSystemName}", "rhs": "Windows" } }
  ],
  "buildPresets": [
    { "name": "linux-debug", "configurePreset": "linux-debug" },
    { "name": "windows-ninja", "configurePreset": "windows-ninja" }
  ]
}"#;

#[test]
fn cmake_presets_for_windows() {
    let d = tree(&[("CMakeLists.txt", "cmake_minimum_required(VERSION 3.25)\nproject(app)\nadd_executable(app main.cpp)\n"), ("CMakePresets.json", PRESETS)]);
    let pf = win(d.path());
    assert_eq!(run(&pf, "cmake configure: windows-ninja").command, "cmake --preset windows-ninja");
    assert_eq!(run(&pf, "cmake build: windows-ninja").command, "cmake --build --preset windows-ninja");
    // Ninja keeps no folder per configuration.
    assert_eq!(run(&pf, "app").command, ps_and_then("cmake --build out/build/windows-ninja --target app", r".\out\build\windows-ninja\app.exe"));
    assert!(!has_run(&pf, "cmake configure: linux-debug"), "{:?}", names(&pf));
    // Visual Studio (also CMake's default when a preset names no generator) does.
    let vs = PRESETS.replace("\"generator\": \"Ninja\", ", "");
    let d = tree(&[("CMakeLists.txt", "cmake_minimum_required(VERSION 3.25)\nproject(app)\nadd_executable(app main.cpp)\n"), ("CMakePresets.json", &vs)]);
    let pf = win(d.path());
    assert_eq!(run(&pf, "app").command, ps_and_then("cmake --build out/build/windows-ninja --config Debug --target app", r".\out\build\windows-ninja\Debug\app.exe"));
    // Unless its build dir was configured with another one (`CMAKE_GENERATOR=Ninja`).
    write(d.path(), "out/build/windows-ninja/CMakeCache.txt", "CMAKE_GENERATOR:INTERNAL=Ninja\n");
    assert_eq!(run(&win(d.path()), "app").command, ps_and_then("cmake --build out/build/windows-ninja --target app", r".\out\build\windows-ninja\app.exe"));
    let pf = detect_checked(d.path());
    assert_eq!(run(&pf, "app").command, "cmake --build out/build/linux-debug --target app && ./out/build/linux-debug/app");
}

#[test]
fn build_wrappers_are_their_batch_files() {
    let d = tree(&[
        ("gradle/settings.gradle.kts", "rootProject.name = \"shop\"\n"),
        ("gradle/build.gradle.kts", "plugins { java }\n"),
        ("gradle/gradlew", "#!/bin/sh\n"),
        ("gradle/gradlew.bat", "@rem gradle\r\n"),
        ("maven/pom.xml", "<project><artifactId>m</artifactId></project>\n"),
        ("maven/mvnw", "#!/bin/sh\n"),
        ("maven/mvnw.cmd", "@REM maven\r\n"),
        ("unix-only/build.gradle", "plugins { id 'java' }\n"),
        ("unix-only/gradlew", "#!/bin/sh\n"),
    ]);
    let pf = win(d.path());
    assert_eq!(run(&pf, "gradle test (gradle)").command, r".\gradlew.bat test");
    assert_eq!(run(&pf, "mvn test (maven)").command, r".\mvnw.cmd test");
    // A wrapper without its batch file is a Unix script: the installed Gradle runs.
    assert_eq!(run(&pf, "gradle test (unix-only)").command, "gradle test");
    let pf = detect_checked(d.path());
    assert_eq!(run(&pf, "gradle test (unix-only)").command, "./gradlew test");
}

#[test]
fn scripts_run_through_their_interpreters() {
    let d = tree(&[
        ("rails/Gemfile", "gem \"rails\"\n"),
        ("rails/config/application.rb", "module Blog; end\n"),
        ("rails/bin/rails", "#!/usr/bin/env ruby\n"),
        ("rails/bin/dev", "#!/usr/bin/env sh\nexec foreman start -f Procfile.dev \"$@\"\n"),
        ("rails/test/models/x_test.rb", ""),
        ("rails8/Gemfile", "gem \"rails\"\n"),
        ("rails8/config/application.rb", "module Shop; end\n"),
        ("rails8/bin/rails", "#!/usr/bin/env ruby\n"),
        ("rails8/bin/dev", "#!/usr/bin/env ruby\nexec \"./bin/rails\", \"server\", *ARGV\n"),
        ("php/composer.json", "{\"require-dev\": {\"phpunit/phpunit\": \"^11\"}}"),
        ("php/phpunit.xml", "<phpunit/>"),
    ]);
    let pf = win(d.path());
    assert_eq!(run(&pf, "rails server (rails)").command, "ruby bin/rails server");
    assert_eq!(run(&pf, "rails test (rails)").command, "ruby bin/rails test");
    assert!(!has_run(&pf, "bin/dev (rails)"), "a shell script: {:?}", names(&pf));
    assert_eq!(run(&pf, "bin/dev (rails8)").command, "ruby bin/dev");
    assert_eq!(run(&pf, "phpunit (php)").command, "php vendor/bin/phpunit");
    let pf = detect_checked(d.path());
    assert_eq!(run(&pf, "bin/dev (rails)").command, "bin/dev");
    assert_eq!(run(&pf, "phpunit (php)").command, "vendor/bin/phpunit");
}

#[test]
fn unity_editor_in_program_files() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "ProjectSettings/ProjectVersion.txt", "m_EditorVersion: 6000.5.6f1\n");
    write(d.path(), "Assets/Editor/Build.cs", "using UnityEditor;\npublic static class B {\n    [MenuItem(\"Game/Build\")]\n    public static void All() { }\n}\n");
    let pf = win(d.path());
    let editor = &pf.toolchains["unity"];
    assert!(editor.ends_with(r"\Unity\Hub\Editor\6000.5.6f1\Editor\Unity.exe"), "{editor}");
    assert_eq!(run(&pf, "Unity Editor").command, "& '{unity}' -projectPath \"$PWD\"");
    assert_eq!(run(&pf, "Unity: Game/Build").command, "& '{unity}' -batchmode -nographics -quit -projectPath \"$PWD\" -executeMethod B.All -logFile -");
    // The version is part of the editor's path, which commands insert as it is: only a
    // version's characters.
    write(d.path(), "ProjectSettings/ProjectVersion.txt", "m_EditorVersion: 6000.0.1f1';Start-Process calc;'\n");
    for pf in [win(d.path()), detect_checked(d.path())] {
        assert!(pf.toolchains.is_empty() && !has_run(&pf, "Unity Editor"), "{:?}", names(&pf));
    }
}

#[test]
fn posix_commands_from_the_repository_are_left_out() {
    let d = tree(&[
        ("Procfile", "web: gunicorn app:app --bind 0.0.0.0:$PORT\nworker: celery -A app worker\nclock: bin/clock\nmail: FOO=1 mailer ${PORT:-25}\n"),
        ("bin/clock", "#!/usr/bin/env ruby\n"),
        ("configure", "#!/bin/sh\n"),
        ("gradlew", "#!/bin/sh\n"),
        ("gradlew.bat", "@rem gradle\r\n"),
        (
            "README.md",
            "```bash\ncargo test --workspace\nnpm run lint && npm test\nFOO=1 npm run dev\n./scripts/lint.sh\n./configure\n./gradlew check\n./target/release/app --help\npython3 -m http.server 8000\ncd web\nnpm run build\ncurl -s http://localhost:8080/api/health\n```\n",
        ),
    ]);
    let pf = win(d.path());
    // `$PORT` is PowerShell's `$env:PORT`; a script by its path would open in another program.
    let web = run(&pf, "Procfile: web");
    assert_eq!(web.command, "gunicorn app:app --bind 0.0.0.0:$env:PORT");
    assert_eq!((web.env.get("PORT").map(String::as_str), web.port), (Some("5000"), Some(5000)));
    assert!(has_run(&pf, "Procfile: worker") && !has_run(&pf, "Procfile: clock") && !has_run(&pf, "Procfile: mail"), "{:?}", names(&pf));
    let suggested: Vec<(&str, &str)> = pf.runs.iter().filter(|r| r.group.as_deref() == Some("suggested")).map(|r| (r.command.as_str(), r.cwd.as_str())).collect();
    // Windows PowerShell's `curl` is `Invoke-WebRequest`; `python3` is the Store's alias.
    let http_server = format!("{} -m http.server 8000", python_words());
    assert_eq!(
        suggested,
        vec![
            ("cargo test --workspace", "."),
            ("./gradlew check", "."),
            ("./target/release/app --help", "."),
            (http_server.as_str(), "."),
            ("npm run build", "web"),
            ("curl.exe -s http://localhost:8080/api/health", "web")
        ]
    );
    let pf = detect_checked(d.path());
    assert_eq!(run(&pf, "Procfile: web").command, "gunicorn app:app --bind 0.0.0.0:$PORT");
    assert!(has_run(&pf, "Procfile: clock") && has_run(&pf, "Procfile: mail"), "{:?}", names(&pf));
    assert_eq!(pf.runs.iter().filter(|r| r.group.as_deref() == Some("suggested")).count(), 10, "{:?}", names(&pf));
}

#[test]
fn names_from_repository_files_never_reach_a_batch_file() {
    let d = tree(&[
        ("package.json", r#"{"scripts":{"lint&calc":"eslint .","t%PATH%":"x","build:prod":"vite build","it's":"x"}}"#),
        ("php/composer.json", r#"{"scripts":{"t&calc":"phpunit","check":"phpunit"}}"#),
        ("Taskfile.yml", "version: '3'\ntasks:\n  \"a|b\":\n    cmds: [echo x]\n  ok:\n    cmds: [echo ok]\n"),
        ("game/ProjectSettings/ProjectVersion.txt", "m_EditorVersion: 6000.5.6f1\n"),
        ("game/Assets/Editor/Build.cs", "using UnityEditor;\npublic static class B {\n    [MenuItem(\"Game/Build & Run\")]\n    public static void All() { }\n}\n"),
    ]);
    // cmd.exe reads a batch file's arguments again (`composer.bat run-script t&calc` also
    // starts `calc`): a run that would hand one such a name is not offered.
    let pf = win(d.path());
    for name in ["lint&calc", "t%PATH%", "composer t&calc (php)", "task a|b"] {
        assert!(!has_run(&pf, name), "{name}: {:?}", names(&pf));
    }
    assert_eq!(run(&pf, "build:prod").command, "npm run build:prod");
    assert_eq!(run(&pf, "it's").command, "npm run 'it''s'");
    assert_eq!(run(&pf, "composer check (php)").command, "composer run-script check");
    assert_eq!(run(&pf, "task ok").command, "task ok");
    // A name that is not part of the command is no argument.
    assert!(has_run(&pf, "Unity: Game/Build & Run (game)"), "{:?}", names(&pf));
    // bash reads nothing again: the quoted names are offered there.
    let pf = detect_checked(d.path());
    assert_eq!(run(&pf, "lint&calc").command, "npm run 'lint&calc'");
    assert_eq!(run(&pf, "composer t&calc (php)").command, "composer run-script 't&calc'");

    use super::super::batch_safe;
    assert!(batch_safe(r"& '..\.venv\Scripts\my tool.exe' -x") && batch_safe("& '{unity}' -projectPath \"$PWD\""));
    assert!(batch_safe(r#"echo "it's" a|b"#), "the quote is in a double-quoted string, the rest is written in the file");
    assert!(!batch_safe("npm run 'a&b'") && !batch_safe("x 'it''s%'") && !batch_safe("x '\"'") && !batch_safe("x 'a^b"));
}

/// Under Windows PowerShell 5.1, a final `\` in a quoted name with a space escapes its
/// closing quote, and the next quoted word is split into arguments: a run with such a name
/// is not offered.
#[test]
fn quoted_names_ending_in_a_backslash_are_not_offered() {
    let d = tree(&[("package.json", r#"{"scripts":{"dev server\\":"vite","a\\b c":"x","ok\\":"x"}}"#)]);
    let pf = win(d.path());
    assert!(!has_run(&pf, r"dev server\"), "{:?}", names(&pf));
    // A `\` elsewhere, or in a name without a space (passed without quotes), arrives as it is.
    assert_eq!(run(&pf, r"a\b c").command, r"npm run 'a\b c'");
    assert_eq!(run(&pf, r"ok\").command, r"npm run ok\");
    // bash reads its quotes itself.
    assert_eq!(run(&detect_checked(d.path()), r"dev server\").command, r"npm run 'dev server\'");

    use super::super::native_quoting_safe;
    assert!(!native_quoting_safe(r"cargo run -p 'x \' --bin 'y --z'") && !native_quoting_safe("x 'a\tb\\'"));
    assert!(native_quoting_safe(r"& '..\.venv\Scripts\my tool.exe' -x") && native_quoting_safe(r"x 'a b\c' 'd\'") && native_quoting_safe("x 'it''s \\ ok'"));
}

#[test]
fn validate_scripts_in_bash_are_not_offered() {
    let d = tree(&[("data/validate.sh", "#!/bin/sh\nexit 0\n"), ("schema/validate.mjs", "process.exit(0)\n")]);
    // `bash` is WSL's on Windows.
    let pf = win(d.path());
    assert!(!has_run(&pf, "validate (data)"), "{:?}", names(&pf));
    assert_eq!(run(&pf, "validate (schema)").command, "node validate.mjs");
    assert_eq!(run(&detect_checked(d.path()), "validate (data)").command, "bash validate.sh");
}

#[test]
fn powershell_builtins_and_posix_syntax() {
    use super::super::posix_only;
    for posix in ["a && b", "a || b", "FOO=1 npm start", "echo $HOME", "x `y`", "sort < in", "ls ~/x", "cmd >/dev/null", "export X=1", "source env", "./run.sh", "bash 'deploy.sh'"] {
        assert!(posix_only(posix), "{posix}");
    }
    for fine in ["npm run dev", "cargo test -p api", "docker compose up -d", "./gradlew build", "python -m http.server 8000", "a; b", "a | b"] {
        assert!(!posix_only(fine), "{fine}");
    }
}
