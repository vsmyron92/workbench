//! JavaScript extras (deno, bun, turbo, nx, dev-server ports, plain Node servers)
//! and Ruby, PHP, Elixir fixtures.

use super::super::node::{ViteInfo, classify};
use super::{detect_checked, has_run, names, run, tree};
use crate::config::project::RunKind;

#[test]
fn deno_tasks_with_comments() {
    let d = tree(&[
        (
            "deno.jsonc",
            r#"{
  // Fresh app
  "tasks": {
    "start": "deno run -A --watch=static/,routes/ dev.ts",
    "build": "deno run -A dev.ts build",
    "check": { "command": "deno fmt --check && deno lint", "description": "CI checks" },
    "deploy": "deployctl deploy --prod",
  },
  "imports": { "$fresh/": "https://deno.land/x/fresh@1.7.3/", },
}"#,
        ),
        ("dev.ts", "import dev from \"$fresh/dev.ts\";\nawait dev(import.meta.url, \"./main.ts\");\n"),
        ("routes/index_test.ts", "Deno.test(\"x\", () => {});\n"),
    ]);
    let pf = detect_checked(d.path());
    let start = run(&pf, "start");
    assert_eq!((start.kind, start.command.as_str(), start.port), (RunKind::Server, "deno task start", Some(8000)));
    assert_eq!(start.source.as_deref(), Some("detected:deno.jsonc#tasks.start"));
    assert_eq!(run(&pf, "build").kind, RunKind::Build);
    assert_eq!(run(&pf, "check").kind, RunKind::Test);
    assert_eq!(run(&pf, "deploy").group.as_deref(), Some("deploy"));
    assert_eq!(run(&pf, "deno test").command, "deno test -A");
    assert!(pf.project.tags.contains(&"deno".to_string()));
}

