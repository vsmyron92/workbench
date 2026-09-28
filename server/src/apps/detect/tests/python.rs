//! Python fixtures: uv, poetry, pdm, hatch, pipenv, plain pip (with and without a
//! virtualenv), Django, FastAPI, Flask, Streamlit, pytest/unittest, ruff, mypy,
//! tox, nox, notebooks and multi-project layouts.

use super::{detect_checked, has_run, names, run, tree, write};
use crate::apps::output::{ResultParser, TestResult};
use crate::config::project::RunKind;

const UV_PYPROJECT: &str = r#"[project]
name = "shop-api"
version = "0.1.0"
requires-python = ">=3.12"
dependencies = ["fastapi>=0.115", "uvicorn[standard]>=0.30", "sqlalchemy"]

[project.scripts]
shop-admin = "shop_api.cli:main"

[dependency-groups]
dev = ["pytest>=8", "ruff", "mypy"]

[tool.pytest.ini_options]
testpaths = ["tests"]

[tool.ruff]
line-length = 100

[tool.mypy]
files = ["app"]
strict = true
"#;

#[test]
fn uv_fastapi_project() {
    let d = tree(&[
        ("pyproject.toml", UV_PYPROJECT),
        ("uv.lock", "version = 1\n"),
        ("app/__init__.py", ""),
        ("app/main.py", "from fastapi import FastAPI\n\napp = FastAPI(title=\"Shop\")\n\n@app.get(\"/health\")\ndef health():\n    return {\"ok\": True}\n"),
        ("app/models.py", "x = 1\n"),
        ("tests/test_health.py", "def test_health():\n    assert True\n"),
    ]);
    let pf = detect_checked(d.path());
    let api = run(&pf, "uvicorn");
    assert_eq!((api.kind, api.port, api.command.as_str()), (RunKind::Server, Some(8000), "uv run uvicorn app.main:app --reload"));
    assert_eq!(api.preview.as_deref(), Some("http://localhost:8000/"));
    assert_eq!(api.source.as_deref(), Some("detected:app/main.py (FastAPI)"));
    assert!(regex::Regex::new(api.ready.as_ref().unwrap().log.as_deref().unwrap()).unwrap().is_match("INFO:     Uvicorn running on http://127.0.0.1:8000 (Press CTRL+C to quit)"));
    let t = run(&pf, "pytest");
    assert_eq!((t.kind, t.command.as_str()), (RunKind::Test, "uv run pytest"));
    assert_eq!(t.result_pattern.as_deref(), Some(super::super::PYTEST_RESULT));
    assert_eq!(run(&pf, "ruff check").command, "uv run ruff check .");
    assert_eq!(run(&pf, "mypy").command, "uv run mypy", "files are configured: no path argument");
    let cli = run(&pf, "shop-admin");
    assert_eq!((cli.kind, cli.command.as_str()), (RunKind::Task, "uv run shop-admin"));
    assert_eq!(cli.source.as_deref(), Some("detected:pyproject.toml#project.scripts.shop-admin"));
    let c = pf.components.iter().find(|c| c.kind == "python").unwrap();
    assert_eq!((c.name.as_str(), c.version.as_deref()), ("python", Some(">=3.12")));
    for t in ["python", "uv", "fastapi"] {
        assert!(pf.project.tags.contains(&t.to_string()), "tag {t}: {:?}", pf.project.tags);
    }
    assert!(!has_run(&pf, "unittest"));
}

