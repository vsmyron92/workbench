//! Comment-preserving updates of `config.toml`.
//!
//! Forms in Settings change a few values; re-serializing the whole config would
//! drop the user's comments and layout. Instead the new config is rendered to
//! TOML and merged into the existing document with `toml_edit`: untouched
//! sections keep their text verbatim, changed values keep their trailing comments.

use toml_edit::{DocumentMut, Item, Table};

use crate::config::GlobalConfig;

pub const HEADER: &str = "# Workbench configuration. See docs/ARCHITECTURE.md#configuration.\n# Secret values never go here; [secrets] says where each one lives.\n\n";

/// A fresh rendering of `cfg` (used when there is no usable file yet).
pub fn render(cfg: &GlobalConfig) -> anyhow::Result<String> {
    Ok(format!("{HEADER}{}", toml::to_string_pretty(cfg)?))
}

/// Rewrite `old_text` so it deserializes to `new_cfg`, keeping comments and
/// formatting of everything that did not change.
pub fn update_text(old_text: &str, new_cfg: &GlobalConfig) -> anyhow::Result<String> {
    let fresh: DocumentMut = toml::to_string_pretty(new_cfg)?.parse()?;
    let (Ok(mut doc), Ok(old_cfg)) = (old_text.parse::<DocumentMut>(), toml::from_str::<GlobalConfig>(old_text)) else {
        return render(new_cfg);
    };
    let old_json = serde_json::to_value(&old_cfg)?;
    let new_json = serde_json::to_value(new_cfg)?;
    // The top-level keys `GlobalConfig` knows, taken from the values themselves so a
    // new section (`[push]`, `[lsp]`, `[debug]`…) cannot be forgotten: a section
    // present in either. Other keys in the file are left alone.
    let mut keys: Vec<&String> = [&old_json, &new_json].into_iter().filter_map(|v| v.as_object()).flat_map(|o| o.keys()).collect();
    keys.sort();
    keys.dedup();
    let root = doc.as_table_mut();
    for key in keys {
        let key = key.as_str();
        if old_json.get(key) == new_json.get(key) {
            continue;
        }
        match (root.get_mut(key), fresh.get(key)) {
            (_, None) => {
                root.remove(key);
            }
            (Some(old), Some(new)) => merge_item(old, new),
            (None, Some(new)) => {
                root.insert(key, detach(new.clone()));
            }
        }
    }
    let text = doc.to_string();
    // Belt and braces: the result must mean exactly `new_cfg`.
    match toml::from_str::<GlobalConfig>(&text) {
        Ok(check) if &check == new_cfg => Ok(text),
        _ => {
            tracing::warn!("config.toml could not be updated in place; writing it afresh (its comments are lost)");
            render(new_cfg)
        }
    }
}

/// Clear document positions copied from the fresh document so inserted tables
/// are printed after the existing ones instead of interleaving.
fn detach(mut item: Item) -> Item {
    if let Item::Table(t) = &mut item {
        detach_table(t);
    }
    item
}

fn detach_table(t: &mut Table) {
    t.set_position(None);
    for (_, v) in t.iter_mut() {
        if let Item::Table(sub) = v {
            detach_table(sub);
        }
    }
}

fn normalized(v: &toml_edit::Value) -> String {
    v.clone().decorated("", "").to_string()
}

fn merge_item(old: &mut Item, new: &Item) {
    match (old, new) {
        (Item::Table(ot), Item::Table(nt)) => merge_table(ot, nt),
        (Item::Value(ov), Item::Value(nv)) => {
            if normalized(ov) != normalized(nv) {
                let decor = ov.decor().clone();
                *ov = nv.clone();
                *ov.decor_mut() = decor;
            }
        }
        (o, n) => *o = detach(n.clone()),
    }
}

