//! CMake (plain and presets) and JVM (Gradle, Maven) fixtures.

use super::{detect_checked, has_run, names, run, tree};
use crate::apps::output::{ResultParser, TestResult};
use crate::config::project::RunKind;

#[test]
fn cmake_project_without_presets() {
    let d = tree(&[
        ("CMakeLists.txt", "cmake_minimum_required(VERSION 3.20)\nproject(engine CXX)\n\nadd_subdirectory(src)\nadd_subdirectory(tools)\nenable_testing()\nadd_subdirectory(tests)\n"),
        ("src/CMakeLists.txt", "add_library(core STATIC core.cpp)\nadd_executable(engine main.cpp)\ntarget_link_libraries(engine core)\n"),
        ("tools/CMakeLists.txt", "add_executable(asset-packer packer.cpp)\nadd_executable(Engine::alias ALIAS engine)\n"),
        ("tests/CMakeLists.txt", "add_executable(unit_tests test_core.cpp)\nadd_test(NAME unit COMMAND unit_tests)\n"),
        // CLion's build profiles are not walked.
        ("cmake-build-debug/CMakeLists.txt", "project(generated)\nadd_executable(stray x.cpp)\n"),
        ("third_party/fmt/CMakeLists.txt", "cmake_minimum_required(VERSION 3.8)\nproject(fmt)\nadd_executable(fmt-test t.cc)\n"),
    ]);
    let pf = detect_checked(d.path());
    let conf = run(&pf, "cmake configure");
    assert_eq!((conf.kind, conf.command.as_str()), (RunKind::Build, "cmake -S . -B build -DCMAKE_BUILD_TYPE=Debug"));
    let build = run(&pf, "cmake build");
    assert_eq!((build.command.as_str(), build.depends_on.clone()), ("cmake --build build", vec!["cmake configure".to_string()]));
    let ctest = run(&pf, "ctest");
    assert_eq!((ctest.kind, ctest.command.as_str()), (RunKind::Test, "ctest --test-dir build --output-on-failure"));
    assert_eq!(ctest.depends_on, vec!["cmake build"]);
    let engine = run(&pf, "engine");
    assert_eq!(engine.command, "cmake --build build --target engine && ./build/src/engine");
    assert_eq!(engine.source.as_deref(), Some("detected:src/CMakeLists.txt add_executable(engine)"));
    assert_eq!(run(&pf, "asset-packer").command, "cmake --build build --target asset-packer && ./build/tools/asset-packer");
    for absent in ["unit_tests", "Engine::alias", "stray", "fmt-test"] {
        assert!(!has_run(&pf, absent), "{absent}: {:?}", names(&pf));
    }
    assert_eq!(pf.components.iter().filter(|c| c.kind == "cmake").count(), 1);
}

#[test]
fn cmake_runtime_output_directory() {
    let d = tree(&[(
        "CMakeLists.txt",
        "cmake_minimum_required(VERSION 3.16)\nproject(game)\nset(CMAKE_RUNTIME_OUTPUT_DIRECTORY ${CMAKE_BINARY_DIR}/bin)\nadd_executable(game src/main.cpp)\n",
    )]);
    let pf = detect_checked(d.path());
    assert_eq!(run(&pf, "game").command, "cmake --build build --target game && ./build/bin/game");
    assert!(!has_run(&pf, "ctest"), "no tests declared");
}

const PRESETS: &str = r#"{
  "version": 6,
  "configurePresets": [
    { "name": "base", "hidden": true, "generator": "Ninja", "binaryDir": "${sourceDir}/out/build/${presetName}" },
    { "name": "linux-debug", "inherits": "base", "cacheVariables": { "CMAKE_BUILD_TYPE": "Debug" },
      "condition": { "type": "equals", "lhs": "${hostSystemName}", "rhs": "Linux" } },
    { "name": "windows-msvc", "inherits": "base",
      "condition": { "type": "equals", "lhs": "${hostSystemName}", "rhs": "Windows" } },
    { "name": "macos", "inherits": "base",
      "condition": { "type": "inList", "string": "${hostSystemName}", "list": ["Darwin"] } }
  ],
  "buildPresets": [
    { "name": "linux-debug", "configurePreset": "linux-debug" },
    { "name": "windows-msvc", "configurePreset": "windows-msvc" }
  ],
  "testPresets": [
    { "name": "linux-debug", "configurePreset": "linux-debug", "output": { "outputOnFailure": true } }
  ],
  "workflowPresets": [
    { "name": "ci", "steps": [ { "type": "configure", "name": "linux-debug" } ] }
  ]
}"#;