#[test]
fn poetry_django_project() {
    let d = tree(&[
        (
            "pyproject.toml",
            "[tool.poetry]\nname = \"blog\"\n\n[tool.poetry.dependencies]\npython = \"^3.11\"\ndjango = \"^5.0\"\n\n[build-system]\nrequires = [\"poetry-core\"]\n",
        ),
        ("poetry.lock", ""),
        ("manage.py", "#!/usr/bin/env python\nimport os\nos.environ.setdefault(\"DJANGO_SETTINGS_MODULE\", \"blog.settings\")\nfrom django.core.management import execute_from_command_line\n"),
        ("blog/settings.py", "DEBUG = True\n"),
        ("blog/asgi.py", "from django.core.asgi import get_asgi_application\napplication = get_asgi_application()\n"),
        ("posts/tests.py", "from django.test import TestCase\n"),
    ]);
    let pf = detect_checked(d.path());
    let rs = run(&pf, "runserver");
    assert_eq!((rs.kind, rs.port, rs.command.as_str()), (RunKind::Server, Some(8000), "poetry run python manage.py runserver"));
    let ready = regex::Regex::new(rs.ready.as_ref().unwrap().log.as_deref().unwrap()).unwrap();
    assert_eq!(&ready.captures("Starting development server at http://127.0.0.1:8000/").unwrap()[1], "http://127.0.0.1:8000/");
    assert_eq!(run(&pf, "django test").command, "poetry run python manage.py test");
    assert!(!has_run(&pf, "pytest") && !has_run(&pf, "uvicorn"), "{:?}", names(&pf));
    assert_eq!(pf.components[0].version.as_deref(), Some("^3.11"));
    assert!(pf.project.tags.contains(&"django".to_string()) && pf.project.tags.contains(&"poetry".to_string()));
}

#[test]
fn django_in_a_subdirectory_with_pytest_django() {
    // requirements at the root, manage.py one level down (`backend/`), pytest-django.
    let d = tree(&[
        ("requirements.txt", "Django==5.1\npsycopg[binary]\n"),
        ("requirements-dev.txt", "-r requirements.txt\npytest-django>=4\n"),
        ("pytest.ini", "[pytest]\nDJANGO_SETTINGS_MODULE = backend.settings\n"),
        ("backend/manage.py", "import django\n"),
        ("docs/requirements.txt", "sphinx\n"),
    ]);
    let pf = detect_checked(d.path());
    let rs = run(&pf, "runserver (backend)");
    assert_eq!((rs.cwd.as_str(), rs.command.as_str()), ("backend", "python3 manage.py runserver"));
    assert!(!has_run(&pf, "django test (backend)"), "pytest-django runs the tests");
    assert_eq!(run(&pf, "pytest").command, "python3 -m pytest");
    assert_eq!(pf.components.iter().filter(|c| c.kind == "python").count(), 1, "docs/requirements.txt belongs to the project");
}

/// cookiecutter-django's layout: `requirements/{base,local}.txt`, settings in
/// `config/`, pytest-django configured in pyproject.
#[test]
fn requirements_directory() {
    let d = tree(&[
        ("requirements/base.txt", "django==5.0.7\n"),
        ("requirements/local.txt", "-r base.txt\npytest-django==4.8.0\n"),
        ("manage.py", "os.environ.setdefault(\"DJANGO_SETTINGS_MODULE\", \"config.settings.local\")\n"),
    ]);
    let pf = detect_checked(d.path());
    assert_eq!(run(&pf, "runserver").command, "python3 manage.py runserver");
    assert_eq!(run(&pf, "pytest").command, "python3 -m pytest");
    assert!(!has_run(&pf, "django test"), "pytest-django runs the tests: {:?}", names(&pf));
    assert_eq!(pf.components.iter().filter(|c| c.kind == "python").map(|c| c.path.as_str()).collect::<Vec<_>>(), vec!["."]);
}

