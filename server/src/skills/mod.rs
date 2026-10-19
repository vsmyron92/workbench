//! Agent skills slice (OWNER: skills slice).
//!
//! What the agent CLIs can load as skills, read from the files on disk, and the few skills
//! Workbench itself offers. The CLIs (Claude Code, Codex, Gemini CLI, Kimi Code) share one format,
//! `SKILL.md`, and differ in where they look, so the overview answers "which skills can my agents
//! use in this project, from where, and what do they cost every session". It is read-only, apart
//! from installing and removing Workbench's own skills, which only the user does.
//!
//! * `parse`: a `SKILL.md`: front matter, flags, the checks (a description that is missing or too
//!   long, a name that does not match its folder…), the cost estimate.
//! * `discover`: each CLI's folders and rules, the scan, plugins, settings that turn skills off,
//!   which skill hides which, the report.
//! * `session`: the skill listing Claude Code recorded in a session's transcript, which is how the
//!   skills built into Claude Code (on no disk) are known.
//! * `catalog`: the skills in `skills/` (embedded), their state per account, install and remove.
//!
//! Routes: `GET /api/projects/{pid}/skills[/{id}[/file]]`, `POST /api/skills/install`,
//! `POST /api/skills/remove`. Event: `skills.changed`. No MCP tools: an agent sees its own skills.

mod catalog;
mod discover;
mod parse;
mod routes;
mod session;
#[cfg(test)]
mod tests;

use std::path::PathBuf;

use parking_lot::RwLock;

pub use routes::router;

#[derive(Default)]
pub struct SkillsState {
    /// The home folder skills are looked for under (`~/.agents/skills`, the default account folders):
    /// the real one, except in tests, which point it at a scratch folder.
    home: RwLock<Option<PathBuf>>,
}

impl SkillsState {
    pub(crate) fn home(&self) -> PathBuf {
        self.home.read().clone().unwrap_or_else(|| dirs::home_dir().unwrap_or_default())
    }

    #[cfg(test)]
    pub(crate) fn set_home(&self, home: PathBuf) {
        *self.home.write() = Some(home);
    }
}
