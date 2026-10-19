//! The slice through its real entry points: scans of scratch folders laid out the way each CLI lays
//! its own, and the REST routes over a full in-process app.

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{Method, Request};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::catalog;
use super::discover::{Account, Cli, Discovery, Inputs, Limits, Scope, SkillInfo, discover, git_root_of};
use crate::app::AppState;
use crate::config::{GlobalConfig, Paths};
use crate::mcp::McpCtx;

// ---------------------------------------------------------------- scratch folders

fn w(path: impl AsRef<Path>, text: &str) {
    let p = path.as_ref();
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn skill_md(name: &str, description: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n\nBody of {name}.\n")
}

struct Fx {
    _t: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
}

/// A home folder and a project (a repository of its own) under one scratch folder.
fn fx() -> Fx {
    let t = tempfile::tempdir().unwrap();
    let base = crate::util::os::path::canonicalize(t.path()).unwrap();
    let (home, root) = (base.join("home"), base.join("work").join("shop"));
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    Fx { _t: t, home, root }
}

fn acct(id: &str, cli: Cli, dir: PathBuf) -> Account {
    Account { id: id.into(), cli, label: id.into(), enabled: true, installed: true, dir, mcp: matches!(cli, Cli::Claude | Cli::Codex) }
}

fn inputs(f: &Fx, accounts: Vec<Account>) -> Inputs {
    Inputs {
        home: f.home.clone(),
        git_root: git_root_of(&f.root),
        root: f.root.clone(),
        accounts,
        managed_claude: f.home.join("etc/claude-code"),
        admin_codex: Some(f.home.join("etc/codex")),
        limits: Limits::default(),
    }
}

fn find<'a>(d: &'a Discovery, qualified: &str) -> &'a SkillInfo {
    d.skills.iter().find(|s| s.qualified == qualified).unwrap_or_else(|| panic!("no skill {qualified:?} in {:?}", d.skills.iter().map(|s| &s.qualified).collect::<Vec<_>>()))
}

fn has(d: &Discovery, qualified: &str) -> bool {
    d.skills.iter().any(|s| s.qualified == qualified)
}

// ---------------------------------------------------------------- Claude Code

