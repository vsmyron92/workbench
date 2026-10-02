//! The Peripherals view's routes: the chip's register map (the `svd` of a remote-target
//! configuration), with the registers read from and written to the target's memory through
//! the debugger. For the user only, like every route that changes or reads what a session
//! holds: reading some peripheral registers changes the chip (a status flag cleared by the
//! read), so an agent does not get them.
//!
//! * `GET sessions/{sid}/svd` → the device and its peripherals (name, base, registers).
//! * `GET sessions/{sid}/svd/{peripheral}?read=true&registers=A,B` → its registers with
//!   fields; with `read` (the program must be suspended) their values. Registers marked
//!   `readAction` and write-only ones are not read unless named in `registers`.
//! * `PUT sessions/{sid}/svd/{peripheral}/{register} {value}` or `{field, value}` (a number,
//!   or a field's enumerated name): write a register or read-modify-write a field.

use std::collections::HashSet;

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::Deserialize;
use serde_json::{Value, json};

use super::routes::{C, session_of, user_only};
use super::session::{Session, SessionState};
use super::svd::{self, Access, Peripheral, Register, mask};
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};

/// Registers read at once: the debug server answers one request at a time anyway.
const READ_AHEAD: usize = 8;

pub async fn list(State(state): State<AppState>, caller: C, Path((pid, sid)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    let s = session_of(&state, &pid, &sid)?;
    let svd = s.svd().await?;
    let peripherals: Vec<Value> = svd
        .peripherals
        .iter()
        .map(|p| json!({ "name": p.name, "base": p.base, "description": p.description, "group": p.group, "registers": p.registers.len() }))
        .collect();
    Ok(Json(json!({ "device": svd.device, "description": svd.description, "peripherals": peripherals })))
}

#[derive(Deserialize)]
pub struct DetailQuery {
    #[serde(default)]
    read: Option<bool>,
    /// Registers to read although reading them changes the chip (comma separated).
    #[serde(default)]
    registers: Option<String>,
}

enum Outcome {
    NotRead,
    Skipped(&'static str),
    Value(u64),
    Error(String),
}

fn hex(v: u64, size: u32) -> String {
    format!("0x{v:0w$X}", w = (size / 4) as usize)
}

fn register_json(base: u64, r: &Register, outcome: &Outcome) -> Value {
    let value = match outcome {
        Outcome::Value(v) => Some(*v),
        _ => None,
    };
    let fields: Vec<Value> = r
        .fields
        .iter()
        .map(|f| {
            let v = value.map(|v| f.extract(v));
            json!({
                "name": f.name, "bitOffset": f.bit_offset, "bitWidth": f.bit_width, "access": f.access,
                "description": f.description, "values": f.values,
                "value": v, "valueName": v.and_then(|v| f.value_name(v)),
            })
        })
        .collect();
    json!({
        "name": r.name, "offset": r.offset, "address": base + r.offset, "size": r.size, "access": r.access,
        "resetValue": r.reset_value.map(|v| hex(v, r.size)), "description": r.description, "readAction": r.read_action,
        "value": value.map(|v| hex(v, r.size)),
        "error": match outcome { Outcome::Error(e) => Some(e.as_str()), _ => None },
        "skipped": match outcome { Outcome::Skipped(why) => Some(*why), _ => None },
        "fields": fields,
    })
}

async fn read_register(s: &Session, base: u64, r: &Register) -> Result<u64, String> {
    let bytes = s.read_memory(base + r.offset, (r.size / 8) as usize).await.map_err(|e| e.message)?;
    let mut le = [0u8; 8];
    le[..bytes.len()].copy_from_slice(&bytes);
    Ok(u64::from_le_bytes(le))
}

fn find<'a>(svd: &'a svd::Svd, name: &str) -> Result<&'a Peripheral, ApiError> {
    svd.peripheral(name).ok_or_else(|| ApiError::not_found(format!("no peripheral {name:?} in {}", svd.device)))
}

fn suspended(s: &Session, what: &str) -> Result<(), ApiError> {
    if s.state() == SessionState::Stopped { Ok(()) } else { Err(ApiError::conflict(format!("the program is not suspended: pause it to {what}"))) }
}

/// What the view shows for one register: its value when it is read, why it was not, or why it failed.
async fn outcome_of(s: &Session, base: u64, r: &Register, read: bool, forced: &HashSet<String>) -> Outcome {
    if !read {
        Outcome::NotRead
    } else if !r.access.readable() {
        Outcome::Skipped("write-only")
    } else if r.read_action && !forced.contains(&r.name.to_ascii_lowercase()) {
        Outcome::Skipped("reading it changes the chip")
    } else {
        match read_register(s, base, r).await {
            Ok(v) => Outcome::Value(v),
            Err(e) => Outcome::Error(e),
        }
    }
}

