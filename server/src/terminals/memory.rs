//! One memory for every account of a CLI.
//!
//! Claude Code takes its memory location as a setting (`agent::shared_memory_dir`). Codex
//! (`$CODEX_HOME/memories/`) and Gemini CLI (`GEMINI.md` in its `.gemini` folder, where
//! `save_memory` writes) have no such setting and keep it inside the account's folder, so an
//! account's copy is a link to the default account's. Kimi Code's `AGENTS.md` is written by
//! hand and Aider keeps no memory, so neither is touched.
//!
//! Nothing that holds content is ever replaced: an account that has memory of its own keeps
//! it. Windows needs a privilege to make links, so there the accounts keep their own.

use std::path::Path;

use super::providers::ProviderKind;

/// The entry of an account's folder that holds its memory, and whether it is a folder.
fn entry(kind: ProviderKind) -> Option<(&'static str, bool)> {
    match kind {
        ProviderKind::Codex => Some(("memories", true)),
        ProviderKind::Gemini => Some(("GEMINI.md", false)),
        _ => None,
    }
}

/// Link `account`'s memory to `default`'s (the two folders `codex_home` / `gemini_dir` name).
/// `Ok(true)` when a link was made now.
#[cfg(unix)]
pub fn link_shared(kind: ProviderKind, account: &Path, default: &Path) -> std::io::Result<bool> {
    let Some((name, is_dir)) = entry(kind) else { return Ok(false) };
    if account == default {
        return Ok(false);
    }
    let (mine, shared) = (account.join(name), default.join(name));
    match std::fs::symlink_metadata(&mine) {
        Ok(m) if m.file_type().is_symlink() => return Ok(false),
        // An empty one is what the CLI makes on its own at first start: nothing to lose.
        Ok(m) if is_dir && m.is_dir() && std::fs::read_dir(&mine)?.next().is_none() => std::fs::remove_dir(&mine)?,
        Ok(m) if !is_dir && m.is_file() && m.len() == 0 => std::fs::remove_file(&mine)?,
        Ok(_) => return Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    if is_dir {
        std::fs::create_dir_all(&shared)?;
    } else {
        std::fs::create_dir_all(default)?;
        std::fs::OpenOptions::new().create(true).append(true).open(&shared)?;
    }
    std::fs::create_dir_all(account)?;
    std::os::unix::fs::symlink(&shared, &mine)?;
    Ok(true)
}

#[cfg(not(unix))]
pub fn link_shared(_: ProviderKind, _: &Path, _: &Path) -> std::io::Result<bool> {
    Ok(false)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn accounts_link_to_the_default_memory_and_keep_their_own_content() {
        let t = tempfile::tempdir().unwrap();
        let (default, a, b) = (t.path().join("default"), t.path().join("a"), t.path().join("b"));
        // Codex: a new account gets a link; the shared folder is made.
        assert!(link_shared(ProviderKind::Codex, &a, &default).unwrap());
        std::fs::write(a.join("memories/MEMORY.md"), "x").unwrap();
        assert_eq!(std::fs::read_to_string(default.join("memories/MEMORY.md")).unwrap(), "x");
        assert!(!link_shared(ProviderKind::Codex, &a, &default).unwrap(), "already linked");
        // An empty folder of the CLI's own is replaced, one with content is not.
        std::fs::create_dir_all(b.join("memories")).unwrap();
        assert!(link_shared(ProviderKind::Codex, &b, &default).unwrap());
        std::fs::remove_file(b.join("memories")).unwrap();
        std::fs::create_dir_all(b.join("memories")).unwrap();
        std::fs::write(b.join("memories/mine.md"), "y").unwrap();
        assert!(!link_shared(ProviderKind::Codex, &b, &default).unwrap());
        assert!(b.join("memories/mine.md").is_file());
        // The default account itself, and CLIs without one.
        assert!(!link_shared(ProviderKind::Codex, &default, &default).unwrap());
        assert!(!link_shared(ProviderKind::Kimi, &a, &default).unwrap());
        // Gemini: the file.
        let (ga, gb) = (t.path().join("ga/.gemini"), t.path().join("gb/.gemini"));
        assert!(link_shared(ProviderKind::Gemini, &ga, &default).unwrap());
        std::fs::write(ga.join("GEMINI.md"), "fact\n").unwrap();
        assert_eq!(std::fs::read_to_string(default.join("GEMINI.md")).unwrap(), "fact\n");
        std::fs::create_dir_all(&gb).unwrap();
        std::fs::write(gb.join("GEMINI.md"), "own\n").unwrap();
        assert!(!link_shared(ProviderKind::Gemini, &gb, &default).unwrap());
        assert_eq!(std::fs::read_to_string(gb.join("GEMINI.md")).unwrap(), "own\n");
    }
}
