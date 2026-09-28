//! What the Database tool window's tree shows: schemas with their tables and views
//! (from `pg_catalog`, estimated row counts), and one table's columns, indexes and
//! foreign keys.

use serde::Serialize;
use tokio_postgres::Client;

use super::query::{SqlError, sql_error};

/// Relations listed at most (a schema with more says so).
const MAX_RELATIONS: i64 = 5000;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Relation {
    pub name: String,
    /// `table`, `view`, `matview`, `partitioned`, `foreign`.
    pub kind: &'static str,
    /// The planner's estimate; `None` before the table was ever analyzed.
    pub rows: Option<i64>,
    pub comment: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaInfo {
    pub name: String,
    pub relations: Vec<Relation>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub database: String,
    pub schemas: Vec<SchemaInfo>,
    pub truncated: bool,
}

fn kind_of(relkind: &str) -> &'static str {
    match relkind {
        "v" => "view",
        "m" => "matview",
        "p" => "partitioned",
        "f" => "foreign",
        _ => "table",
    }
}

const SYSTEM: &str = "n.nspname NOT IN ('pg_catalog', 'information_schema') AND n.nspname NOT LIKE 'pg\\_toast%' AND n.nspname NOT LIKE 'pg\\_temp%'";

pub async fn catalog(client: &Client) -> Result<Catalog, SqlError> {
    let db: String = client.query_one("SELECT current_database()::text", &[]).await.map_err(|e| sql_error(&e))?.get(0);
    let schemas = client
        .query(&format!("SELECT n.nspname::text FROM pg_namespace n WHERE {SYSTEM} ORDER BY n.nspname <> 'public', n.nspname"), &[])
        .await
        .map_err(|e| sql_error(&e))?;
    let rels = client
        .query(
            &format!(
                "SELECT n.nspname::text, c.relname::text, c.relkind::text, c.reltuples::float8, obj_description(c.oid, 'pg_class')
                 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
                 WHERE c.relkind IN ('r', 'v', 'm', 'p', 'f') AND NOT c.relispartition AND {SYSTEM}
                 ORDER BY n.nspname, c.relname LIMIT $1"
            ),
            &[&(MAX_RELATIONS + 1)],
        )
        .await
        .map_err(|e| sql_error(&e))?;
    let truncated = rels.len() as i64 > MAX_RELATIONS;
    let mut out: Vec<SchemaInfo> = schemas.iter().map(|r| SchemaInfo { name: r.get(0), relations: vec![] }).collect();
    for r in rels.iter().take(MAX_RELATIONS as usize) {
        let schema: String = r.get(0);
        let tuples: f64 = r.get(3);
        let rel = Relation { name: r.get(1), kind: kind_of(r.get::<_, String>(2).as_str()), rows: (tuples >= 0.0).then_some(tuples as i64), comment: r.get(4) };
        match out.iter_mut().find(|s| s.name == schema) {
            Some(s) => s.relations.push(rel),
            None => out.push(SchemaInfo { name: schema, relations: vec![rel] }),
        }
    }
    Ok(Catalog { database: db, schemas: out, truncated })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub name: String,
    /// `format_type`: `character varying(80)`, `timestamp with time zone`…
    pub data_type: String,
    pub nullable: bool,
    pub default: Option<String>,
    pub primary_key: bool,
    pub comment: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Named {
    pub name: String,
    pub definition: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableInfo {
    pub columns: Vec<Column>,
    pub indexes: Vec<Named>,
    pub foreign_keys: Vec<Named>,
}

/// `schema.table` as a `regclass`, quoted server-side.
const REL: &str = "(quote_ident($1::text) || '.' || quote_ident($2::text))::regclass";

pub async fn table(client: &Client, schema: &str, table: &str) -> Result<TableInfo, SqlError> {
    let cols = client
        .query(
            &format!(
                "SELECT a.attname::text, format_type(a.atttypid, a.atttypmod), NOT a.attnotnull, pg_get_expr(d.adbin, d.adrelid),
                        EXISTS (SELECT 1 FROM pg_index i WHERE i.indrelid = a.attrelid AND i.indisprimary AND a.attnum = ANY(i.indkey)),
                        col_description(a.attrelid, a.attnum)
                 FROM pg_attribute a LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
                 WHERE a.attrelid = {REL} AND a.attnum > 0 AND NOT a.attisdropped
                 ORDER BY a.attnum"
            ),
            &[&schema, &table],
        )
        .await
        .map_err(|e| sql_error(&e))?;
    let indexes = client
        .query(
            &format!("SELECT c.relname::text, pg_get_indexdef(i.indexrelid) FROM pg_index i JOIN pg_class c ON c.oid = i.indexrelid WHERE i.indrelid = {REL} ORDER BY NOT i.indisprimary, c.relname"),
            &[&schema, &table],
        )
        .await
        .map_err(|e| sql_error(&e))?;
    let fks = client
        .query(
            &format!("SELECT conname::text, pg_get_constraintdef(oid) FROM pg_constraint WHERE conrelid = {REL} AND contype = 'f' ORDER BY conname"),
            &[&schema, &table],
        )
        .await
        .map_err(|e| sql_error(&e))?;
    Ok(TableInfo {
        columns: cols
            .iter()
            .map(|r| Column { name: r.get(0), data_type: r.get(1), nullable: r.get(2), default: r.get(3), primary_key: r.get(4), comment: r.get(5) })
            .collect(),
        indexes: indexes.iter().map(|r| Named { name: r.get(0), definition: r.get(1) }).collect(),
        foreign_keys: fks.iter().map(|r| Named { name: r.get(0), definition: r.get(1) }).collect(),
    })
}