pub async fn detail(State(state): State<AppState>, caller: C, Path((pid, sid, name)): Path<(String, String, String)>, Query(q): Query<DetailQuery>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    let s = session_of(&state, &pid, &sid)?;
    let svd = s.svd().await?;
    let p = find(&svd, &name)?;
    let read = q.read.unwrap_or(false);
    if read {
        suspended(&s, "read registers")?;
    }
    let forced: HashSet<String> = q.registers.as_deref().unwrap_or("").split(',').map(|n| n.trim().to_ascii_lowercase()).filter(|n| !n.is_empty()).collect();
    let mut outcomes: Vec<Outcome> = Vec::with_capacity(p.registers.len());
    for chunk in p.registers.chunks(READ_AHEAD) {
        outcomes.extend(futures::future::join_all(chunk.iter().map(|r| outcome_of(&s, p.base, r, read, &forced))).await);
    }
    let registers: Vec<Value> = p.registers.iter().zip(&outcomes).map(|(r, o)| register_json(p.base, r, o)).collect();
    Ok(Json(json!({ "name": p.name, "base": p.base, "description": p.description, "group": p.group, "read": read, "registers": registers })))
}

#[derive(Deserialize)]
pub struct WriteBody {
    value: Value,
    #[serde(default)]
    field: Option<String>,
}

/// A number (`5`, `"0x1F"`, `"#101"`), or — for a field — one of its enumerated names.
fn parse_value(v: &Value, names: &[svd::EnumValue]) -> Result<u64, ApiError> {
    match v {
        Value::Number(n) => n.as_u64().ok_or_else(|| ApiError::bad_request("a value is a non-negative integer")),
        Value::String(t) => svd::parse_number(t)
            .or_else(|| names.iter().find(|e| e.name.eq_ignore_ascii_case(t.trim())).map(|e| e.value))
            .ok_or_else(|| ApiError::bad_request(format!("{t:?} is not a number or one of the field's values"))),
        _ => Err(ApiError::bad_request("a value is a number (or a string: 0x1F, #101, a field's value name)")),
    }
}

pub async fn write(State(state): State<AppState>, caller: C, Path((pid, sid, name, reg)): Path<(String, String, String, String)>, Json(b): Json<WriteBody>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    let s = session_of(&state, &pid, &sid)?;
    let svd = s.svd().await?;
    let p = find(&svd, &name)?;
    let r = p.register(&reg).ok_or_else(|| ApiError::not_found(format!("no register {reg:?} in {}", p.name)))?;
    suspended(&s, "write registers")?;
    if !r.access.writable() {
        return Err(ApiError::bad_request(format!("{}.{} is read-only", p.name, r.name)));
    }
    let new = match &b.field {
        None => {
            let v = parse_value(&b.value, &[])?;
            if v > mask(r.size) {
                return Err(ApiError::bad_request(format!("{}.{} is {} bits wide: {v:#x} does not fit", p.name, r.name, r.size)));
            }
            v
        }
        Some(f) => {
            let field = r.field(f).ok_or_else(|| ApiError::not_found(format!("no field {f:?} in {}.{}", p.name, r.name)))?;
            if field.access == Access::ReadOnly {
                return Err(ApiError::bad_request(format!("{}.{}.{} is read-only", p.name, r.name, field.name)));
            }
            // Read-modify-write needs the rest of the register: not a read that changes the chip,
            // and a write-only register has nothing to read (its other bits are written as 0).
            if r.read_action {
                return Err(ApiError::bad_request(format!("reading {}.{} changes the chip, so one of its fields cannot be changed alone: write the whole register", p.name, r.name)));
            }
            let current = if r.access.readable() { read_register(&s, p.base, r).await.map_err(|e| ApiError::new(axum::http::StatusCode::UNPROCESSABLE_ENTITY, "debugger_error", e))? } else { 0 };
            r.with_field(current, field, parse_value(&b.value, &field.values)?).map_err(ApiError::bad_request)?
        }
    };
    let n = (r.size / 8) as usize;
    s.write_memory(p.base + r.offset, &new.to_le_bytes()[..n]).await?;
    s.log("workbench", format!("Wrote {} to {}.{}{}\n", hex(new, r.size), p.name, r.name, b.field.as_deref().map(|f| format!(" (field {f})")).unwrap_or_default()), None);
    s.flush(&state);
    // What the chip holds now (a write may be masked, ignored or have side effects).
    let outcome = if r.access.readable() && !r.read_action {
        match read_register(&s, p.base, r).await {
            Ok(v) => Outcome::Value(v),
            Err(e) => Outcome::Error(e),
        }
    } else {
        Outcome::NotRead
    };
    Ok(Json(register_json(p.base, r, &outcome)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_numbers_or_a_fields_names() {
        let names = vec![svd::EnumValue { value: 2, name: "Alternate".into(), description: None }];
        assert_eq!(parse_value(&json!(5), &names).unwrap(), 5);
        assert_eq!(parse_value(&json!("0x1F"), &names).unwrap(), 31);
        assert_eq!(parse_value(&json!("#101"), &names).unwrap(), 5);
        assert_eq!(parse_value(&json!(" alternate "), &names).unwrap(), 2);
        assert!(parse_value(&json!("nonsense"), &names).is_err());
        assert!(parse_value(&json!(-1), &names).is_err());
        assert!(parse_value(&json!(1.5), &names).is_err());
        assert!(parse_value(&json!(true), &names).is_err());
        assert_eq!(hex(0x5, 32), "0x00000005");
        assert_eq!(hex(0x5, 16), "0x0005");
        assert_eq!(hex(0xAB, 8), "0xAB");
    }
}
