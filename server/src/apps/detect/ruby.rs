//! Ruby projects (`Gemfile`) → runs: Rails (`bin/dev`, `bin/rails server` :3000,
//! `bin/rails test`), RSpec, Rake tests, Rack apps (`rackup` :9292) and Jekyll
//! sites (`jekyll serve` :4000).

use std::path::Path;

use super::{Ctx, scoped, source};
use crate::config::project::{Component, Ready, RunConfig, RunKind};
use crate::util::os::shell::Dialect;

/// Puma / WEBrick: `* Listening on http://127.0.0.1:3000`, `WEBrick::HTTPServer#start … port=4000`.
const RUBY_READY: &str = r"Listening on (https?://\S+)|Server address: (https?://\S+)|WEBrick::HTTPServer#start";

/// Whether a Gemfile declares `gem "name"` (or `gem 'name'`).
fn has_gem(gemfile: &str, name: &str) -> bool {
    gemfile.lines().any(|l| {
        let t = l.trim_start();
        !t.starts_with('#') && (t.starts_with(&format!("gem \"{name}\"")) || t.starts_with(&format!("gem '{name}'")))
    })
}

pub fn detect(cx: &mut Ctx, f: &Path) {
    let Some(dir) = f.parent() else { return };
    let Some(gemfile) = cx.read(f) else { return };
    let cwd = cx.rel(dir);
    cx.tag("ruby");
    cx.pf.components.push(Component { name: scoped("ruby", &cwd), path: cwd.clone(), kind: "bundler".into(), version: None });
    let server = |name: &str, command: String, port: u16, src: Option<String>| RunConfig {
        name: scoped(name, &cwd),
        kind: RunKind::Server,
        command,
        cwd: cwd.clone(),
        port: Some(port),
        ready: Some(Ready { log: Some(RUBY_READY.into()), http: None, timeout_s: 180 }),
        preview: Some(format!("http://localhost:{port}/")),
        source: src,
        group: Some("dev".into()),
        ..Default::default()
    };
    let rails = dir.join("config/application.rb").is_file() && (has_gem(&gemfile, "rails") || dir.join("bin/rails").is_file());
    // Windows starts no script by its shebang: `ruby bin/rails` there (`super::dialect`).
    let windows = super::dialect() == Dialect::PowerShell;
    let rails_cmd = match (dir.join("bin/rails").is_file(), windows) {
        (true, false) => "bin/rails",
        (true, true) => "ruby bin/rails",
        (false, _) => "bundle exec rails",
    };
    let mut runs: Vec<RunConfig> = vec![];
    if rails {
        cx.tag("rails");
        // Rails 7+: `bin/dev` runs Procfile.dev (server + asset watchers) through foreman.
        // On Windows only a Ruby `bin/dev` (Rails 8) runs; a shell script does not.
        let dev = dir.join("bin/dev");
        if dev.is_file() && !windows {
            runs.push(server("bin/dev", "bin/dev".into(), 3000, source(cx, &dev, "")));
        } else if dev.is_file() && cx.read(&dev).is_some_and(|t| t.lines().next().is_some_and(|l| l.starts_with("#!") && l.contains("ruby"))) {
            runs.push(server("bin/dev", "ruby bin/dev".into(), 3000, source(cx, &dev, "")));
        }
        runs.push(server("rails server", format!("{rails_cmd} server"), 3000, source(cx, &dir.join("config/application.rb"), "")));
        if dir.join("test").is_dir() {
            runs.push(RunConfig {
                name: scoped("rails test", &cwd),
                kind: RunKind::Test,
                command: format!("{rails_cmd} test"),
                cwd: cwd.clone(),
                source: source(cx, &dir.join("test"), "/"),
                group: Some("test".into()),
                ..Default::default()
            });
        }
    } else if dir.join("config.ru").is_file() {
        runs.push(server("rackup", "bundle exec rackup".into(), 9292, source(cx, &dir.join("config.ru"), "")));
    }
    if has_gem(&gemfile, "jekyll") || dir.join("_config.yml").is_file() && gemfile.contains("jekyll") {
        cx.tag("jekyll");
        runs.push(server("jekyll serve", "bundle exec jekyll serve --livereload".into(), 4000, source(cx, f, "")));
    }
    if has_gem(&gemfile, "rspec") || has_gem(&gemfile, "rspec-rails") || dir.join(".rspec").is_file() {
        if dir.join("spec").is_dir() {
            runs.push(RunConfig {
                name: scoped("rspec", &cwd),
                kind: RunKind::Test,
                command: "bundle exec rspec".into(),
                cwd: cwd.clone(),
                source: source(cx, f, " (rspec)"),
                group: Some("test".into()),
                ..Default::default()
            });
        }
    } else if !rails && dir.join("Rakefile").is_file() && dir.join("test").is_dir() {
        runs.push(RunConfig {
            name: scoped("rake test", &cwd),
            kind: RunKind::Test,
            command: "bundle exec rake test".into(),
            cwd: cwd.clone(),
            source: source(cx, &dir.join("Rakefile"), ""),
            group: Some("test".into()),
            ..Default::default()
        });
    }
    for r in runs {
        cx.add_run(r);
    }
}