#[test]
fn deno_serve_port() {
    let d = tree(&[("deno.json", r#"{"tasks":{"dev":"deno serve --watch main.ts","serve":"deno run --allow-net server.ts"}}"#), ("server.ts", "Deno.serve({ port: 8787 }, () => new Response(\"ok\"));\n")]);
    let pf = detect_checked(d.path());
    assert_eq!(run(&pf, "dev").port, Some(8000));
    assert_eq!(run(&pf, "serve").port, Some(8787));
    assert!(!has_run(&pf, "deno test"));
}

#[test]
fn bun_turbo_monorepo_root() {
    let d = tree(&[
        ("package.json", r#"{"name":"acme","private":true,"workspaces":["apps/*","packages/*"],"scripts":{"dev":"turbo dev","format":"prettier -w ."}}"#),
        ("bun.lock", "{}"),
        (
            "turbo.json",
            r#"{ "$schema": "https://turbo.build/schema.json",
  "tasks": {
    "build": { "dependsOn": ["^build"], "outputs": ["dist/**"] },
    "dev": { "cache": false, "persistent": true },
    "lint": {},
    "test": { "dependsOn": ["build"] },
    "web#build": { "outputs": [".next/**"] },
    "//#check-types": {}
  } }"#,
        ),
        ("apps/web/package.json", r#"{"scripts":{"dev":"next dev --port 3001","build":"next build"}}"#),
        ("packages/ui/package.json", r#"{"scripts":{"lint":"eslint ."}}"#),
    ]);
    let pf = detect_checked(d.path());
    assert_eq!(run(&pf, "dev").command, "bun run dev");
    assert_eq!(run(&pf, "turbo build").command, "bunx turbo run build");
    assert_eq!(run(&pf, "turbo test").kind, RunKind::Test);
    assert_eq!(run(&pf, "turbo lint").kind, RunKind::Task);
    assert!(!has_run(&pf, "turbo dev"), "the root `dev` script already runs it: {:?}", names(&pf));
    assert!(!pf.runs.iter().any(|r| r.name.contains('#')), "{:?}", names(&pf));
    let web = run(&pf, "dev (apps/web)");
    assert_eq!((web.command.as_str(), web.port), ("bun run dev", Some(3001)));
    assert_eq!(run(&pf, "lint (packages/ui)").command, "bun run lint");
}

#[test]
fn nx_workspace_root() {
    let d = tree(&[
        ("package.json", r#"{"name":"org","devDependencies":{"nx":"20.0.0"},"scripts":{"start":"nx serve shop"}}"#),
        ("package-lock.json", "{}"),
        ("nx.json", r#"{"targetDefaults":{"build":{"cache":true},"test":{},"e2e-ci--**/*":{},"@nx/jest:jest":{}}}"#),
    ]);
    let pf = detect_checked(d.path());
    assert_eq!(run(&pf, "nx build").command, "npx nx run-many -t build");
    assert_eq!(run(&pf, "nx test").kind, RunKind::Test);
    assert_eq!(pf.runs.iter().filter(|r| r.name.starts_with("nx ")).count(), 2, "{:?}", names(&pf));
}

#[test]
fn node_server_port_from_its_entry_file() {
    let d = tree(&[
        ("package.json", r#"{"main":"src/index.js","scripts":{"start":"node server.js","dev":"nodemon","watch":"tsx watch src/api.ts","ship":"rsync -az dist/ deploy@203.0.113.5:/srv/site/"}}"#),
        ("server.js", "const express = require('express');\nconst app = express();\nconst PORT = process.env.PORT || 4000;\napp.listen(PORT);\n"),
        ("src/index.js", "require('http').createServer(h).listen(3100);\n"),
        ("src/api.ts", "Bun.serve({ port: 7070, fetch() {} });\n"),
    ]);
    let pf = detect_checked(d.path());
    let start = run(&pf, "start");
    assert_eq!((start.kind, start.port, start.preview.as_deref()), (RunKind::Server, Some(4000), Some("http://localhost:4000/")));
    assert_eq!(run(&pf, "dev").port, Some(3100), "nodemon runs `main`");
    assert_eq!(run(&pf, "watch").port, Some(7070));
    // A script that ships files to a server is a deploy whatever its name.
    let ship = run(&pf, "ship");
    assert_eq!(ship.group.as_deref(), Some("deploy"));
    assert!(crate::apps::runs::needs_confirmation(ship));
}

#[test]
fn more_dev_servers() {
    let v = ViteInfo::default();
    for (name, cmd, port) in [
        ("develop", "gatsby develop", 8000),
        ("start", "docusaurus start", 3000),
        ("storybook", "storybook dev -p 6006", 6006),
        ("dev", "wrangler dev", 8787),
        ("serve", "vue-cli-service serve", 8080),
        ("dev", "remix vite:dev", 5173),
        ("preview", "astro preview", 4321),
        ("start", "serve -s build", 3000),
        ("dev", "parcel index.html", 1234),
    ] {
        let k = classify(name, cmd, &v);
        assert_eq!((k.kind, k.port), (RunKind::Server, Some(port)), "{cmd}");
        assert!(k.ready.is_some(), "{cmd}");
    }
    assert_eq!(classify("build", "parcel build index.html", &v).kind, RunKind::Build);
}

#[test]
fn rails_app() {
    let d = tree(&[
        ("Gemfile", "source \"https://rubygems.org\"\ngem \"rails\", \"~> 7.2\"\ngem \"puma\"\ngroup :test do\n  gem 'rspec-rails'\nend\n"),
        ("config/application.rb", "module Blog\n  class Application < Rails::Application\n  end\nend\n"),
        ("bin/rails", "#!/usr/bin/env ruby\n"),
        ("bin/dev", "#!/usr/bin/env sh\nexec foreman start -f Procfile.dev\n"),
        ("test/models/post_test.rb", ""),
        ("spec/models/post_spec.rb", ""),
    ]);
    let pf = detect_checked(d.path());
    let s = run(&pf, "rails server");
    assert_eq!((s.kind, s.command.as_str(), s.port), (RunKind::Server, "bin/rails server", Some(3000)));
    assert!(regex::Regex::new(s.ready.as_ref().unwrap().log.as_deref().unwrap()).unwrap().is_match("* Listening on http://127.0.0.1:3000"));
    assert_eq!(run(&pf, "bin/dev").port, Some(3000));
    assert_eq!(run(&pf, "rails test").command, "bin/rails test");
    assert_eq!(run(&pf, "rspec").command, "bundle exec rspec");
    assert!(pf.project.tags.contains(&"rails".to_string()));
}

#[test]
fn rack_jekyll_and_rake() {
    let d = tree(&[
        ("api/Gemfile", "gem 'sinatra'\n"),
        ("api/config.ru", "run App\n"),
        ("site/Gemfile", "gem \"jekyll\", \"~> 4.3\"\n"),
        ("site/_config.yml", "title: Docs\n"),
        ("gem/Gemfile", "gemspec\n"),
        ("gem/Rakefile", "require 'rake/testtask'\n"),
        ("gem/test/test_x.rb", ""),
    ]);
    let pf = detect_checked(d.path());
    assert_eq!(run(&pf, "rackup (api)").port, Some(9292));
    assert_eq!(run(&pf, "jekyll serve (site)").command, "bundle exec jekyll serve --livereload");
    assert_eq!(run(&pf, "rake test (gem)").kind, RunKind::Test);
}

#[test]
fn laravel_and_composer_scripts() {
    let d = tree(&[
        (
            "composer.json",
            r#"{"require":{"php":"^8.2","laravel/framework":"^11.0"},"scripts":{
                "post-autoload-dump":["@php artisan package:discover --ansi"],
                "dev":["Composer\\Config::disableProcessTimeout","npx concurrently \"php artisan serve --port=8001\" \"npm run dev\""],
                "test":"@php artisan test",
                "lint":"pint --test",
                "release":"./vendor/bin/envoy run deploy"}}"#,
        ),
        ("artisan", "#!/usr/bin/env php\n<?php\nuse Illuminate\\Foundation\\Application;\n"),
        ("phpunit.xml", "<phpunit/>"),
    ]);
    let pf = detect_checked(d.path());
    let serve = run(&pf, "artisan serve");
    assert_eq!((serve.kind, serve.command.as_str(), serve.port), (RunKind::Server, "php artisan serve", Some(8000)));
    assert_eq!(run(&pf, "artisan test").kind, RunKind::Test);
    assert!(!has_run(&pf, "phpunit"), "Laravel runs PHPUnit through artisan");
    let dev = run(&pf, "composer dev");
    assert_eq!((dev.kind, dev.port, dev.command.as_str()), (RunKind::Server, Some(8001), "composer run-script dev"));
    assert_eq!(run(&pf, "composer test").kind, RunKind::Test);
    assert_eq!(run(&pf, "composer release").group.as_deref(), Some("deploy"));
    assert!(!pf.runs.iter().any(|r| r.name.contains("post-autoload")), "{:?}", names(&pf));
}

#[test]
fn symfony_and_phpunit() {
    let d = tree(&[
        ("composer.json", r#"{"require":{"symfony/framework-bundle":"7.1.*"},"require-dev":{"phpunit/phpunit":"^11"}}"#),
        ("bin/console", "#!/usr/bin/env php\n"),
        ("phpunit.xml.dist", "<phpunit/>"),
    ]);
    let pf = detect_checked(d.path());
    let s = run(&pf, "symfony server");
    assert_eq!((s.kind, s.port), (RunKind::Server, Some(8000)));
    assert_eq!(run(&pf, "phpunit").command, "vendor/bin/phpunit");
}

#[test]
fn phoenix_umbrella() {
    let d = tree(&[
        ("mix.exs", "defmodule Shop.Umbrella.MixProject do\n  def project do\n    [apps_path: \"apps\", deps: []]\n  end\nend\n"),
        ("apps/shop_web/mix.exs", "defp deps do\n  [{:phoenix, \"~> 1.7\"}, {:bandit, \"~> 1.5\"}]\nend\n"),
        ("apps/shop/mix.exs", "defp deps do\n  [{:ecto_sql, \"~> 3.10\"}]\nend\n"),
    ]);
    let pf = detect_checked(d.path());
    let phx = run(&pf, "phx.server");
    assert_eq!((phx.kind, phx.command.as_str(), phx.port), (RunKind::Server, "mix phx.server", Some(4000)));
    let ready = regex::Regex::new(phx.ready.as_ref().unwrap().log.as_deref().unwrap()).unwrap();
    assert!(ready.is_match("[info] Running ShopWeb.Endpoint with Bandit 1.5.7 at 127.0.0.1:4000 (http)"));
    assert_eq!(run(&pf, "mix test").command, "mix test");
    assert_eq!(pf.runs.iter().filter(|r| r.name.starts_with("mix test")).count(), 1, "umbrella apps belong to the root: {:?}", names(&pf));
}

/// Phoenix listens where `config/dev.exs` says (4000 only by default).
#[test]
fn phoenix_port_from_dev_config() {
    use super::super::elixir::endpoint_port;
    let d = tree(&[
        ("mix.exs", "defp deps do\n  [{:phoenix, \"~> 1.7\"}]\nend\n"),
        (
            "config/dev.exs",
            "import Config\n# http: [port: 9999]\nconfig :hello, HelloWeb.Endpoint,\n  http: [ip: {127, 0, 0, 1}, port: 4001],\n  check_origin: false\n",
        ),
    ]);
    let pf = detect_checked(d.path());
    let phx = run(&pf, "phx.server");
    assert_eq!((phx.port, phx.preview.as_deref()), (Some(4001), Some("http://localhost:4001/")));
    assert_eq!(endpoint_port("config :a, A.Endpoint, http: [port: String.to_integer(System.get_env(\"PORT\") || \"4002\")]"), Some(4002));
    assert_eq!(endpoint_port("config :a, A.Endpoint, url: [host: \"localhost\"]"), None);
}