#[test]
fn plain_pip_flask_with_a_virtualenv() {
    let d = tree(&[
        ("requirements.txt", "flask>=3\ngunicorn\npytest\n"),
        ("app.py", "from flask import Flask\n\napp = Flask(__name__)\n\nif __name__ == \"__main__\":\n    app.run(debug=True, port=5050)\n"),
        (".venv/pyvenv.cfg", "home = /usr/bin\n"),
        (".venv/bin/python", ""),
        ("tests/test_app.py", "def test_x(): pass\n"),
    ]);
    let pf = detect_checked(d.path());
    let f = run(&pf, "flask run");
    assert_eq!(f.command, ".venv/bin/python -m flask --app app run --debug --port 5050");
    assert_eq!((f.kind, f.port), (RunKind::Server, Some(5050)));
    assert_eq!(run(&pf, "pytest").command, ".venv/bin/python -m pytest");
    assert!(pf.project.tags.contains(&"pip".to_string()) && pf.project.tags.contains(&"flask".to_string()));
}

#[test]
fn flask_factory_in_a_src_layout_and_unittest() {
    let d = tree(&[
        ("setup.py", "from setuptools import setup\nsetup(name='wiki', install_requires=['flask'])\n"),
        ("src/wiki/__init__.py", "from flask import Flask\n\ndef create_app():\n    app = Flask(__name__)\n    return app\n"),
        ("tests/test_pages.py", "import unittest\n"),
    ]);
    let pf = detect_checked(d.path());
    let f = run(&pf, "flask run");
    assert_eq!(f.command, "python3 -m flask --app wiki run --debug");
    assert_eq!(f.env.get("PYTHONPATH").map(String::as_str), Some("src"));
    let u = run(&pf, "unittest");
    assert_eq!((u.kind, u.command.as_str()), (RunKind::Test, "python3 -m unittest discover -s tests"));
}

#[test]
fn src_layout_fastapi_factory_and_task_runners() {
    let d = tree(&[
        (
            "pyproject.toml",
            r#"[project]
name = "orders"
dependencies = ["litestar", "uvicorn"]

[tool.pdm.scripts]
_ = {env_file = ".env"}
serve = "uvicorn orders.app:app --port 9000"
lint = {cmd = "ruff check src"}
pre_install = "echo hook"
publish-docs = {shell = "mkdocs gh-deploy"}

[tool.poe.tasks]
fmt = "ruff format ."
"#,
        ),
        ("pdm.lock", ""),
        ("src/orders/__init__.py", ""),
        ("src/orders/app.py", "from litestar import Litestar\n\ndef create_app() -> Litestar:\n    return Litestar(route_handlers=[])\n"),
    ]);
    let pf = detect_checked(d.path());
    let api = run(&pf, "uvicorn");
    assert_eq!(api.command, "pdm run uvicorn --factory orders.app:create_app --reload --app-dir src");
    let serve = run(&pf, "serve");
    assert_eq!((serve.kind, serve.command.as_str(), serve.port), (RunKind::Server, "pdm run serve", Some(9000)));
    assert_eq!(run(&pf, "lint").command, "pdm run lint");
    assert_eq!(run(&pf, "fmt").command, "pdm run poe fmt");
    assert_eq!(run(&pf, "publish-docs").group.as_deref(), Some("deploy"), "gh-deploy publishes");
    assert!(!has_run(&pf, "_") && !has_run(&pf, "pre_install"), "{:?}", names(&pf));
}

#[test]
fn hatch_pipenv_tox_nox_and_streamlit() {
    let d = tree(&[
        (
            "lib/pyproject.toml",
            "[project]\nname = \"lib\"\n\n[tool.hatch.envs.default.scripts]\ntest = \"pytest {args}\"\ncov = [\"coverage run -m pytest\", \"coverage report\"]\n\n[tool.hatch.build]\npackages = [\"src/lib\"]\n",
        ),
        ("lib/tox.ini", "[tox]\nenvlist = py312\n"),
        ("lib/noxfile.py", "import nox\n@nox.session\ndef tests(session): ...\n"),
        ("dash/Pipfile", "[packages]\nstreamlit = \"*\"\n\n[scripts]\nreport = \"python report.py\"\n"),
        ("dash/streamlit_app.py", "import streamlit as st\nst.title('Sales')\n"),
    ]);
    let pf = detect_checked(d.path());
    assert_eq!(run(&pf, "test (lib)").command, "hatch run test");
    assert_eq!(run(&pf, "cov (lib)").kind, RunKind::Test);
    assert_eq!(run(&pf, "tox (lib)").command, "tox");
    assert_eq!(run(&pf, "nox (lib)").command, "nox");
    let st = run(&pf, "streamlit (dash)");
    assert_eq!((st.command.as_str(), st.port), ("pipenv run streamlit run streamlit_app.py", Some(8501)));
    let rep = run(&pf, "report (dash)");
    assert_eq!((rep.command.as_str(), rep.source.as_deref()), ("pipenv run report", Some("detected:dash/Pipfile#scripts.report")));
}