fn claude_fixture() -> (Fx, PathBuf) {
    let f = fx();
    let claude = f.home.join(".claude");
    // Personal skills, one in the trash, one in claude.ai's synced folder.
    w(claude.join("skills/personal-one/SKILL.md"), "---\nname: personal-one\ndescription: >\n  Does the personal thing.\n  Use when asked.\n---\n# Personal\n");
    w(claude.join("skills/quiet/SKILL.md"), &skill_md("quiet", "Quiet skill"));
    w(claude.join("skills/gone/SKILL.md"), &skill_md("gone", "Turned off"));
    w(claude.join("skills/dup/SKILL.md"), &skill_md("dup", "Personal dup"));
    w(claude.join("skills/.trash/20260101/old/SKILL.md"), &skill_md("old", "In the trash"));
    w(claude.join("skills/synced/bucket-1/docx/SKILL.md"), &skill_md("docx", "Word documents"));
    w(claude.join("skills/synced/bucket-1/manifest.json"), "{}");
    // Commands.
    w(claude.join("commands/ship.md"), "---\ndescription: Ship it\n---\nShip.\n");
    w(claude.join("commands/frontend/component.md"), "Make a component.\n");
    // Plugins: the registry says which version is installed; the cache keeps old ones.
    let cache = claude.join("plugins/cache/mp");
    w(cache.join("stripe/1.0/.claude-plugin/plugin.json"), r#"{"name":"stripe"}"#);
    w(cache.join("stripe/1.0/skills/pay/SKILL.md"), &skill_md("pay", "Take payments"));
    w(cache.join("stripe/1.0/commands/explain-error.md"), "---\ndescription: Explain an error\n---\nx\n");
    w(cache.join("stripe/0.9/skills/old/SKILL.md"), &skill_md("old", "An old version"));
    w(cache.join("off/1.0/skills/sleepy/SKILL.md"), &skill_md("sleepy", "A disabled plugin's"));
    w(f.home.join("elsewhere/evil/skills/evil/SKILL.md"), &skill_md("evil", "Outside the plugin folder"));
    w(cache.join("other/1.0/skills/foreign/SKILL.md"), &skill_md("foreign", "Installed for another project"));
    let install = |p: &Path| p.to_string_lossy().replace('\\', "/");
    w(
        claude.join("plugins/installed_plugins.json"),
        &json!({"version": 2, "plugins": {
            "stripe@mp": [{"scope": "user", "installPath": install(&cache.join("stripe/1.0")), "version": "1.0"}],
            "off@mp": [{"scope": "user", "installPath": install(&cache.join("off/1.0"))}],
            "evil@mp": [{"scope": "user", "installPath": install(&f.home.join("elsewhere/evil"))}],
            "other@mp": [{"scope": "project", "projectPath": "/somewhere/else", "installPath": install(&cache.join("other/1.0"))}],
        }})
        .to_string(),
    );
    w(
        claude.join("settings.json"),
        &json!({"enabledPlugins": {"stripe@mp": true, "off@mp": false, "evil@mp": true, "other@mp": true}, "skillOverrides": {"quiet": "name-only", "gone": "off"}}).to_string(),
    );
    // The project: its own skills, one hidden by the personal one of the same name, nested ones.
    w(f.root.join(".claude/skills/proj-skill/SKILL.md"), &skill_md("proj-skill", "The project's own"));
    w(f.root.join(".claude/skills/dup/SKILL.md"), &skill_md("dup", "Project dup"));
    w(f.root.join("apps/web/.claude/skills/deploy/SKILL.md"), &skill_md("deploy", "Deploy the web app"));
    w(f.root.join("node_modules/pkg/.claude/skills/bad/SKILL.md"), &skill_md("bad", "In a dependency"));
    // An organization's.
    w(f.home.join("etc/claude-code/.claude/skills/org-policy/SKILL.md"), &skill_md("org-policy", "Organization policy"));
    (f, claude)
}

#[test]
fn claude_code_skills_come_from_every_place_it_reads_and_only_those() {
    let (f, claude) = claude_fixture();
    let d = discover(&inputs(&f, vec![acct("claude", Cli::Claude, claude)]));
    let q = |s: &str| find(&d, s);

    let p = q("personal-one");
    assert_eq!((p.scope, p.kind, p.status, p.description.as_str()), (Scope::User, "skill", "active", "Does the personal thing. Use when asked."));
    assert_eq!((p.agents.clone(), p.invoke.iter().map(|i| i.text.as_str()).collect::<Vec<_>>()), (vec![Cli::Claude], vec!["/personal-one"]));
    assert!(p.tokens > 0 && p.issues.is_empty(), "{:?}", p.issues);
    assert_eq!(q("quiet").status, "name-only");
    assert!(q("quiet").tokens < q("personal-one").tokens, "a name-only skill costs less");
    assert_eq!(q("gone").status, "disabled");
    assert_eq!(q("gone").tokens, 0);
    assert!(!has(&d, "old"), "the trash and old plugin versions are not skills");

    // The same name in the project is hidden by the personal one.
    let dups: Vec<(Scope, &str)> = d.skills.iter().filter(|s| s.qualified == "dup").map(|s| (s.scope, s.status)).collect();
    assert_eq!(dups, [(Scope::Project, "shadowed"), (Scope::User, "active")]);
    assert_eq!(q("proj-skill").scope, Scope::Project);
    assert_eq!(q("org-policy").scope, Scope::Managed);

    // claude.ai's synced skills carry their namespace; commands are listed beside skills.
    assert_eq!((q("anthropic-skills:docx").scope, q("anthropic-skills:docx").kind), (Scope::Synced, "skill"));
    assert_eq!((q("ship").kind, q("ship").description.as_str()), ("command", "Ship it"));
    assert_eq!(q("frontend:component").kind, "command");
    assert_eq!(q("frontend:component").description, "Make a component.", "no description: the first line of the text");
    assert!(q("frontend:component").description_from_text);

    // Plugins by the registry only, enabled by the settings, inside Claude's own folder.
    assert_eq!((q("stripe:pay").scope, q("stripe:pay").scope_label.as_deref(), q("stripe:pay").status), (Scope::Plugin, Some("stripe"), "active"));
    assert_eq!((q("stripe:explain-error").kind, q("stripe:explain-error").status), ("command", "active"));
    assert_eq!(q("off:sleepy").status, "disabled");
    assert!(q("off:sleepy").status_reason.as_deref().unwrap().contains("not enabled"));
    assert!(!d.skills.iter().any(|s| s.name == "evil" || s.name == "foreign"), "a plugin outside the plugin folder, or for another project, is not read");

    // Nested skills are namespaced by their folder; dependencies are not searched.
    assert_eq!((q("apps/web:deploy").scope, q("apps/web:deploy").scope_label.as_deref()), (Scope::Nested, Some("apps/web")));
    assert!(!d.skills.iter().any(|s| s.name == "bad"));

    // What a session carries: active listed skills, not the nested, disabled or hidden ones.
    let a = &d.accounts[0];
    assert!(a.listing_tokens > 0 && a.session.is_none());
    let listed: u64 = d.skills.iter().filter(|s| s.scope != Scope::Nested).map(|s| s.tokens).sum();
    assert!(a.listing_tokens >= listed.saturating_sub(d.skills.len() as u64) && a.listing_tokens <= listed + d.skills.len() as u64, "{} vs {listed}", a.listing_tokens);
    assert!(d.skills.iter().all(|s| s.in_session.is_none()), "no transcript: nothing to compare with");
}

#[test]
fn the_newest_sessions_listing_adds_built_in_skills_and_says_what_was_loaded() {
    let (f, claude) = claude_fixture();
    let dir = crate::terminals::claude_project_dir(&claude, &f.root);
    // Workbench's own skill was installed when the session began and removed since: Claude Code's list still has it.
    let removed = catalog::bundled()[0].name.clone();
    let names = ["personal-one", "stripe:pay", "init", "loop", removed.as_str()];
    let content = format!("- personal-one: Does the personal thing.\n- stripe:pay: Take payments\n- init: Start a CLAUDE.md\n- loop: Run a prompt on an interval\n- {removed}: Taught how to use Workbench");
    let line = json!({"type": "attachment", "timestamp": "2026-10-16T05:00:00Z", "version": "2.1.301",
        "attachment": {"type": "skill_listing", "content": content, "skillCount": 5, "isInitial": true, "names": names}});
    w(dir.join("s1.jsonl"), &format!("{{\"type\":\"user\"}}\n{line}\n"));
    let d = discover(&inputs(&f, vec![acct("claude", Cli::Claude, claude)]));

    assert_eq!(find(&d, "personal-one").in_session, Some(true));
    assert_eq!(find(&d, "proj-skill").in_session, Some(false), "listed by no session: it was added since, or Claude Code left it out");
    // A hidden skill and a nested one say nothing about the session.
    let hidden: Vec<Option<bool>> = d.skills.iter().filter(|s| s.qualified == "dup").map(|s| s.in_session).collect();
    assert_eq!(hidden.len(), 2);
    assert!(d.skills.iter().filter(|s| s.qualified == "dup" && s.status == "shadowed").all(|s| s.in_session.is_none()));
    assert!(find(&d, "apps/web:deploy").in_session.is_none());
    let init = find(&d, "init");
    assert_eq!((init.kind, init.scope, init.description.as_str(), init.path.as_str()), ("builtin", Scope::Builtin, "Start a CLAUDE.md", ""));
    assert!(has(&d, "loop") && !has(&d, "stripe:pay-builtin"));
    assert!(d.skills.iter().filter(|s| s.kind == "builtin").count() == 2, "only what no folder explains");
    assert!(!has(&d, &removed), "a skill of Workbench's that was removed since is not one of Claude Code's");
    // A skill that is turned off is not meant to be in the listing: the session says nothing about it.
    assert_eq!(find(&d, "gone").status, "disabled");
    assert!(find(&d, "gone").in_session.is_none());
    let note = d.accounts[0].session.as_ref().unwrap();
    assert_eq!((note.count, note.chars, note.version.as_deref()), (5, content.chars().count(), Some("2.1.301")));
    // The built-in ones are part of what every session carries.
    assert!(d.accounts[0].listing_tokens >= init.tokens + find(&d, "loop").tokens);
}

#[test]
fn a_second_claude_account_has_its_own_personal_skills_and_shares_the_projects() {
    let f = fx();
    let (a, b) = (f.home.join(".claude"), f.home.join(".claude-work"));
    w(a.join("skills/mine/SKILL.md"), &skill_md("mine", "Account a's"));
    w(b.join("skills/theirs/SKILL.md"), &skill_md("theirs", "Account b's"));
    w(f.root.join(".claude/skills/shared/SKILL.md"), &skill_md("shared", "Both read it"));
    let d = discover(&inputs(&f, vec![acct("claude", Cli::Claude, a), acct("claude-work", Cli::Claude, b)]));
    assert_eq!(find(&d, "mine").accounts, ["claude"]);
    assert_eq!(find(&d, "theirs").accounts, ["claude-work"]);
    assert_eq!(find(&d, "shared").accounts, ["claude", "claude-work"]);
    assert_eq!((d.accounts[0].skills, d.accounts[1].skills), (2, 2));
    // Two accounts that share one folder read one skill once.
    let d = discover(&inputs(&f, vec![acct("one", Cli::Claude, f.home.join(".claude")), acct("two", Cli::Claude, f.home.join(".claude"))]));
    assert_eq!(d.skills.iter().filter(|s| s.qualified == "mine").count(), 1);
    assert_eq!(find(&d, "mine").accounts, ["one", "two"]);
}

#[test]
fn accounts_never_hide_or_turn_off_each_others_skills() {
    let f = fx();
    let (a, b) = (f.home.join(".claude"), f.home.join(".claude-work"));
    // The same personal skill in both accounts, as installing one of Workbench's into each makes: no account sees the other's.
    w(a.join("skills/twin/SKILL.md"), &skill_md("twin", "Account a's twin"));
    w(b.join("skills/twin/SKILL.md"), &skill_md("twin", "Account b's twin"));
    // A project skill that a's personal skill of the name hides, and that b still lists.
    w(f.root.join(".claude/skills/dup/SKILL.md"), &skill_md("dup", "Project dup"));
    w(a.join("skills/dup/SKILL.md"), &skill_md("dup", "Personal dup of a"));
    // A project skill that b's settings turn off, and a still lists; one that both turn off.
    w(f.root.join(".claude/skills/shared/SKILL.md"), &skill_md("shared", "Both read it"));
    w(f.root.join(".claude/skills/nobody/SKILL.md"), &skill_md("nobody", "Off for both"));
    w(a.join("settings.json"), r#"{"skillOverrides": {"nobody": "off"}}"#);
    w(b.join("settings.json"), r#"{"skillOverrides": {"shared": "off", "nobody": "off"}}"#);
    // One that a's personal skill and b's both hide.
    w(f.root.join(".claude/skills/covered/SKILL.md"), &skill_md("covered", "Hidden for both"));
    w(a.join("skills/covered/SKILL.md"), &skill_md("covered", "a's"));
    w(b.join("skills/covered/SKILL.md"), &skill_md("covered", "b's"));
    let d = discover(&inputs(&f, vec![acct("a", Cli::Claude, a), acct("b", Cli::Claude, b)]));
    let of = |q: &str, scope: Scope| d.skills.iter().find(|s| s.qualified == q && s.scope == scope).unwrap_or_else(|| panic!("no {q} of {scope:?}"));
    let said = |s: &SkillInfo, what: &str| s.issues.iter().any(|i| i.code == "per_account" && i.message.contains(what));

    for (q, who) in [("twin", "a"), ("twin", "b"), ("covered", "a"), ("covered", "b")] {
        let s = d.skills.iter().find(|s| s.qualified == q && s.scope == Scope::User && s.accounts == [who]).unwrap_or_else(|| panic!("{q} of {who}"));
        assert_eq!(s.status, "active", "{q} of {who}: its own account's skill is never hidden by another account's");
    }
    // Hidden for a only: still listed, for b.
    let dup = of("dup", Scope::Project);
    assert_eq!((dup.status, dup.accounts.clone()), ("active", vec!["a".to_string(), "b".into()]));
    assert!(said(dup, "Hidden for a"), "{:?}", dup.issues);
    // Off for b only: still listed, for a.
    let shared = of("shared", Scope::Project);
    assert_eq!(shared.status, "active");
    assert!(said(shared, "Off for b"), "{:?}", shared.issues);
    assert!(shared.tokens > 0 && !shared.flags.manual_only);
    // Off for both, or hidden for both: it is what it says, with no remark about who differs.
    let nobody = of("nobody", Scope::Project);
    assert_eq!((nobody.status, nobody.tokens), ("disabled", 0));
    assert!(!nobody.issues.iter().any(|i| i.code == "per_account"));
    let covered = of("covered", Scope::Project);
    assert_eq!(covered.status, "shadowed");
    assert!(covered.issues.iter().any(|i| i.code == "shadowed"));

    // What each account has and pays for follows its own view. a: twin, personal dup, shared, covered = 4; b: twin, dup, covered = 3.
    let (ra, rb) = (&d.accounts[0], &d.accounts[1]);
    assert_eq!((ra.skills, rb.skills), (4, 3), "{ra:?} {rb:?}");
    assert!(ra.listing_tokens > rb.listing_tokens);
}

#[test]
fn a_user_invocable_only_override_is_one_accounts_and_not_the_skills() {
    let f = fx();
    let (a, b) = (f.home.join(".claude"), f.home.join(".claude-work"));
    w(f.root.join(".claude/skills/pick/SKILL.md"), &skill_md("pick", "Both read it"));
    w(a.join("settings.json"), r#"{"skillOverrides": {"pick": "user-invocable-only"}}"#);
    std::fs::create_dir_all(&b).unwrap();
    let d = discover(&inputs(&f, vec![acct("a", Cli::Claude, a), acct("b", Cli::Claude, b)]));
    let pick = find(&d, "pick");
    assert!(!pick.flags.manual_only && pick.tokens > 0, "b still lists it to its model");
    // a does not carry it in every session; b does.
    assert!(d.accounts[0].listing_tokens < d.accounts[1].listing_tokens, "{:?}", d.accounts);
    // One account starts it by hand and the other has it off: no session lists it, whatever the skill's own status says.
    let b3 = f.home.join(".claude-work");
    w(b3.join("settings.json"), r#"{"skillOverrides": {"pick": "off"}}"#);
    let d = discover(&inputs(&f, vec![acct("a", Cli::Claude, f.home.join(".claude")), acct("b", Cli::Claude, b3)]));
    assert_eq!((find(&d, "pick").tokens, d.accounts[0].listing_tokens, d.accounts[1].listing_tokens), (0, 0, 0));
    // With both accounts set so, it is the skill's: nobody lists it.
    let b2 = f.home.join(".claude-work");
    w(b2.join("settings.json"), r#"{"skillOverrides": {"pick": "user-invocable-only"}}"#);
    let d = discover(&inputs(&f, vec![acct("a", Cli::Claude, f.home.join(".claude")), acct("b", Cli::Claude, b2)]));
    assert!(find(&d, "pick").flags.manual_only);
    assert_eq!((find(&d, "pick").tokens, d.accounts[0].listing_tokens), (0, 0));
}

// ---------------------------------------------------------------- the other CLIs

#[test]
fn codex_gemini_and_kimi_read_their_own_folders_and_share_agents_skills() {
    let f = fx();
    // A project below the repository's top: `.agents/skills` counts at every level for Codex.
    let repo = f.root.parent().unwrap().join("mono");
    let root = repo.join("services/api");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(&root).unwrap();
    let f = Fx { _t: f._t, home: f.home, root };
    let (codex, gemini, kimi) = (f.home.join(".codex"), f.home.join(".gemini"), f.home.join(".kimi-code"));
    let s = |n: &str| skill_md(n, &format!("The {n} skill"));

    w(codex.join("skills/c-user/SKILL.md"), &s("c-user"));
    w(codex.join("skills/group/nested/SKILL.md"), &s("nested"));
    w(codex.join("skills/.system/sys-skill/SKILL.md"), &s("sys-skill"));
    w(f.home.join(".agents/skills/shared/SKILL.md"), &s("shared"));
    w(f.home.join(".agents/skills/deep/inner/SKILL.md"), &s("inner"));
    w(repo.join(".agents/skills/top/SKILL.md"), &s("top"));
    w(f.root.join(".agents/skills/api-shared/SKILL.md"), &s("api-shared"));
    w(f.root.join(".codex/skills/codex-only/SKILL.md"), &s("codex-only"));
    w(f.root.join(".gemini/skills/gem/SKILL.md"), &s("gem"));
    w(repo.join(".kimi-code/skills/kimi-top/SKILL.md"), &s("kimi-top"));
    w(repo.join(".kimi-code/skills/loose.md"), "---\ndescription: A loose file\n---\nLoose.\n");
    w(gemini.join("skills/g-user/SKILL.md"), &s("g-user"));
    w(kimi.join("skills/k-user/SKILL.md"), &s("k-user"));
    w(kimi.join("config.toml"), "extra_skill_dirs = [\"~/extra-skills\"]\n");
    w(f.home.join("extra-skills/xtra/SKILL.md"), &s("xtra"));
    // Codex's config turns one off by the path of its file.
    w(codex.join("config.toml"), &format!("[[skills.config]]\npath = '{}'\nenabled = false\n", codex.join("skills/c-user/SKILL.md").display()));

    let d = discover(&inputs(&f, vec![acct("codex", Cli::Codex, codex), acct("gemini", Cli::Gemini, gemini), acct("kimi", Cli::Kimi, kimi)]));
    let agents = |n: &str| find(&d, n).agents.clone();

    assert_eq!(agents("shared"), [Cli::Codex, Cli::Gemini, Cli::Kimi], "~/.agents/skills is the one personal folder the three share");
    assert_eq!(agents("inner"), [Cli::Codex], "only Codex searches below the first level");
    assert_eq!(agents("nested"), [Cli::Codex]);
    assert_eq!(find(&d, "sys-skill").scope, Scope::System);
    assert_eq!(agents("top"), [Cli::Codex, Cli::Kimi], "the repository's top: Codex and Kimi Code, not Gemini CLI (it reads the project folder only)");
    assert_eq!(agents("api-shared"), [Cli::Codex, Cli::Gemini], "Kimi Code reads the repository's top, not a folder below it");
    assert_eq!((agents("codex-only"), agents("gem"), agents("kimi-top")), (vec![Cli::Codex], vec![Cli::Gemini], vec![Cli::Kimi]));
    assert_eq!((agents("g-user"), agents("k-user")), (vec![Cli::Gemini], vec![Cli::Kimi]));
    assert_eq!((find(&d, "xtra").scope, agents("xtra")), (Scope::Extra, vec![Cli::Kimi]));
    assert_eq!(find(&d, "loose").description, "A loose file", "Kimi Code also takes loose Markdown files");
    // Codex turns one off by its path; the others do not know about that.
    assert_eq!((find(&d, "c-user").status, find(&d, "shared").status), ("disabled", "active"));
    // How each is started.
    let shared = find(&d, "shared");
    let invoke: Vec<(Cli, &str)> = shared.invoke.iter().map(|i| (i.agent, i.text.as_str())).collect();
    assert_eq!(invoke, [(Cli::Codex, "$shared"), (Cli::Kimi, "/skill:shared")], "Gemini CLI has the model activate it: no command");
}

#[test]
fn a_name_used_twice_is_resolved_by_each_clis_own_precedence() {
    let f = fx();
    let kimi = f.home.join(".kimi-code");
    w(kimi.join("skills/both/SKILL.md"), &skill_md("both", "Personal"));
    w(f.root.join(".kimi-code/skills/both/SKILL.md"), &skill_md("both", "Project"));
    let d = discover(&inputs(&f, vec![acct("kimi", Cli::Kimi, kimi)]));
    let got: Vec<(Scope, &str)> = d.skills.iter().filter(|s| s.name == "both").map(|s| (s.scope, s.status)).collect();
    assert_eq!(got, [(Scope::Project, "active"), (Scope::User, "shadowed")], "Kimi Code: the project over the personal");

    // Codex lists them side by side: both stay, with a remark.
    let codex = f.home.join(".codex");
    w(codex.join("skills/twin/SKILL.md"), &skill_md("twin", "One"));
    w(f.root.join(".agents/skills/twin/SKILL.md"), &skill_md("twin", "Two"));
    let d = discover(&inputs(&f, vec![acct("codex", Cli::Codex, codex)]));
    let twins: Vec<&SkillInfo> = d.skills.iter().filter(|s| s.name == "twin").collect();
    assert_eq!(twins.len(), 2);
    assert!(twins.iter().all(|s| s.status == "active" && s.issues.iter().any(|i| i.code == "duplicate_name")));
}

#[test]
fn codex_accounts_each_list_their_own_skills_and_only_repeats_within_one_are_remarked() {
    let f = fx();
    let (a, b) = (f.home.join(".codex"), f.home.join(".codex-work"));
    // A name each account has once: no account sees the other's.
    w(a.join("skills/twin/SKILL.md"), &skill_md("twin", "Account a's"));
    w(b.join("skills/twin/SKILL.md"), &skill_md("twin", "Account b's"));
    // A name one account has twice (its own and the project's): Codex lists both.
    w(a.join("skills/pair/SKILL.md"), &skill_md("pair", "a's"));
    w(f.root.join(".agents/skills/pair/SKILL.md"), &skill_md("pair", "the project's"));
    let d = discover(&inputs(&f, vec![acct("a", Cli::Codex, a), acct("b", Cli::Codex, b)]));
    let remarked = |s: &SkillInfo| s.issues.iter().any(|i| i.code == "duplicate_name");
    assert!(d.skills.iter().filter(|s| s.qualified == "twin").all(|s| !remarked(s)), "{:?}", d.skills.iter().map(|s| &s.issues).collect::<Vec<_>>());
    let pairs: Vec<&SkillInfo> = d.skills.iter().filter(|s| s.qualified == "pair").collect();
    assert!(pairs.len() == 2 && pairs.iter().all(|s| remarked(s)));
    // A skill both accounts read is remarked once, not once for each.
    assert!(pairs.iter().all(|s| s.issues.iter().filter(|i| i.code == "duplicate_name").count() == 1));
}

// ---------------------------------------------------------------- broken and hostile folders

#[test]
fn broken_oversized_and_powerful_skills_are_reported_and_never_run() {
    let f = fx();
    let claude = f.home.join(".claude");
    let dir = claude.join("skills");
    w(dir.join("bad-yaml/SKILL.md"), "---\nname: bad-yaml\ndescription: Use when: colons break it\n---\n# Bad YAML\nFirst real line.\n");
    w(dir.join("big/SKILL.md"), &format!("---\nname: big\ndescription: Big\n---\n{}", "x".repeat(300 * 1024)));
    w(dir.join("no-desc/SKILL.md"), "---\nname: no-desc\n---\n# Heading\nWhat it does.\n");
    std::fs::create_dir_all(dir.join("binary")).unwrap();
    std::fs::write(dir.join("binary/SKILL.md"), (0..=255u8).cycle().take(4000).collect::<Vec<u8>>()).unwrap();
    w(dir.join("mismatch/SKILL.md"), &skill_md("something-else", "Named differently"));
    w(dir.join("long/SKILL.md"), &format!("---\nname: long\ndescription: {}\n---\n", "w ".repeat(800)));
    w(
        dir.join("power/SKILL.md"),
        "---\nname: power\ndescription: Powerful\nallowed-tools: Bash(git *) Read\ndisable-model-invocation: true\ncontext: fork\nagent: Explore\nhooks:\n  PreToolUse: []\npaths: src/**\nmodel: opus\nargument-hint: '[issue]'\n---\nBranch: !`git branch --show-current`\n",
    );
    w(dir.join("power/scripts/run.sh"), "#!/bin/sh\n");
    w(dir.join("power/references/a.md"), "a\n");
    // A skill folder reached through a link (the usual way to share a checkout of skills).
    #[cfg(unix)]
    {
        w(f.home.join("shared-checkout/linked/SKILL.md"), &skill_md("linked", "Reached through a link"));
        std::os::unix::fs::symlink(f.home.join("shared-checkout/linked"), dir.join("linked")).unwrap();
        // A FIFO where the file should be would hang a read: it is not a file, so there is no skill.
        std::fs::create_dir_all(dir.join("fifo")).unwrap();
        nix::unistd::mkfifo(&dir.join("fifo/SKILL.md"), nix::sys::stat::Mode::from_bits_truncate(0o600)).unwrap();
    }
    let d = discover(&inputs(&f, vec![acct("claude", Cli::Claude, claude)]));
    let codes = |n: &str| find(&d, n).issues.iter().map(|i| i.code).collect::<Vec<_>>();

    let bad = find(&d, "bad-yaml");
    assert_eq!(codes("bad-yaml"), ["front_matter"], "one error, not also a missing description");
    assert_eq!(bad.issues[0].level, "error");
    assert_eq!(bad.description, "Bad YAML", "its first line stands in, as it does for Claude Code");
    assert_eq!(codes("big"), ["too_big"]);
    assert_eq!((codes("binary"), find(&d, "binary").description.as_str()), (vec!["not_text"], ""), "a binary file is no skill text");
    assert_eq!(codes("no-desc"), ["no_description"]);
    assert_eq!(find(&d, "no-desc").description, "Heading");
    assert_eq!(codes("something-else"), ["name_mismatch"], "Claude Code names it by its front matter, not its folder");
    assert_eq!(codes("long"), ["description_long"]);

    let p = find(&d, "power");
    assert!(p.flags.manual_only && p.flags.hooks && p.flags.shell && p.flags.scripts);
    assert_eq!((p.flags.forks.as_deref(), p.flags.model.as_deref(), p.flags.argument_hint.as_deref()), (Some("Explore"), Some("opus"), Some("[issue]")));
    assert_eq!((p.flags.preapproves.clone(), p.flags.paths.clone()), (vec!["Bash(git *)".to_string(), "Read".into()], vec!["src/**".to_string()]));
    assert_eq!((p.files, p.tokens), (2, 0), "a skill only the user starts is not in the listing");
    #[cfg(unix)]
    {
        assert_eq!(find(&d, "linked").description, "Reached through a link");
        assert!(!has(&d, "fifo"));
    }
}

#[cfg(unix)]
#[test]
fn plugin_and_synced_skills_that_link_out_are_not_read_but_a_personal_skill_may_link_anywhere() {
    let (f, claude) = claude_fixture();
    let secret = f.home.join("private/notes.md");
    w(&secret, "---\nname: stolen\ndescription: PRIVATE-DESCRIPTION\n---\nPRIVATE-BODY\n");
    // Third-party content (a plugin from a marketplace, claude.ai's sync) that points at a private file.
    let plugin_skill = claude.join("plugins/cache/mp/stripe/1.0/skills/linked");
    std::fs::create_dir_all(&plugin_skill).unwrap();
    std::os::unix::fs::symlink(&secret, plugin_skill.join("SKILL.md")).unwrap();
    // (Its own target: two links to one file are one skill, reached twice.)
    let secret2 = f.home.join("private/other.md");
    w(&secret2, "---\nname: stolen2\ndescription: PRIVATE-TOO\n---\nPRIVATE-BODY\n");
    let synced_skill = claude.join("skills/synced/bucket-1/linked-sync");
    std::fs::create_dir_all(&synced_skill).unwrap();
    std::os::unix::fs::symlink(&secret2, synced_skill.join("SKILL.md")).unwrap();
    // The user's own checkout of skills, linked into their personal folder: the usual way to share them.
    w(f.home.join("dev/my-skills/mine/SKILL.md"), &skill_md("mine", "My own checkout"));
    std::os::unix::fs::symlink(f.home.join("dev/my-skills/mine"), claude.join("skills/mine")).unwrap();

    let d = discover(&inputs(&f, vec![acct("claude", Cli::Claude, claude)]));
    assert!(!serde_json::to_string(&d.skills).unwrap().contains("PRIVATE"), "nothing of the private file reaches the report");
    for (q, place) in [("stripe:linked", "its plugin"), ("anthropic-skills:linked-sync", "claude.ai's synced folder")] {
        let s = find(&d, q);
        assert_eq!(s.description, "", "{q}");
        let issue = s.issues.iter().find(|i| i.code == "link_out").unwrap_or_else(|| panic!("{q}: {:?}", s.issues));
        assert!(issue.message.contains(place), "{q}: {}", issue.message);
        assert!(!d.located.iter().find(|l| l.id == s.id).unwrap().readable, "{q}");
    }
    let mine = find(&d, "mine");
    assert_eq!(mine.description, "My own checkout");
    assert!(mine.issues.iter().all(|i| i.code != "link_out"));
    assert!(d.located.iter().find(|l| l.id == mine.id).unwrap().readable);
}

#[test]
fn only_a_personal_folder_can_say_a_skill_is_workbenchs() {
    let f = fx();
    let claude = f.home.join(".claude");
    let names: Vec<String> = catalog::bundled().into_iter().map(|s| s.name).collect();
    assert!(names.len() >= 4, "the test needs four of Workbench's skills to stand in for each case");
    let (installed, forged, odd, hostile) = (&names[0], &names[1], &names[2], &names[3]);
    let marker = |name: &str, files: &str| format!(r#"{{"skill":"{name}","version":"0.17.0","files":{files}}}"#);
    let marker_file = catalog::MARKER;
    // Installed by Workbench: a folder of the bundle's name with a marker that names it.
    w(claude.join(format!("skills/{installed}/SKILL.md")), &skill_md(installed, "Installed by Workbench"));
    w(claude.join(format!("skills/{installed}/{marker_file}")), &marker(installed, "{}"));
    // A repository forging the marker of a skill of the same name.
    w(f.root.join(format!(".claude/skills/{forged}/SKILL.md")), &skill_md(forged, "A repository forging the marker"));
    w(f.root.join(format!(".claude/skills/{forged}/{marker_file}")), &marker(forged, "{}"));
    // The marker names another skill than the folder's.
    w(claude.join(format!("skills/{odd}/SKILL.md")), &skill_md(odd, "Odd"));
    w(claude.join(format!("skills/{odd}/{marker_file}")), &marker(installed, "{}"));
    // A marker that lists a file outside the folder does not hold up.
    w(claude.join(format!("skills/{hostile}/SKILL.md")), &skill_md(hostile, "Lists a file that is not its own"));
    w(claude.join(format!("skills/{hostile}/{marker_file}")), &marker(hostile, r#"{"../../.ssh/id_demo":"0"}"#));
    // A skill of the user's that is no skill of the bundle, with a marker that says it is one: it stays a personal skill.
    w(claude.join("skills/mine/SKILL.md"), &skill_md("mine", "The user's own"));
    w(claude.join(format!("skills/mine/{marker_file}")), &marker("mine", "{}"));
    // A copy of an installed skill (marker and all) somewhere Install does not write and the catalog does not follow: the
    // folder the agents share, and the folder of an agent that does not get Workbench's tools. Ordinary skills, still listed.
    assert!(names.len() >= 5);
    let copied = &names[4];
    for dir in [f.home.join(".agents/skills"), f.home.join(".gemini/skills")] {
        w(dir.join(format!("{copied}/SKILL.md")), &skill_md(copied, "A copy of an installed skill"));
        w(dir.join(format!("{copied}/{marker_file}")), &marker(copied, "{}"));
    }
    let accounts = vec![acct("claude", Cli::Claude, claude), acct("codex", Cli::Codex, f.home.join(".codex")), acct("gemini", Cli::Gemini, f.home.join(".gemini"))];
    let d = discover(&inputs(&f, accounts));
    let copies: Vec<&SkillInfo> = d.skills.iter().filter(|s| s.qualified == *copied).collect();
    assert_eq!(copies.len(), 2);
    assert!(copies.iter().all(|s| s.workbench.is_none() && s.scope == Scope::User), "{copies:?}");
    assert_eq!(find(&d, installed).workbench.as_deref(), Some(installed.as_str()));
    let project_copy = d.skills.iter().find(|s| s.qualified == *forged && s.scope == Scope::Project).unwrap();
    assert_eq!(project_copy.workbench, None);
    for (name, why) in [(odd, "a marker that names another skill"), (hostile, "a marker that does not hold up"), (&"mine".to_string(), "a skill the bundle does not have")] {
        let s = find(&d, name);
        assert_eq!((s.workbench.clone(), s.scope), (None, Scope::User), "{why}: a personal skill like any other");
    }
    // The marker file is not one of the skill's files.
    assert_eq!(find(&d, installed).files, 0);
}

#[test]
fn a_huge_tree_is_cut_short_and_says_so() {
    let f = fx();
    let claude = f.home.join(".claude");
    for i in 0..(super::discover::MAX_SKILLS + 20) {
        w(claude.join(format!("skills/s{i:05}/SKILL.md")), &skill_md(&format!("s{i:05}"), "x"));
    }
    let d = discover(&inputs(&f, vec![acct("claude", Cli::Claude, claude)]));
    assert_eq!(d.skills.len(), super::discover::MAX_SKILLS);
    assert!(d.notes.iter().any(|n| n.contains("cut short")), "{:?}", d.notes);
}

/// Inputs whose bounds are small, so that a test does not have to build a huge tree to reach them.
fn small(f: &Fx, accounts: Vec<Account>, limits: Limits) -> Inputs {
    Inputs { limits, ..inputs(f, accounts) }
}

#[test]
fn a_repository_with_thousands_of_folders_cannot_starve_the_users_own_skills() {
    let f = fx();
    let claude = f.home.join(".claude");
    w(claude.join("skills/mine/SKILL.md"), &skill_md("mine", "The user's own"));
    w(f.root.join(".claude/skills/proj/SKILL.md"), &skill_md("proj", "The project's"));
    // More folders below the project than the search for nested skills may read.
    for i in 0..400 {
        std::fs::create_dir_all(f.root.join(format!("apps/p{i:05}"))).unwrap();
    }
    w(f.root.join("aaa/.claude/skills/deep/SKILL.md"), &skill_md("deep", "Found before the cut"));
    let d = discover(&small(&f, vec![acct("claude", Cli::Claude, claude)], Limits { nested_work: 300, ..Limits::default() }));
    assert!(has(&d, "mine") && has(&d, "proj"), "the roots are read whatever the search below the project costs");
    assert!(has(&d, "aaa:deep"), "what the search reached before the bound is kept");
    assert!(d.notes.iter().any(|n| n.contains("below the project") && n.contains("cut short")), "{:?}", d.notes);
}

#[test]
fn a_flood_in_one_projects_root_leaves_the_other_roots_and_the_users_own_alone() {
    let f = fx();
    let (claude, codex) = (f.home.join(".claude"), f.home.join(".codex"));
    w(codex.join("skills/mine/SKILL.md"), &skill_md("mine", "The user's own for Codex"));
    // The user's shared folder, read last of all, holds more than what is left of the project's allowance.
    for i in 0..60 {
        w(f.home.join(format!(".agents/skills/shared{i:03}/SKILL.md")), &skill_md(&format!("shared{i:03}"), "The user's shared one"));
    }
    // Two of the project's roots are flooded: names Claude Code reads as commands, and folders Codex reads down six levels.
    std::fs::create_dir_all(f.root.join(".claude/commands")).unwrap();
    for i in 0..150 {
        std::fs::write(f.root.join(format!(".claude/commands/c{i:03}")), "").unwrap();
        std::fs::create_dir_all(f.root.join(format!(".agents/skills/g{i:03}"))).unwrap();
    }
    // A root of the project that is read after both.
    w(f.root.join(".codex/skills/real/SKILL.md"), &skill_md("real", "The project's, for Codex"));
    let limits = Limits { root_work: 100, untrusted_work: 250, ..Limits::default() };
    let d = discover(&small(&f, vec![acct("claude", Cli::Claude, claude), acct("codex", Cli::Codex, codex)], limits));
    let shared = d.skills.iter().filter(|s| s.name.starts_with("shared")).count();
    assert!(has(&d, "mine") && has(&d, "real") && shared == 60, "starved by the flood: real {}, shared {shared} of 60", has(&d, "real"));
    assert!(d.notes.iter().any(|n| n.contains("cut short")), "{:?}", d.notes);
}

#[cfg(unix)]
#[test]
fn links_that_make_one_big_folder_many_cannot_multiply_the_work() {
    let f = fx();
    let codex = f.home.join(".codex");
    w(codex.join("skills/mine/SKILL.md"), &skill_md("mine", "The user's own"));
    // A hundred links in the project's `.agents/skills` to one folder of 300 names, which a recursive read descends into each time.
    std::fs::create_dir_all(f.root.join("big")).unwrap();
    for i in 0..300 {
        std::fs::create_dir_all(f.root.join(format!("big/d{i:03}"))).unwrap();
    }
    std::fs::create_dir_all(f.root.join(".agents/skills")).unwrap();
    for i in 0..100 {
        std::os::unix::fs::symlink(f.root.join("big"), f.root.join(format!(".agents/skills/l{i:03}"))).unwrap();
    }
    let limits = Limits { root_work: 1000, untrusted_work: 1500, ..Limits::default() };
    let d = discover(&small(&f, vec![acct("codex", Cli::Codex, codex)], limits));
    assert!(has(&d, "mine"));
    assert!(d.notes.iter().any(|n| n.contains("cut short")), "the 30,000 names the links lead to were not all read: {:?}", d.notes);
}

#[test]
fn a_folder_of_countless_files_is_read_only_as_far_as_it_can_matter() {
    let f = fx();
    let claude = f.home.join(".claude");
    w(claude.join("skills/mine/SKILL.md"), &skill_md("mine", "The user's own"));
    // A project's skills folder holding more names than one folder may be read for.
    let skills = f.root.join(".claude/skills");
    std::fs::create_dir_all(&skills).unwrap();
    for i in 0..80 {
        std::fs::write(skills.join(format!("f{i:05}")), "").unwrap();
    }
    let d = discover(&small(&f, vec![acct("claude", Cli::Claude, claude)], Limits { entries: 50, ..Limits::default() }));
    assert!(has(&d, "mine"));
    assert!(d.notes.iter().any(|n| n.contains("cut short") && n.contains("names in one folder")), "{:?}", d.notes);

    // The primitive: at most `cap` names, by name, and whether there were more.
    let dir = f.root.join("names");
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..25 {
        std::fs::write(dir.join(format!("n{i:02}")), "").unwrap();
    }
    let (some, more) = super::discover::read_dir_sorted(&dir, 10);
    assert_eq!((some.len(), more), (10, true));
    let (all, more) = super::discover::read_dir_sorted(&dir, 25);
    assert_eq!((all.len(), more), (25, false));
    assert_eq!(all.first().unwrap().file_name(), "n00");

    // A skill's own folder: the list is cut at its limit, and says so.
    let skill = f.root.join("one");
    std::fs::create_dir_all(skill.join("refs")).unwrap();
    for i in 0..60 {
        std::fs::write(skill.join(format!("refs/a{i:05}")), "x").unwrap();
    }
    let (files, cut) = super::discover::folder_files(&skill, 5);
    assert!(cut && files.len() == 5, "{} files, cut: {cut}", files.len());
}

#[test]
fn a_project_inside_a_home_that_is_a_repository_keeps_to_itself_and_the_users_shared_skills_stay_theirs() {
    let f = fx();
    // The user keeps dotfiles in a repository at `~`, and the project merely lies inside it with no repository of its own.
    std::fs::create_dir_all(f.home.join(".git")).unwrap();
    let root = f.home.join("play/proj");
    std::fs::create_dir_all(&root).unwrap();
    w(f.home.join(".agents/skills/shared/SKILL.md"), &skill_md("shared", "The user's skill, shared by their agents"));
    w(f.home.join(".kimi-code/skills/kimi-own/SKILL.md"), &skill_md("kimi-own", "The user's own for Kimi Code"));
    w(f.home.join(".claude/skills/mine/SKILL.md"), &skill_md("mine", "The user's own for Claude Code"));
    w(root.join(".claude/skills/fine/SKILL.md"), &skill_md("fine", "Inside the project"));
    #[cfg(unix)]
    {
        w(f.home.join(".ssh/id_demo"), "---\nname: stolen\ndescription: PRIVATE-KEY\n---\nPRIVATE-BODY\n");
        std::fs::create_dir_all(root.join(".claude/skills/leak")).unwrap();
        std::os::unix::fs::symlink(f.home.join(".ssh/id_demo"), root.join(".claude/skills/leak/SKILL.md")).unwrap();
    }
    let f = Fx { _t: f._t, home: f.home, root };
    let accounts = vec![acct("claude", Cli::Claude, f.home.join(".claude")), acct("codex", Cli::Codex, f.home.join(".codex")), acct("kimi", Cli::Kimi, f.home.join(".kimi-code"))];
    let d = discover(&inputs(&f, accounts));
    assert!(!serde_json::to_string(&d.skills).unwrap().contains("PRIVATE"), "nothing of the private file reaches the report");
    // The user's own skills are theirs and readable: not the project's, and no "link out" of anything.
    for (q, scope) in [("shared", Scope::User), ("kimi-own", Scope::User), ("mine", Scope::User), ("fine", Scope::Project)] {
        let s = find(&d, q);
        assert_eq!(s.scope, scope, "{q}");
        assert!(s.issues.iter().all(|i| i.code != "link_out") && !s.description.is_empty(), "{q}: {:?}", s.issues);
        assert!(d.located.iter().find(|l| l.id == s.id).unwrap().readable, "{q}");
    }
    #[cfg(unix)]
    {
        let leak = find(&d, "leak");
        assert!(leak.issues.iter().any(|i| i.code == "link_out"), "{:?}", leak.issues);
        assert!(!d.located.iter().find(|l| l.id == leak.id).unwrap().readable);
    }
}

#[test]
fn text_is_read_up_to_the_cap_without_a_character_cut_in_two() {
    let t = tempfile::tempdir().unwrap();
    let cap = super::parse::MAX_BYTES;
    // 3-byte characters: the cap lands inside one.
    let big = t.path().join("big.md");
    std::fs::write(&big, "日".repeat(cap as usize / 3 + 1000)).unwrap();
    let (text, cut) = super::discover::read_capped(&big, cap).unwrap();
    assert!(cut && text.ends_with('日') && !text.contains('\u{fffd}'), "{} chars", text.chars().count());
    assert_eq!(text.len() % 3, 0, "whole characters only");
    let (text, cut) = super::discover::read_text(&big, cap).unwrap();
    assert!(cut && text.is_some_and(|t| t.ends_with('日')));
    // Not text: a NUL, or bytes that are not UTF-8. Text that has a replacement mark of its own is text.
    // (Not named `nul`: on Windows that is a device, not a file.)
    let (nul, bad, mark) = (t.path().join("has-a-nul-byte"), t.path().join("not-utf8"), t.path().join("replacement-mark"));
    std::fs::write(&nul, b"ab\0cd").unwrap();
    std::fs::write(&bad, b"ab\xffcd").unwrap();
    std::fs::write(&mark, "a \u{fffd} b").unwrap();
    assert!(super::discover::read_text(&nul, cap).unwrap().0.is_none());
    assert!(super::discover::read_text(&bad, cap).unwrap().0.is_none());
    assert_eq!(super::discover::read_capped(&bad, cap).unwrap().0, "ab\u{fffd}cd", "a skill file in another encoding still reads, with marks");
    assert_eq!(super::discover::read_text(&mark, cap).unwrap().0.as_deref(), Some("a \u{fffd} b"));
}

// ---------------------------------------------------------------- routes

struct Env {
    state: AppState,
    pid: String,
    root: PathBuf,
    claude: PathBuf,
    router: axum::Router,
    _dirs: Vec<tempfile::TempDir>,
}

async fn env() -> Env {
    let (cfg, data, home, proj) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let root = crate::util::os::path::canonicalize(proj.path()).unwrap();
    let home_dir = crate::util::os::path::canonicalize(home.path()).unwrap();
    let claude = home_dir.join("acct");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let slash = |p: &Path| p.to_string_lossy().replace('\\', "/");
    // The claude account lives in a scratch folder and its command is a file that exists, so it counts as installed anywhere.
    let mut config: GlobalConfig = toml::from_str(&format!(
        "[agents.providers.claude]\ncommand = \"{}\"\nenv = {{ CLAUDE_CONFIG_DIR = \"{}\" }}\n",
        slash(&std::env::current_exe().unwrap()),
        slash(&claude)
    ))
    .unwrap();
    config.projects.roots = vec![];
    config.projects.include = vec![root.display().to_string()];
    config.notify.desktop = false;
    let paths = Paths { config_dir: cfg.path().to_path_buf(), data_dir: data.path().to_path_buf() };
    let state = AppState::new(paths, config, "127.0.0.1:7998".parse().unwrap()).await.unwrap();
    state.skills.set_home(home_dir);
    let router = crate::app::build_router(state.clone());
    let pid = state.projects.list()[0].id.clone();
    Env { state, pid, root, claude, router, _dirs: vec![cfg, data, home, proj] }
}

impl Env {
    async fn send(&self, method: Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let token = self.state.auth.master_token().to_string();
        let mut req = Request::builder().method(method).uri(path).header("host", "127.0.0.1").header("authorization", format!("Bearer {token}"));
        let body = match body {
            Some(v) => {
                req = req.header("content-type", "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let resp = self.router.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let status = resp.status().as_u16();
        let bytes = axum::body::to_bytes(resp.into_body(), 16 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned())))
    }

    async fn get(&self, rest: &str) -> (u16, Value) {
        self.send(Method::GET, &format!("/api/projects/{}/skills{rest}", self.pid), None).await
    }

    async fn as_agent(&self, method: Method, path: &str, body: Value) -> Result<Value, crate::error::ApiError> {
        crate::mcp::call_api(&self.state, method, path, Some(body), &McpCtx { terminal_id: Some("t1".into()), project_id: Some(self.pid.clone()) }).await
    }
}

fn skill_named<'a>(report: &'a Value, qualified: &str) -> &'a Value {
    report["skills"].as_array().unwrap().iter().find(|s| s["qualified"] == qualified).unwrap_or_else(|| panic!("no {qualified} in {report}"))
}

#[tokio::test]
async fn the_report_the_detail_and_the_files_of_a_skill() {
    let e = env().await;
    w(e.root.join(".claude/skills/hello/SKILL.md"), &skill_md("hello", "Says hello. Use when greeting."));
    w(e.root.join(".claude/skills/hello/references/guide.md"), "# Guide\n");
    w(e.root.join(".claude/skills/hello/assets/blob.bin"), "ab\0cd");
    // A long reference file in a script that takes three bytes a character: the cap cuts inside one, which is no binary file.
    w(e.root.join(".claude/skills/hello/references/long.md"), &"日".repeat(super::parse::MAX_BYTES as usize / 3 + 500));
    // A skill file that is not text shows nothing of itself.
    std::fs::create_dir_all(e.root.join(".claude/skills/blob")).unwrap();
    std::fs::write(e.root.join(".claude/skills/blob/SKILL.md"), b"---\nname: blob\ndescription: x\n---\nab\0cd").unwrap();
    let (status, report) = e.get("").await;
    assert_eq!(status, 200, "{report}");
    let hello = skill_named(&report, "hello");
    assert_eq!((hello["scope"].as_str(), hello["status"].as_str(), hello["agents"][0].as_str()), (Some("project"), Some("active"), Some("claude")));
    let claude = report["agents"].as_array().unwrap().iter().find(|a| a["id"] == "claude").unwrap();
    assert_eq!((claude["cli"].as_str(), claude["installed"].as_bool(), claude["mcp"].as_bool()), (Some("claude"), Some(true), Some(true)));
    assert!(report["catalog"].as_array().is_some() && report["notes"].as_array().is_some());

    let id = hello["id"].as_str().unwrap();
    let (status, d) = e.get(&format!("/{id}")).await;
    assert_eq!(status, 200, "{d}");
    assert!(d["text"].as_str().unwrap().contains("Body of hello") && d["truncated"] == false);
    let files: Vec<&str> = d["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap()).collect();
    assert_eq!(files, ["assets", "assets/blob.bin", "references", "references/guide.md", "references/long.md"]);

    let (status, f) = e.get(&format!("/{id}/file?path=references/guide.md")).await;
    assert_eq!((status, f["text"].as_str(), f["binary"].as_bool()), (200, Some("# Guide\n"), Some(false)));
    let (status, f) = e.get(&format!("/{id}/file?path=assets/blob.bin")).await;
    assert_eq!((status, f["binary"].as_bool(), f["text"].is_null()), (200, Some(true), true));
    let (status, f) = e.get(&format!("/{id}/file?path=references/long.md")).await;
    assert_eq!((status, f["binary"].as_bool(), f["truncated"].as_bool()), (200, Some(false), Some(true)), "{}", f["text"].as_str().map_or(0, str::len));
    assert!(f["text"].as_str().unwrap().ends_with('日'));
    let blob = skill_named(&report, "blob");
    assert!(blob["issues"].as_array().unwrap().iter().any(|i| i["code"] == "not_text"));
    let (status, b) = e.get(&format!("/{}", blob["id"].as_str().unwrap())).await;
    assert_eq!((status, b["text"].is_null()), (200, true), "{b}");
    // The client names a file inside the skill's folder, never beyond it.
    for bad in ["../../x", "/etc/passwd", "..%2F..%2Fx", "references/../../hello2/SKILL.md"] {
        let (status, body) = e.get(&format!("/{id}/file?path={bad}")).await;
        assert!(status == 400 || status == 403 || status == 404, "{bad}: {status} {body}");
    }
    assert_eq!(e.get("/s000000000000").await.0, 404);
    assert_eq!(e.get("/s000000000000/file?path=SKILL.md").await.0, 404);
    assert_eq!(e.send(Method::GET, "/api/projects/nope/skills", None).await.0, 404);
}

#[cfg(unix)]
#[tokio::test]
async fn a_project_skill_that_is_a_link_out_of_the_project_is_listed_and_never_read() {
    let e = env().await;
    // What a hostile repository could point at: a private file of the user's, as the skill file or as the whole folder.
    w(e.claude.join("secret.txt"), "---\nname: stolen\ndescription: PRIVATE-DESCRIPTION\n---\nPRIVATE-BODY\n");
    std::fs::create_dir_all(e.root.join(".claude/skills/file-link")).unwrap();
    std::os::unix::fs::symlink(e.claude.join("secret.txt"), e.root.join(".claude/skills/file-link/SKILL.md")).unwrap();
    w(e.claude.join("elsewhere/dir-link/SKILL.md"), "---\nname: dir-link\ndescription: PRIVATE-DIR\n---\nPRIVATE-DIR-BODY\n");
    w(e.claude.join("elsewhere/dir-link/notes.txt"), "PRIVATE-NOTES");
    std::os::unix::fs::symlink(e.claude.join("elsewhere/dir-link"), e.root.join(".claude/skills/dir-link")).unwrap();
    // A link that stays inside the project is fine.
    w(e.root.join("shared/real/SKILL.md"), &skill_md("real", "Shared inside the repository"));
    std::os::unix::fs::symlink(e.root.join("shared/real"), e.root.join(".claude/skills/real")).unwrap();

    let (_, report) = e.get("").await;
    assert!(!report.to_string().contains("PRIVATE"), "nothing of the files behind the links reaches the report");
    for name in ["file-link", "dir-link"] {
        let s = skill_named(&report, name);
        assert_eq!(s["description"], "", "{name}");
        assert!(s["issues"].as_array().unwrap().iter().any(|i| i["code"] == "link_out"), "{name}: {s}");
        let id = s["id"].as_str().unwrap();
        let (status, d) = e.get(&format!("/{id}")).await;
        assert_eq!(status, 200);
        assert!(d["text"].is_null() && d["files"].as_array().unwrap().is_empty(), "{name}: {d}");
        assert!(!d.to_string().contains("PRIVATE"), "{name}");
        let (status, body) = e.get(&format!("/{id}/file?path=notes.txt")).await;
        assert_eq!(status, 403, "{name}: {body}");
        assert!(!body.to_string().contains("PRIVATE"));
    }
    assert_eq!(skill_named(&report, "real")["description"], "Shared inside the repository");
}

#[cfg(unix)]
#[tokio::test]
async fn a_link_inside_a_skill_that_leaves_its_folder_is_refused() {
    let e = env().await;
    w(e.root.join(".claude/skills/hello/SKILL.md"), &skill_md("hello", "Hello"));
    w(e.claude.join("secret.txt"), "private\n");
    std::os::unix::fs::symlink(e.claude.join("secret.txt"), e.root.join(".claude/skills/hello/leak.txt")).unwrap();
    let (_, report) = e.get("").await;
    let id = skill_named(&report, "hello")["id"].as_str().unwrap().to_string();
    let (status, body) = e.get(&format!("/{id}/file?path=leak.txt")).await;
    assert!(status == 403 || status == 400, "{status} {body}");
    assert!(!body.to_string().contains("private"));
    // It is listed as what it is, not followed.
    let (_, d) = e.get(&format!("/{id}")).await;
    assert!(d["files"].as_array().unwrap().iter().any(|f| f["path"] == "leak.txt"));
}

#[tokio::test]
async fn installing_and_removing_workbench_skills_is_the_users_and_touches_only_their_files() {
    let e = env().await;
    let all = catalog::bundled();
    let first = all.first().expect("skills/ holds the optional Workbench skills");
    let name = first.name.clone();
    let (_, report) = e.get("").await;
    let entry = report["catalog"].as_array().unwrap().iter().find(|c| c["name"] == name.as_str()).unwrap();
    assert_eq!((entry["installs"][0]["account"].as_str(), entry["installs"][0]["state"].as_str()), (Some("claude"), Some("available")));
    assert!(skill_named_opt(&report, &name).is_none());

    // Detail of a skill that is not installed: its text and files, from the program.
    let (status, d) = e.get(&format!("/wb:{name}")).await;
    assert_eq!(status, 200, "{d}");
    assert!(d["text"].as_str().unwrap().starts_with("---\nname:") && d["catalog"]["name"] == name.as_str());
    let (status, f) = e.get(&format!("/wb:{name}/file?path=SKILL.md")).await;
    assert_eq!((status, f["text"].as_str().unwrap().contains("name:")), (200, true));
    assert_eq!(e.get(&format!("/wb:{name}/file?path=../x")).await.0, 404);
    assert_eq!(e.get("/wb:no-such-skill").await.0, 404);

    // Agents (in-process callers) cannot install or remove.
    for path in ["/api/skills/install", "/api/skills/remove"] {
        let err = e.as_agent(Method::POST, path, json!({"skill": name, "account": "claude"})).await.unwrap_err();
        assert_eq!(err.status.as_u16(), 403, "{path}");
    }
    assert!(!e.claude.join("skills").join(&name).exists());

    let (status, r) = e.send(Method::POST, "/api/skills/install", Some(json!({"skill": name, "account": "claude"}))).await;
    assert_eq!((status, r["state"].as_str()), (200, Some("installed")), "{r}");
    assert!(e.claude.join("skills").join(&name).join("SKILL.md").is_file());
    let (_, report) = e.get("").await;
    let s = skill_named(&report, &name);
    assert_eq!((s["scope"].as_str(), s["workbench"].as_str()), (Some("user"), Some(name.as_str())));
    let entry = report["catalog"].as_array().unwrap().iter().find(|c| c["name"] == name.as_str()).unwrap();
    assert_eq!(entry["installs"][0]["state"], "installed");
    // Again: nothing changes.
    assert_eq!(e.send(Method::POST, "/api/skills/install", Some(json!({"skill": name, "account": "claude"}))).await.0, 200);

    // An edit makes it the user's: no silent overwrite, no silent removal.
    w(e.claude.join("skills").join(&name).join("SKILL.md"), "edited\n");
    let (status, r) = e.send(Method::POST, "/api/skills/install", Some(json!({"skill": name, "account": "claude"}))).await;
    assert_eq!(status, 409, "{r}");
    assert_eq!(e.send(Method::POST, "/api/skills/remove", Some(json!({"skill": name, "account": "claude"}))).await.0, 409);
    assert_eq!(std::fs::read_to_string(e.claude.join("skills").join(&name).join("SKILL.md")).unwrap(), "edited\n");
    assert_eq!(e.send(Method::POST, "/api/skills/remove", Some(json!({"skill": name, "account": "claude", "force": true}))).await.0, 200);
    assert!(!e.claude.join("skills").join(&name).exists());

    // Bad requests: an unknown skill, an account that is not one of Workbench's targets, a name that is a path.
    assert_eq!(e.send(Method::POST, "/api/skills/install", Some(json!({"skill": "nope", "account": "claude"}))).await.0, 404);
    assert_eq!(e.send(Method::POST, "/api/skills/install", Some(json!({"skill": name, "account": "aider"}))).await.0, 400);
    assert_eq!(e.send(Method::POST, "/api/skills/remove", Some(json!({"skill": "../../etc", "account": "claude"}))).await.0, 404);
    // A folder of the same name that is someone else's is left alone.
    w(e.claude.join("skills").join(&name).join("SKILL.md"), "someone else's\n");
    assert_eq!(e.send(Method::POST, "/api/skills/install", Some(json!({"skill": name, "account": "claude", "force": true}))).await.0, 409);
    assert_eq!(std::fs::read_to_string(e.claude.join("skills").join(&name).join("SKILL.md")).unwrap(), "someone else's\n");
}

fn skill_named_opt<'a>(report: &'a Value, qualified: &str) -> Option<&'a Value> {
    report["skills"].as_array().unwrap().iter().find(|s| s["qualified"] == qualified)
}

// ---------------------------------------------------------------- the bundled skills

/// The ```toml fences of a Markdown text.
fn toml_fences(text: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur: Option<String> = None;
    for line in text.lines() {
        match (&mut cur, line.trim_end()) {
            (None, "```toml") => cur = Some(String::new()),
            (Some(buf), "```") => out.push(std::mem::take(buf)),
            (Some(buf), l) => {
                buf.push_str(l);
                buf.push('\n');
            }
            _ => {}
        }
        if line.trim_end() == "```" {
            cur = None;
        }
    }
    out
}

/// Words that start like a Workbench tool and are not one: names the skills use as examples (secret names in TOML).
const NOT_TOOLS: [&str; 2] = ["gitlab_api", "gitlab_shop"];

/// Names that look like Workbench tools: a domain prefix, an underscore, a word, anywhere in the text. Code counts
/// too: a misspelt name in an example call is as wrong as one in a sentence.
fn tool_like(text: &str) -> Vec<String> {
    const PREFIXES: [&str; 13] = ["workbench_", "workspace_", "data_", "debug_", "gitlab_", "github_", "run_", "env_", "code_", "files_", "confluence_", "jira_", "devcontainer_"];
    text.split(|c: char| !(c.is_ascii_lowercase() || c == '_'))
        // Claude Code's own spelling of a tool: `mcp__workbench__<name>`.
        .map(|w| w.strip_prefix("mcp__workbench__").unwrap_or(w))
        .filter(|w| PREFIXES.iter().any(|p| w.starts_with(p) && w.len() > p.len()) && !NOT_TOOLS.contains(w))
        .map(str::to_string)
        .collect()
}

#[test]
fn the_bundled_skills_name_only_tools_that_exist() {
    let tools: std::collections::BTreeSet<String> = crate::mcp::all_tools().into_iter().map(|t| t.name).collect();
    for s in catalog::bundled() {
        for f in &s.files {
            let text = std::str::from_utf8(&f.data).unwrap();
            for name in tool_like(text) {
                assert!(tools.contains(&name), "{}/{} names a tool that does not exist: {name}", s.name, f.path);
            }
        }
    }
}

#[test]
fn the_bundled_skills_link_only_to_files_they_have() {
    for s in catalog::bundled() {
        for f in s.files.iter().filter(|f| f.path.ends_with(".md")) {
            // Links outside code: an example of Markdown inside a fence or backticks is not a link of this file.
            let mut fenced = false;
            let text: String = std::str::from_utf8(&f.data).unwrap().lines().filter(|l| {
                if l.trim_start().starts_with("```") {
                    fenced = !fenced;
                    return false;
                }
                !fenced
            }).collect::<Vec<_>>().join("\n");
            // ... and outside inline code.
            let text = text.split('`').step_by(2).collect::<Vec<_>>().join(" ");
            let base = f.path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
            // `](target)` of Markdown links that are relative.
            for part in text.split("](").skip(1) {
                let target = part.split(')').next().unwrap_or("").split('#').next().unwrap_or("");
                // Not a file: a URL, `mention:<id>` (Confluence's syntax), an absolute path.
                if target.is_empty() || target.contains(':') || target.starts_with('/') {
                    continue;
                }
                let joined = if base.is_empty() { target.to_string() } else { format!("{base}/{target}") };
                assert!(s.file(&joined).is_some(), "{}/{} links to {target}, which the skill does not have", s.name, f.path);
            }
        }
    }
}

#[test]
fn every_toml_example_in_the_bundled_skills_parses_as_the_file_it_belongs_to() {
    let mut seen = 0;
    for s in catalog::bundled() {
        for f in &s.files {
            for fence in toml_fences(std::str::from_utf8(&f.data).unwrap()) {
                seen += 1;
                // An example that says it is for config.toml is read as that; the rest are project files
                // (`.workbench.toml` and the machine overlay share one schema).
                let result = if fence.starts_with("# config.toml") { toml::from_str::<GlobalConfig>(&fence).map(|_| ()) } else { toml::from_str::<crate::config::project::ProjectFile>(&fence).map(|_| ()) };
                assert!(result.is_ok(), "{}/{}: this TOML example does not parse: {}\n{fence}", s.name, f.path, result.unwrap_err());
            }
        }
    }
    assert!(seen > 0, "the project-setup skill carries TOML examples");
}

#[test]
fn the_tool_name_scan_sees_names_wherever_they_are_and_skips_other_words() {
    assert_eq!(tool_like("Use `data_append` and `run_list`, not `data` or `Data_append` or `foo_bar` or `debug_`; `a b`."), ["data_append", "run_list"]);
    // In an example call, in a fence, and spelt the way Claude Code spells it; example secret names are no tools.
    assert_eq!(tool_like("```\ndebug_start(config)\n```\n`mcp__workbench__workspace_create_card` and `token = \"gitlab_shop\"`"), ["debug_start", "workspace_create_card"]);
    assert_eq!(toml_fences("x\n```toml\n[a]\nb = 1\n```\n```bash\nls\n```\n```toml\n# c\n```\n"), ["[a]\nb = 1\n", "# c\n"]);
}