#[test]
fn cmake_presets() {
    let d = tree(&[
        ("CMakeLists.txt", "cmake_minimum_required(VERSION 3.25)\nproject(app)\nadd_executable(app main.cpp)\ninclude(CTest)\n"),
        ("CMakePresets.json", PRESETS),
    ]);
    let pf = detect_checked(d.path());
    assert_eq!(run(&pf, "cmake configure: linux-debug").command, "cmake --preset linux-debug");
    let b = run(&pf, "cmake build: linux-debug");
    assert_eq!((b.command.as_str(), b.depends_on.clone()), ("cmake --build --preset linux-debug", vec!["cmake configure: linux-debug".to_string()]));
    let t = run(&pf, "ctest: linux-debug");
    assert_eq!((t.command.as_str(), t.depends_on.clone()), ("ctest --preset linux-debug --output-on-failure", vec!["cmake build: linux-debug".to_string()]));
    assert_eq!(run(&pf, "cmake workflow: ci").command, "cmake --workflow --preset ci");
    assert_eq!(run(&pf, "app").command, "cmake --build out/build/linux-debug --target app && ./out/build/linux-debug/app");
    for absent in ["cmake configure: windows-msvc", "cmake configure: macos", "cmake build: windows-msvc", "cmake configure: base", "cmake configure"] {
        assert!(!has_run(&pf, absent), "{absent}: {:?}", names(&pf));
    }
}

