//! Data sources the user adds or edits in the UI go into the project's machine
//! overlay (`projects/<id>.toml`), as `[[database]]` tables edited in place:
//! comments and every other section stay as they were. Only secret *names* are
//! written; values stay wherever `[secrets]` points.

use std::sync::LazyLock;

use regex::Regex;
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};

use crate::config::ProjectFile;
use crate::config::project::DatabaseSource;
use crate::error::ApiError;

/// Names shown in the tree and used in URLs: no leading `_` (reserved paths).
pub fn valid_name(name: &str) -> bool {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9 ._-]{0,63}$").unwrap());
    RE.is_match(name) && !name.ends_with(' ')
}

fn valid_secret_name(name: &str) -> bool {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_.-]{1,64}$").unwrap());
    name.is_empty() || RE.is_match(name)
}

pub fn validate(src: &DatabaseSource) -> Result<(), ApiError> {
    if !valid_name(&src.name) {
        return Err(ApiError::bad_request("a data source name is 1–64 letters, digits, spaces, dots, dashes or underscores, not starting with _"));
    }
    if !matches!(src.kind.as_str(), "" | "postgres" | "postgresql") {
        return Err(ApiError::bad_request("only postgres data sources are supported"));
    }
    if src.host.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ApiError::bad_request("the host has spaces in it"));
    }
    if src.port == Some(0) {
        return Err(ApiError::bad_request("port 0 is not a port"));
    }
    for (what, n) in [("password", &src.password), ("url", &src.url)] {
        if !valid_secret_name(n) {
            return Err(ApiError::bad_request(format!("{what} is the *name* of a [secrets] entry (letters, digits, _ . -), not a value")));
        }
    }
    if [&src.database, &src.user].iter().any(|v| v.chars().any(char::is_control)) {
        return Err(ApiError::bad_request("database and user cannot contain control characters"));
    }
    super::conn::Tls::parse(&src.sslmode)?;
    Ok(())
}

fn table_of(src: &DatabaseSource) -> Table {
    let mut t = Table::new();
    t["name"] = value(src.name.clone());
    if !matches!(src.kind.as_str(), "" | "postgres") {
        t["kind"] = value(src.kind.clone());
    }
    for (k, v) in [("host", &src.host), ("database", &src.database), ("user", &src.user), ("password", &src.password), ("url", &src.url), ("sslmode", &src.sslmode)] {
        if !v.trim().is_empty() {
            t[k] = value(v.trim().to_string());
        }
    }
    if let Some(p) = src.port {
        t["port"] = value(i64::from(p));
    }
    if src.read_only {
        t["read_only"] = value(true);
    }
    t
}

/// The overlay text with the source `name` replaced by `new` (added when missing), or
/// removed when `new` is `None`. `Ok(None)`: nothing to remove.
pub fn edit(text: &str, name: &str, new: Option<&DatabaseSource>) -> Result<Option<String>, ApiError> {
    let mut doc: DocumentMut = text.parse().map_err(|e| ApiError::bad_request(format!("the overlay does not parse, so it was left alone: {e}")))?;
    if !doc.contains_key("database") {
        if new.is_none() {
            return Ok(None);
        }
        doc.insert("database", Item::ArrayOfTables(ArrayOfTables::new()));
    }
    let arr = doc["database"].as_array_of_tables_mut().ok_or_else(|| ApiError::bad_request("database in the overlay is not a list of [[database]] tables"))?;
    let at = arr.iter().position(|t| t.get("name").and_then(|v| v.as_str()) == Some(name));
    match (at, new) {
        (Some(i), Some(src)) => {
            // In place: the entry keeps its position (and the comments around it).
            let t = arr.get_mut(i).expect("index from position");
            let decor = t.decor().clone();
            *t = table_of(src);
            *t.decor_mut() = decor;
        }
        (None, Some(src)) => arr.push(table_of(src)),
        (Some(i), None) => {
            arr.remove(i);
        }
        (None, None) => return Ok(None),
    }
    if arr.is_empty() {
        doc.remove("database");
    }
    let out = doc.to_string();
    // What is written must load.
    toml::from_str::<ProjectFile>(&out).map_err(|e| ApiError::bad_request(format!("the edited overlay would not load: {e}")))?;
    Ok(Some(out))
}

/// Names of the overlay's own `[[database]]` entries.
pub fn overlay_names(text: &str) -> Vec<String> {
    toml::from_str::<ProjectFile>(text).map(|p| p.databases.into_iter().map(|d| d.name).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(name: &str) -> DatabaseSource {
        DatabaseSource { name: name.into(), host: "localhost".into(), port: Some(5432), database: "shop".into(), user: "app".into(), password: "shop_pw".into(), ..Default::default() }
    }

    #[test]
    fn edits_keep_comments_and_other_sections() {
        let text = "# machine overlay\n[secrets]\nshop_pw = { file = \"~/.shop_pw\" }\n\n# the dev database\n[[database]]\nname = \"dev\"\nhost = \"old\"\n\n[[run]]\nname = \"api\"\ncommand = \"make\"\n";
        let mut s = src("dev");
        s.read_only = true;
        let out = edit(text, "dev", Some(&s)).unwrap().unwrap();
        assert!(out.contains("# machine overlay") && out.contains("# the dev database"), "{out}");
        assert!(out.contains("host = \"localhost\"") && !out.contains("\"old\""), "{out}");
        assert!(out.contains("read_only = true") && out.contains("[[run]]") && out.contains("shop_pw = { file"), "{out}");
        let pf: ProjectFile = toml::from_str(&out).unwrap();
        assert_eq!(pf.databases[0].port, Some(5432));

        let added = edit(&out, "reports", Some(&src("reports"))).unwrap().unwrap();
        assert_eq!(overlay_names(&added), ["dev", "reports"]);
        let removed = edit(&added, "dev", None).unwrap().unwrap();
        assert_eq!(overlay_names(&removed), ["reports"]);
        let gone = edit(&removed, "reports", None).unwrap().unwrap();
        assert!(!gone.contains("[[database]]") && gone.contains("[[run]]"), "{gone}");
        assert!(edit(&gone, "nope", None).unwrap().is_none());
        assert!(edit("", "x", Some(&src("x"))).unwrap().unwrap().starts_with("[[database]]"));
    }

    #[test]
    fn values_are_validated() {
        assert!(validate(&src("dev")).is_ok());
        assert!(validate(&src("_meta")).is_err(), "reserved");
        assert!(validate(&src("a/b")).is_err());
        let mut s = src("dev");
        s.password = "hunter 2".into();
        assert!(validate(&s).is_err(), "a value typed where a secret name goes");
        let mut s = src("dev");
        s.sslmode = "allow".into();
        assert!(validate(&s).is_err());
        let mut s = src("dev");
        s.host = "a b".into();
        assert!(validate(&s).is_err());
    }
}