#[test]
fn nested_python_services_are_separate_projects() {
    let d = tree(&[
        ("pyproject.toml", "[tool.uv.workspace]\nmembers = [\"services/*\"]\n[tool.ruff]\n"),
        ("uv.lock", ""),
        ("services/api/pyproject.toml", "[project]\nname = \"api\"\ndependencies = [\"fastapi\"]\n"),
        ("services/api/main.py", "import fastapi\napp = fastapi.FastAPI()\n"),
        ("services/worker/pyproject.toml", "[project]\nname = \"worker\"\n[project.scripts]\nworker = \"worker:main\"\n"),
        ("services/worker/tests/conftest.py", ""),
    ]);
    let pf = detect_checked(d.path());
    assert_eq!(run(&pf, "uvicorn (services/api)").command, "uv run uvicorn main:app --reload", "the workspace lock applies");
    assert_eq!(run(&pf, "worker (services/worker)").command, "uv run worker");
    assert_eq!(run(&pf, "pytest (services/worker)").command, "uv run pytest");
    assert_eq!(run(&pf, "ruff check").cwd, ".");
    assert!(!has_run(&pf, "uvicorn"), "the root is not an app: {:?}", names(&pf));
}

#[test]
fn notebooks_are_components() {
    let d = tree(&[
        ("analysis/explore.ipynb", "{}"),
        ("analysis/model.ipynb", "{}"),
        ("analysis/.ipynb_checkpoints/explore-checkpoint.ipynb", "{}"),
        ("reports/q3.ipynb", "{}"),
    ]);
    let pf = detect_checked(d.path());
    let nb: Vec<&str> = pf.components.iter().filter(|c| c.kind == "jupyter").map(|c| c.path.as_str()).collect();
    assert_eq!(nb, vec!["analysis", "reports"]);
    assert!(pf.project.tags.contains(&"jupyter".to_string()));
    assert!(pf.runs.is_empty(), "notebooks are noted, not run");
}

#[test]
fn pytest_summary_lines() {
    let p = ResultParser::new(super::super::PYTEST_RESULT).unwrap();
    let parse = |lines: &[&str]| {
        let mut acc = TestResult::default();
        for l in lines {
            p.feed(l, &mut acc);
        }
        (acc.passed, acc.failed)
    };
    assert_eq!(parse(&["tests/test_a.py ..F.  [100%]", "===== 1 failed, 3 passed, 2 skipped, 1 warning in 0.52s ====="]), (3, 1));
    assert_eq!(parse(&["============================== 45 passed in 1.20s ==============================="]), (45, 0));
    assert_eq!(parse(&["========= 2 failed in 65.43s (0:01:05) ========="]), (0, 2));
    assert_eq!(parse(&["collected 3 items", "short test summary info"]), (0, 0));
}

#[test]
fn python_scripts_without_a_project_only_tag() {
    // Blender or tooling scripts (a game's Tools/ folder): no project, no runs.
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "Tools/blender_sheet.py", "import bpy\n");
    let pf = detect_checked(d.path());
    assert!(pf.runs.is_empty() && pf.components.is_empty());
    assert!(pf.project.tags.contains(&"python".to_string()));
}