/// A `release` preset is a build type, not a publication: no confirmation needed.
#[test]
fn cmake_release_preset_is_an_everyday_build() {
    let d = tree(&[
        ("CMakeLists.txt", "cmake_minimum_required(VERSION 3.25)\nproject(app)\n"),
        ("CMakePresets.json", r#"{"version":3,"configurePresets":[{"name":"release","binaryDir":"${sourceDir}/build/release"}],"buildPresets":[{"name":"release","configurePreset":"release"}]}"#),
    ]);
    let pf = detect_checked(d.path());
    for name in ["cmake configure: release", "cmake build: release"] {
        let r = run(&pf, name);
        assert_eq!(r.group.as_deref(), Some("build"), "{name}");
        assert!(!crate::apps::runs::needs_confirmation(r), "{name}");
    }
    assert!(super::super::docs::is_risky("npm run release") && !super::super::docs::is_risky("ctest -C Release"));
}

#[test]
fn ctest_output() {
    let p = ResultParser::new(super::super::CTEST_RESULT).unwrap();
    let mut acc = TestResult::default();
    for l in [
        "    Start 1: parser",
        "1/3 Test #1: parser ...........................   Passed    0.01 sec",
        "2/3 Test #2: lexer ............................***Failed    0.02 sec",
        "3/3 Test #3: codegen ..........................   Passed    0.30 sec",
        "67% tests passed, 1 tests failed out of 3",
    ] {
        p.feed(l, &mut acc);
    }
    assert_eq!((acc.passed, acc.failed), (2, 1));
    assert_eq!(acc.items[1].name, "lexer");
}

#[test]
fn gradle_spring_boot_multi_project() {
    let d = tree(&[
        ("settings.gradle.kts", "rootProject.name = \"shop\"\ninclude(\"app\", \"lib\")\ninclude(\":tools:cli\")\n"),
        ("build.gradle.kts", "plugins {\n    id(\"org.springframework.boot\") version \"3.3.0\" apply false\n    java\n}\n"),
        ("gradlew", "#!/bin/sh\n"),
        ("app/build.gradle.kts", "plugins {\n    id(\"org.springframework.boot\")\n    id(\"io.spring.dependency-management\")\n    java\n}\n"),
        ("app/src/main/resources/application.properties", "spring.application.name=shop\nserver.port=${PORT:8081}\n"),
        ("lib/build.gradle.kts", "plugins { `java-library` }\n"),
        ("tools/cli/build.gradle.kts", "plugins {\n    application\n}\napplication {\n    mainClass.set(\"shop.cli.MainKt\")\n}\n"),
    ]);
    let pf = detect_checked(d.path());
    assert_eq!(run(&pf, "gradle test").command, "./gradlew test");
    assert_eq!(run(&pf, "gradle build").command, "./gradlew build");
    let boot = run(&pf, "bootRun app");
    assert_eq!((boot.kind, boot.command.as_str(), boot.port), (RunKind::Server, "./gradlew :app:bootRun", Some(8081)));
    assert_eq!(boot.ready.as_ref().unwrap().timeout_s, 600);
    assert!(regex::Regex::new(boot.ready.as_ref().unwrap().log.as_deref().unwrap()).unwrap().is_match("Started ShopApplication in 2.345 seconds (process running for 2.8)"));
    assert_eq!(run(&pf, "gradle run tools:cli").command, "./gradlew :tools:cli:run");
    assert!(!has_run(&pf, "bootRun"), "the root only declares the plugin: {:?}", names(&pf));
    assert_eq!(pf.components.iter().filter(|c| c.kind == "gradle").count(), 1, "subprojects belong to the build");
}

#[test]
fn gradle_android_without_wrapper() {
    let d = tree(&[
        ("android/settings.gradle", "include ':app'\n"),
        ("android/app/build.gradle", "plugins {\n    id 'com.android.application'\n}\n"),
    ]);
    let pf = detect_checked(d.path());
    assert_eq!(run(&pf, "gradle assembleDebug (android)").command, "gradle assembleDebug");
    assert!(!has_run(&pf, "gradle build (android)"));
}

#[test]
fn maven_multi_module() {
    let d = tree(&[
        (
            "pom.xml",
            "<project>\n  <packaging>pom</packaging>\n  <modules>\n    <module>core</module>\n    <module>web</module>\n    <!-- <module>legacy</module> -->\n  </modules>\n  <build><pluginManagement><plugins><plugin><artifactId>spring-boot-maven-plugin</artifactId></plugin></plugins></pluginManagement></build>\n</project>\n",
        ),
        ("mvnw", "#!/bin/sh\n"),
        ("core/pom.xml", "<project><artifactId>core</artifactId></project>\n"),
        ("web/pom.xml", "<project><artifactId>web</artifactId><build><plugins><plugin><groupId>org.springframework.boot</groupId><artifactId>spring-boot-maven-plugin</artifactId></plugin></plugins></build></project>\n"),
        ("web/src/main/resources/application.yml", "spring:\n  application:\n    name: web\nserver:\n  port: 9090\n"),
    ]);
    let pf = detect_checked(d.path());
    assert_eq!(run(&pf, "mvn test").command, "./mvnw test");
    assert_eq!(run(&pf, "mvn package").command, "./mvnw package");
    let web = run(&pf, "spring-boot:run web");
    assert_eq!((web.command.as_str(), web.port), ("./mvnw -pl web spring-boot:run", Some(9090)));
    assert!(!has_run(&pf, "spring-boot:run") && !has_run(&pf, "mvn test (web)"), "{:?}", names(&pf));
}

#[test]
fn maven_quarkus_single_module() {
    let d = tree(&[("pom.xml", "<project><build><plugins><plugin><artifactId>quarkus-maven-plugin</artifactId></plugin></plugins></build></project>")]);
    let pf = detect_checked(d.path());
    let q = run(&pf, "quarkus:dev");
    assert_eq!((q.command.as_str(), q.port), ("mvn quarkus:dev", Some(8080)));
}