fn merge_table(old: &mut Table, new: &Table) {
    let gone: Vec<String> = old.iter().map(|(k, _)| k.to_string()).filter(|k| !new.contains_key(k)).collect();
    for k in gone {
        old.remove(&k);
    }
    for (k, ni) in new.iter() {
        match old.get_mut(k) {
            Some(oi) => merge_item(oi, ni),
            None => {
                old.insert(k, detach(ni.clone()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGINAL: &str = r#"# My Workbench config
# keep this comment

[server]
bind = "127.0.0.1:7777" # local only for now
allowed_hosts = []

[projects]
# where my repos live
roots = ["~/workspace"]
include = []
exclude = []

[agents]
command = "claude"
effort = "xhigh"
remote_control = false
restore_on_start = true
statusline = true

[secrets]
gitlab = { file = "~/.gitlab_token" }
"#;

    #[test]
    fn changes_values_and_keeps_comments() {
        let mut cfg: GlobalConfig = toml::from_str(ORIGINAL).unwrap();
        cfg.server.bind = "0.0.0.0:7777".into();
        cfg.server.allowed_hosts = vec!["box.tailnet.ts.net".into()];
        cfg.agents.effort = Some("high".into());
        let out = update_text(ORIGINAL, &cfg).unwrap();
        assert!(out.contains("# My Workbench config\n# keep this comment"), "{out}");
        assert!(out.contains("bind = \"0.0.0.0:7777\" # local only for now"), "{out}");
        assert!(out.contains("# where my repos live"), "{out}");
        assert!(out.contains("effort = \"high\""), "{out}");
        assert!(out.contains("box.tailnet.ts.net"), "{out}");
        let back: GlobalConfig = toml::from_str(&out).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn adds_and_removes_sections() {
        let mut cfg: GlobalConfig = toml::from_str(ORIGINAL).unwrap();
        cfg.gitlab = Some(crate::config::global::GitlabConfig { host: "gitlab.com".into(), token: "gitlab".into() });
        cfg.secrets.clear();
        cfg.notify.command = Some("curl -d \"$WORKBENCH_MESSAGE\" ntfy.sh/x".into());
        let out = update_text(ORIGINAL, &cfg).unwrap();
        assert!(out.contains("[gitlab]"), "{out}");
        assert!(!out.contains("gitlab_token"), "{out}");
        assert!(out.contains("# keep this comment"), "{out}");
        let back: GlobalConfig = toml::from_str(&out).unwrap();
        assert_eq!(back, cfg);
        // Sections come after the existing ones.
        assert!(out.find("[agents]").unwrap() < out.find("[gitlab]").unwrap(), "{out}");
    }

    /// Settings › Notifications saves `{notify, push}`: a section added after this
    /// module was written must be merged too, not trigger a whole-file rewrite.
    #[test]
    fn every_config_section_is_merged_in_place() {
        let text = format!("{ORIGINAL}\n[notify]\ndesktop = false # quiet box\n");
        let mut cfg: GlobalConfig = toml::from_str(&text).unwrap();
        cfg.push.subject = Some("mailto:me@example.com".into());
        let out = update_text(&text, &cfg).unwrap();
        assert!(out.starts_with("# My Workbench config\n# keep this comment"), "{out}");
        assert!(out.contains("bind = \"127.0.0.1:7777\" # local only for now"), "{out}");
        assert!(out.contains("desktop = false # quiet box"), "{out}");
        assert!(out.contains("[push]\nsubject = \"mailto:me@example.com\""), "{out}");
        assert!(!out.contains(HEADER), "no fallback to a fresh rendering: {out}");
        assert_eq!(toml::from_str::<GlobalConfig>(&out).unwrap(), cfg);
        // And back: the section goes, the comments stay.
        cfg.push = Default::default();
        let back = update_text(&out, &cfg).unwrap();
        assert!(!back.contains("[push]") && back.contains("# keep this comment"), "{back}");
        assert_eq!(toml::from_str::<GlobalConfig>(&back).unwrap(), cfg);
    }

    #[test]
    fn keeps_unknown_keys_and_falls_back_on_garbage() {
        let text = format!("future_setting = 1\n{ORIGINAL}");
        let mut cfg: GlobalConfig = toml::from_str(&text).unwrap();
        cfg.extra_roots = vec!["~/.claude".into()];
        let out = update_text(&text, &cfg).unwrap();
        assert!(out.contains("future_setting = 1"), "{out}");
        let out = update_text("this is [not toml", &cfg).unwrap();
        assert!(out.starts_with(HEADER));
        assert_eq!(toml::from_str::<GlobalConfig>(&out).unwrap(), cfg);
    }
}
