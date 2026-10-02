//! CMSIS-SVD: the register map of a microcontroller (peripherals, registers, bit fields,
//! enumerated values), as chip vendors publish it. The debugger's Peripherals view reads
//! one to name the memory-mapped registers it reads and writes.
//!
//! The reader is tolerant the way vendors' files need it to be: `derivedFrom` on
//! peripherals, registers, fields and enumerations, `dim` arrays (`%s` and `[%s]`),
//! clusters, defaults inherited from the device, the peripheral and the cluster, numbers as
//! decimal, `0x` or `#` binary. What it does not use is dropped (address blocks, write
//! constraints, interrupts, DMA…). It never runs anything and never echoes the file's text
//! into an error: the file is a project file of unknown origin.

use std::collections::HashMap;
use std::path::Path;

use quick_xml::Reader;
use quick_xml::events::Event;
use serde::Serialize;

/// The largest SVD file read (the biggest vendor files are about 20 MB).
pub const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_DEPTH: usize = 32;
const MAX_PERIPHERALS: usize = 2000;
const MAX_REGISTERS: usize = 200_000;
const MAX_FIELDS: usize = 256;
const MAX_DIM: u64 = 4096;
const MAX_ENUMS: usize = 4096;
const TEXT: usize = 400;

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Access {
    #[default]
    ReadWrite,
    ReadOnly,
    WriteOnly,
}

impl Access {
    fn parse(s: &str) -> Option<Access> {
        match s.trim() {
            "read-only" => Some(Access::ReadOnly),
            "write-only" | "writeOnce" => Some(Access::WriteOnly),
            "read-write" | "read-writeOnce" => Some(Access::ReadWrite),
            _ => None,
        }
    }

    pub fn readable(self) -> bool {
        self != Access::WriteOnly
    }

    pub fn writable(self) -> bool {
        self != Access::ReadOnly
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EnumValue {
    pub value: u64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    pub name: String,
    pub bit_offset: u32,
    pub bit_width: u32,
    pub access: Access,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub values: Vec<EnumValue>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Register {
    /// Within its peripheral: `CR1`, or `CH0.CTRL` inside a cluster; array elements are
    /// numbered (`DATA[3]`, `TIM2`).
    pub name: String,
    /// From the peripheral's base address.
    pub offset: u64,
    /// Bits: 8, 16, 32 or 64.
    pub size: u32,
    pub access: Access,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_value: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Reading it changes the chip (a status register that clears on read): it is only read
    /// when asked for by name.
    pub read_action: bool,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Peripheral {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    pub base: u64,
    pub registers: Vec<Register>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Svd {
    pub device: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub peripherals: Vec<Peripheral>,
}

pub fn mask(width: u32) -> u64 {
    if width >= 64 { u64::MAX } else { (1u64 << width) - 1 }
}

impl Field {
    /// The field's bits of a register's value.
    pub fn extract(&self, register: u64) -> u64 {
        (register >> self.bit_offset.min(63)) & mask(self.bit_width)
    }

    pub fn value_name(&self, v: u64) -> Option<&str> {
        self.values.iter().find(|e| e.value == v).map(|e| e.name.as_str())
    }
}

impl Register {
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name.eq_ignore_ascii_case(name))
    }

    /// The register's value with `field` set to `new`; an error when `new` does not fit.
    pub fn with_field(&self, current: u64, field: &Field, new: u64) -> Result<u64, String> {
        if new > mask(field.bit_width) {
            return Err(format!("{} is {} bit{} wide: {new} does not fit", field.name, field.bit_width, if field.bit_width == 1 { "" } else { "s" }));
        }
        let m = mask(field.bit_width) << field.bit_offset.min(63);
        Ok((current & !m) | (new << field.bit_offset.min(63)))
    }
}

impl Svd {
    /// A peripheral by name (case-insensitive).
    pub fn peripheral(&self, name: &str) -> Option<&Peripheral> {
        self.peripherals.iter().find(|p| p.name.eq_ignore_ascii_case(name))
    }
}

impl Peripheral {
    pub fn register(&self, name: &str) -> Option<&Register> {
        self.registers.iter().find(|r| r.name.eq_ignore_ascii_case(name))
    }
}

// ---------------------------------------------------------------- reading the file

/// Read an SVD file: a regular file of at most `MAX_FILE` bytes named `.svd` or `.xml`.
pub fn load(path: &Path) -> Result<Svd, String> {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    if !matches!(ext.as_deref(), Some("svd" | "xml")) {
        return Err(format!("{} is not an SVD file (.svd or .xml)", path.display()));
    }
    let meta = std::fs::metadata(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if !meta.is_file() || meta.len() > MAX_FILE {
        return Err(format!("{} is not a regular file of at most {} MB", path.display(), MAX_FILE / (1024 * 1024)));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse(&String::from_utf8_lossy(&bytes)).map_err(|e| format!("{}: {e}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()))
}

#[derive(Debug, Default, Clone)]
struct Node {
    name: String,
    derived_from: Option<String>,
    text: String,
    children: Vec<Node>,
}

impl Node {
    fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    fn named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// The trimmed text of a child element.
    fn get(&self, name: &str) -> Option<&str> {
        self.child(name).map(|c| c.text.trim()).filter(|t| !t.is_empty())
    }

    fn number(&self, name: &str) -> Option<u64> {
        self.get(name).and_then(parse_number)
    }

    fn description(&self) -> Option<String> {
        self.get("description").map(clean_text).filter(|t| !t.is_empty())
    }
}

fn clean_text(s: &str) -> String {
    let joined = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() > TEXT { format!("{}…", joined.chars().take(TEXT).collect::<String>()) } else { joined }
}

/// SVD's scaled non-negative integers: `42`, `0x2A`, `#101010`.
pub fn parse_number(s: &str) -> Option<u64> {
    let s = s.trim();
    if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return u64::from_str_radix(h, 16).ok();
    }
    if let Some(b) = s.strip_prefix('#').or_else(|| s.strip_prefix("0b")) {
        // `x` bits are "don't care": not a value.
        return if b.is_empty() || b.chars().any(|c| c != '0' && c != '1') { None } else { u64::from_str_radix(b, 2).ok() };
    }
    s.parse().ok()
}

fn parse_tree(src: &str) -> Result<Node, String> {
    let mut reader = Reader::from_str(src);
    reader.config_mut().trim_text(false);
    let mut stack: Vec<Node> = vec![Node::default()];
    let node_of = |e: &quick_xml::events::BytesStart| {
        let name = e.local_name().as_ref().to_string();
        let derived_from = e.attributes().flatten().find(|a| a.key.local_name().as_ref() == "derivedFrom").map(|a| a.value.trim().to_string());
        Node { name, derived_from, ..Default::default() }
    };
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                if stack.len() > MAX_DEPTH {
                    return Err(format!("elements nested deeper than {MAX_DEPTH}"));
                }
                stack.push(node_of(&e));
            }
            Ok(Event::Empty(e)) => {
                let n = node_of(&e);
                stack.last_mut().expect("root").children.push(n);
            }
            Ok(Event::End(_)) => {
                let n = stack.pop().expect("root");
                match stack.last_mut() {
                    Some(parent) => parent.children.push(n),
                    None => return Err("an end tag without its start".into()),
                }
            }
            Ok(Event::Text(t)) => {
                let top = stack.last_mut().expect("root");
                if top.text.len() < 64 * 1024 {
                    top.text.push_str(&t);
                }
            }
            Ok(Event::CData(c)) => stack.last_mut().expect("root").text.push_str(&c),
            Ok(Event::GeneralRef(r)) => {
                let ch = match r.resolve_char_ref() {
                    Ok(Some(c)) => Some(c),
                    _ => match &*r {
                        "amp" => Some('&'),
                        "lt" => Some('<'),
                        "gt" => Some('>'),
                        "quot" => Some('"'),
                        "apos" => Some('\''),
                        _ => None,
                    },
                };
                if let Some(c) = ch {
                    stack.last_mut().expect("root").text.push(c);
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => return Err(format!("not valid XML (byte {}): {e}", reader.buffer_position())),
        }
    }
    if stack.len() != 1 {
        return Err("the file ends inside an element".into());
    }
    Ok(stack.pop().expect("root"))
}

// ---------------------------------------------------------------- the register map

#[derive(Debug, Clone, Copy)]
struct Defaults {
    size: u32,
    access: Access,
    reset_value: Option<u64>,
}

impl Defaults {
    fn over(self, n: &Node) -> Defaults {
        Defaults {
            size: n.number("size").and_then(|s| u32::try_from(s).ok()).filter(|s| matches!(s, 8 | 16 | 32 | 64)).unwrap_or(self.size),
            access: n.get("access").and_then(Access::parse).unwrap_or(self.access),
            reset_value: n.number("resetValue").or(self.reset_value),
        }
    }
}

/// The names and index offsets of the instances of an element with `dim`: one without it is
/// itself.
fn instances(n: &Node) -> Result<Vec<(String, u64)>, String> {
    let name = n.get("name").ok_or_else(|| format!("a <{}> without a name", n.name))?.to_string();
    let Some(dim) = n.number("dim") else { return Ok(vec![(name, 0)]) };
    if dim == 0 || dim > MAX_DIM {
        return Err(format!("{name}: dim {dim} is not between 1 and {MAX_DIM}"));
    }
    let step = n.number("dimIncrement").unwrap_or(0);
    let mut labels: Vec<String> = vec![];
    match n.get("dimIndex") {
        Some(list) => {
            for tok in list.split(',').map(str::trim).filter(|t| !t.is_empty()) {
                match tok.split_once('-').and_then(|(a, b)| Some((a.trim().parse::<u64>().ok()?, b.trim().parse::<u64>().ok()?))) {
                    Some((a, b)) if a <= b && b - a < MAX_DIM => labels.extend((a..=b).map(|i| i.to_string())),
                    _ => labels.push(tok.to_string()),
                }
            }
        }
        None => labels = (0..dim).map(|i| i.to_string()).collect(),
    }
    labels.truncate(dim as usize);
    Ok(labels
        .into_iter()
        .enumerate()
        .map(|(i, label)| {
            let name = if name.contains("%s") { name.replace("%s", &label) } else { format!("{name}{label}") };
            (name, step.saturating_mul(i as u64))
        })
        .collect())
}

fn bit_range(f: &Node) -> Option<(u32, u32)> {
    let (off, width) = if let Some(off) = f.number("bitOffset") {
        (off, f.number("bitWidth").unwrap_or(1))
    } else if let (Some(lsb), Some(msb)) = (f.number("lsb"), f.number("msb")) {
        (lsb, msb.checked_sub(lsb)? + 1)
    } else if let Some(r) = f.get("bitRange") {
        let inner = r.trim().strip_prefix('[')?.strip_suffix(']')?;
        let (msb, lsb) = inner.split_once(':')?;
        let (msb, lsb) = (msb.trim().parse::<u64>().ok()?, lsb.trim().parse::<u64>().ok()?);
        (lsb, msb.checked_sub(lsb)? + 1)
    } else {
        return None;
    };
    (width >= 1 && width <= 64 && off < 64 && off + width <= 64).then_some((off as u32, width as u32))
}

/// What reading a peripheral needs to look things up while it is built.
struct Ctx<'a> {
    /// Registers of this peripheral so far (for `derivedFrom` within it).
    out: Vec<Register>,
    /// Enumerations by name, of this peripheral and earlier ones.
    enums: HashMap<String, Vec<EnumValue>>,
    /// Peripherals already built.
    done: &'a [Peripheral],
    registers_total: &'a mut usize,
}

impl Ctx<'_> {
    fn find_register(&self, path: &str) -> Option<Register> {
        let path = path.trim();
        if let Some(r) = self.out.iter().find(|r| r.name == path) {
            return Some(r.clone());
        }
        let (periph, rest) = path.split_once('.')?;
        self.done.iter().find(|p| p.name == periph)?.registers.iter().find(|r| r.name == rest).cloned()
    }
}

fn enum_values(f: &Node, ctx: &mut Ctx) -> Result<Vec<EnumValue>, String> {
    // Of the sets of a field (one per usage), the one that reads, else the first.
    let sets: Vec<&Node> = f.named("enumeratedValues").collect();
    let Some(set) = sets.iter().find(|s| matches!(s.get("usage"), Some("read" | "read-write"))).or_else(|| sets.first()) else { return Ok(vec![]) };
    if let Some(from) = &set.derived_from {
        let key = from.rsplit('.').next().unwrap_or(from);
        return Ok(ctx.enums.get(key).cloned().unwrap_or_default());
    }
    let mut out = vec![];
    for v in set.named("enumeratedValue") {
        // `isDefault` entries and `#1x0` don't-care patterns are not one value.
        let (Some(name), Some(value)) = (v.get("name"), v.number("value")) else { continue };
        out.push(EnumValue { value, name: name.to_string(), description: v.description() });
        if out.len() >= MAX_ENUMS {
            return Err("too many enumerated values".into());
        }
    }
    if let Some(name) = set.get("name") {
        ctx.enums.insert(name.to_string(), out.clone());
    }
    Ok(out)
}

fn build_fields(r: &Node, template: Option<&Register>, defaults: Defaults, ctx: &mut Ctx) -> Result<Vec<Field>, String> {
    let mut out: Vec<Field> = template.map(|t| t.fields.clone()).unwrap_or_default();
    let Some(fields) = r.child("fields") else { return Ok(out) };
    for f in fields.named("field") {
        for (name, _) in instances(f)? {
            let mut base: Option<Field> = None;
            if let Some(from) = &f.derived_from {
                let leaf = from.rsplit('.').next().unwrap_or(from);
                base = out.iter().find(|x| x.name == leaf).cloned().or_else(|| ctx.out.iter().flat_map(|r| r.fields.iter()).find(|x| x.name == leaf).cloned());
            }
            let (bit_offset, bit_width) = bit_range(f).or(base.as_ref().map(|b| (b.bit_offset, b.bit_width))).ok_or_else(|| format!("field {name}: no bit range"))?;
            let values = {
                let v = enum_values(f, ctx)?;
                if v.is_empty() { base.as_ref().map(|b| b.values.clone()).unwrap_or_default() } else { v }
            };
            let field = Field {
                name: name.clone(),
                bit_offset,
                bit_width,
                access: f.get("access").and_then(Access::parse).or(base.as_ref().map(|b| b.access)).unwrap_or(defaults.access),
                description: f.description().or_else(|| base.and_then(|b| b.description)),
                values,
            };
            match out.iter_mut().find(|x| x.name == name) {
                Some(slot) => *slot = field,
                None => out.push(field),
            }
            if out.len() > MAX_FIELDS {
                return Err(format!("more than {MAX_FIELDS} fields in one register"));
            }
        }
    }
    out.sort_by_key(|f| f.bit_offset);
    Ok(out)
}

fn build_register(r: &Node, prefix: &str, name: &str, offset: u64, defaults: Defaults, ctx: &mut Ctx) -> Result<Register, String> {
    let template = match &r.derived_from {
        Some(from) => Some(ctx.find_register(from).ok_or_else(|| format!("register {name}: derivedFrom {from:?} is not a register of this file"))?),
        None => None,
    };
    let d = defaults.over(r);
    let fields = build_fields(r, template.as_ref(), d, ctx)?;
    Ok(Register {
        name: format!("{prefix}{name}"),
        offset,
        size: r.number("size").and_then(|s| u32::try_from(s).ok()).filter(|s| matches!(s, 8 | 16 | 32 | 64)).or(template.as_ref().map(|t| t.size)).unwrap_or(d.size),
        access: r.get("access").and_then(Access::parse).or(template.as_ref().map(|t| t.access)).unwrap_or(d.access),
        reset_value: r.number("resetValue").or(template.as_ref().and_then(|t| t.reset_value)).or(d.reset_value),
        description: r.description().or_else(|| template.as_ref().and_then(|t| t.description.clone())),
        read_action: r.get("readAction").is_some_and(|a| a != "none") || template.as_ref().is_some_and(|t| t.read_action),
        fields,
    })
}

/// The registers of a `<registers>` or `<cluster>` element, appended to `ctx.out`.
fn build_registers(node: &Node, prefix: &str, base: u64, defaults: Defaults, depth: usize, ctx: &mut Ctx) -> Result<(), String> {
    if depth > 8 {
        return Err("clusters nested deeper than 8".into());
    }
    for child in &node.children {
        match child.name.as_str() {
            "register" => {
                let offset = child.number("addressOffset").unwrap_or(0);
                for (name, step) in instances(child)? {
                    *ctx.registers_total += 1;
                    if *ctx.registers_total > MAX_REGISTERS {
                        return Err(format!("more than {MAX_REGISTERS} registers"));
                    }
                    let reg = build_register(child, prefix, &name, base.saturating_add(offset).saturating_add(step), defaults, ctx)?;
                    ctx.out.push(reg);
                }
            }
            "cluster" => {
                let offset = child.number("addressOffset").unwrap_or(0);
                let d = defaults.over(child);
                for (name, step) in instances(child)? {
                    build_registers(child, &format!("{prefix}{name}."), base.saturating_add(offset).saturating_add(step), d, depth + 1, ctx)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// A peripheral element, optionally derived from `base` (its registers are the base's, then
/// this element's own, replacing by name).
fn build_peripheral(n: &Node, name: &str, base_offset: u64, from: Option<&Peripheral>, device: Defaults, done: &[Peripheral], enums: &mut HashMap<String, Vec<EnumValue>>, total: &mut usize) -> Result<Peripheral, String> {
    let d = device.over(n);
    let mut ctx = Ctx { out: vec![], enums: std::mem::take(enums), done, registers_total: total };
    if let Some(f) = from {
        ctx.out = f.registers.clone();
    }
    let inherited = ctx.out.len();
    if let Some(regs) = n.child("registers") {
        let own_start = ctx.out.len();
        build_registers(regs, "", 0, d, 0, &mut ctx)?;
        // A register of this element replaces the base's of the same name.
        let own: Vec<Register> = ctx.out.split_off(own_start);
        for r in own {
            match ctx.out[..inherited.min(ctx.out.len())].iter().position(|x| x.name == r.name) {
                Some(i) => ctx.out[i] = r,
                None => ctx.out.push(r),
            }
        }
    }
    *enums = ctx.enums;
    let mut registers = ctx.out;
    registers.sort_by(|a, b| a.offset.cmp(&b.offset).then_with(|| a.name.cmp(&b.name)));
    Ok(Peripheral {
        name: name.to_string(),
        description: n.description().or_else(|| from.and_then(|f| f.description.clone())),
        group: n.get("groupName").map(str::to_string).or_else(|| from.and_then(|f| f.group.clone())),
        base: n.number("baseAddress").map(|b| b.saturating_add(base_offset)).or(from.map(|f| f.base)).ok_or_else(|| format!("peripheral {name}: no baseAddress"))?,
        registers,
    })
}

/// Parse the text of an SVD file.
pub fn parse(text: &str) -> Result<Svd, String> {
    let root = parse_tree(text)?;
    let device = root.child("device").ok_or("this is not an SVD file: no <device> element")?;
    let defaults = Defaults { size: 32, access: Access::ReadWrite, reset_value: None }.over(device);
    let mut done: Vec<Peripheral> = vec![];
    let mut enums: HashMap<String, Vec<EnumValue>> = HashMap::new();
    let mut total = 0usize;
    let mut pending: Vec<(&Node, String, u64)> = vec![];
    for p in device.child("peripherals").map(|c| c.named("peripheral").collect::<Vec<_>>()).unwrap_or_default() {
        for (name, step) in instances(p)? {
            if done.len() + pending.len() >= MAX_PERIPHERALS {
                return Err(format!("more than {MAX_PERIPHERALS} peripherals"));
            }
            if p.derived_from.is_some() {
                pending.push((p, name, step));
            } else {
                let built = build_peripheral(p, &name, step, None, defaults, &done, &mut enums, &mut total)?;
                done.push(built);
            }
        }
    }
    // Derived peripherals, once what they derive from exists (vendors list them in any order).
    while !pending.is_empty() {
        let before = pending.len();
        let mut next = vec![];
        for (p, name, step) in pending {
            let from = p.derived_from.as_deref().unwrap_or("");
            match done.iter().find(|d| d.name == from).cloned() {
                Some(base) => {
                    let built = build_peripheral(p, &name, step, Some(&base), defaults, &done, &mut enums, &mut total)?;
                    done.push(built);
                }
                None => next.push((p, name, step)),
            }
        }
        if next.len() == before {
            let (_, name, _) = &next[0];
            return Err(format!("peripheral {name} derives from {:?}, which is not in the file", next[0].0.derived_from.as_deref().unwrap_or("")));
        }
        pending = next;
    }
    done.sort_by(|a, b| a.base.cmp(&b.base).then_with(|| a.name.cmp(&b.name)));
    Ok(Svd { device: device.get("name").unwrap_or("device").to_string(), description: device.description(), peripherals: done })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHIP: &str = r##"<?xml version="1.0" encoding="utf-8"?>
<device schemaVersion="1.3" xmlns:xs="http://www.w3.org/2001/XMLSchema-instance">
  <name>TESTCHIP</name>
  <description>A chip &amp; its &quot;registers&quot;</description>
  <size>32</size>
  <access>read-write</access>
  <resetValue>0x00000000</resetValue>
  <peripherals>
    <peripheral>
      <name>TIM1</name>
      <description>Timer   one</description>
      <groupName>TIM</groupName>
      <baseAddress>0x40010000</baseAddress>
      <registers>
        <register>
          <name>CR1</name>
          <description>control 1</description>
          <addressOffset>0x0</addressOffset>
          <resetValue>0x0</resetValue>
          <fields>
            <field><name>CEN</name><description>Counter enable</description><bitOffset>0</bitOffset><bitWidth>1</bitWidth>
              <enumeratedValues><name>CENe</name>
                <enumeratedValue><name>Disabled</name><value>0</value></enumeratedValue>
                <enumeratedValue><name>Enabled</name><description>on</description><value>#1</value></enumeratedValue>
                <enumeratedValue><name>Dontcare</name><value>#x1</value></enumeratedValue>
              </enumeratedValues>
            </field>
            <field><name>CMS</name><bitRange>[6:5]</bitRange><enumeratedValues derivedFrom="CENe"/></field>
            <field><name>CKD</name><lsb>8</lsb><msb>9</msb></field>
          </fields>
        </register>
        <register><name>SR</name><addressOffset>0x10</addressOffset><access>read-only</access><size>16</size>
          <readAction>clear</readAction>
          <fields><field><name>UIF</name><bitOffset>0</bitOffset></field></fields>
        </register>
        <register derivedFrom="CR1"><name>CR2</name><addressOffset>0x4</addressOffset></register>
        <register>
          <dim>3</dim><dimIncrement>0x4</dimIncrement><dimIndex>1-3</dimIndex>
          <name>CCR%s</name><addressOffset>0x34</addressOffset>
        </register>
        <cluster>
          <dim>2</dim><dimIncrement>0x20</dimIncrement>
          <name>CH%s</name><addressOffset>0x100</addressOffset>
          <size>16</size>
          <register><name>CTRL</name><addressOffset>0x0</addressOffset></register>
          <register><name>ARR[%s]</name><dim>2</dim><dimIncrement>2</dimIncrement><dimIndex>0,1</dimIndex><addressOffset>0x4</addressOffset></register>
        </cluster>
      </registers>
    </peripheral>
    <peripheral derivedFrom="TIM1">
      <name>TIM2</name>
      <baseAddress>0x40000000</baseAddress>
      <registers>
        <register><name>SR</name><addressOffset>0x10</addressOffset><fields><field><name>OVF</name><bitOffset>3</bitOffset><bitWidth>2</bitWidth></field></fields></register>
        <register><name>EXTRA</name><addressOffset>0x80</addressOffset></register>
      </registers>
    </peripheral>
    <peripheral derivedFrom="LATER">
      <name>BEFORE_ITS_BASE</name>
      <baseAddress>0x50000000</baseAddress>
    </peripheral>
    <peripheral>
      <name>LATER</name>
      <baseAddress>0x60000000</baseAddress>
      <registers><register><name>R</name><addressOffset>0</addressOffset></register></registers>
    </peripheral>
    <peripheral>
      <name>PORT%s</name><dim>2</dim><dimIncrement>0x400</dimIncrement><dimIndex>A,B</dimIndex>
      <baseAddress>0x48000000</baseAddress>
      <registers><register><name>ODR</name><addressOffset>0x14</addressOffset><size>16</size></register></registers>
    </peripheral>
  </peripherals>
</device>"##;

    #[test]
    fn numbers_come_in_the_svd_forms() {
        assert_eq!(parse_number("42"), Some(42));
        assert_eq!(parse_number(" 0x2A "), Some(42));
        assert_eq!(parse_number("0X2a"), Some(42));
        assert_eq!(parse_number("#101010"), Some(42));
        assert_eq!(parse_number("0b11"), Some(3));
        assert_eq!(parse_number("#1x0"), None, "don't-care bits are not a value");
        assert_eq!(parse_number("-1"), None);
        assert_eq!(parse_number("0x"), None);
        assert_eq!(parse_number(""), None);
    }

    fn register<'a>(p: &'a Peripheral, name: &str) -> &'a Register {
        p.register(name).unwrap_or_else(|| panic!("no register {name} in {}: {:?}", p.name, p.registers.iter().map(|r| &r.name).collect::<Vec<_>>()))
    }

    #[test]
    fn a_chip_is_read_with_its_peripherals_in_address_order() {
        let svd = parse(CHIP).unwrap();
        assert_eq!((svd.device.as_str(), svd.description.as_deref()), ("TESTCHIP", Some("A chip & its \"registers\"")), "entities are decoded");
        let names: Vec<&str> = svd.peripherals.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["TIM2", "TIM1", "PORTA", "PORTB", "BEFORE_ITS_BASE", "LATER"]);
        let tim1 = svd.peripheral("tim1").unwrap();
        assert_eq!((tim1.base, tim1.group.as_deref(), tim1.description.as_deref()), (0x4001_0000, Some("TIM"), Some("Timer one")), "whitespace is collapsed");
        // Arrays: `dim` instances, `%s` and `[%s]` filled from `dimIndex` (a range, a list, or counting), the cluster's own offset and size.
        let regs: Vec<(&str, u64, u32)> = tim1.registers.iter().map(|r| (r.name.as_str(), r.offset, r.size)).collect();
        assert_eq!(
            regs,
            [
                ("CR1", 0x0, 32),
                ("CR2", 0x4, 32),
                ("SR", 0x10, 16),
                ("CCR1", 0x34, 32),
                ("CCR2", 0x38, 32),
                ("CCR3", 0x3C, 32),
                ("CH0.CTRL", 0x100, 16),
                ("CH0.ARR[0]", 0x104, 16),
                ("CH0.ARR[1]", 0x106, 16),
                ("CH1.CTRL", 0x120, 16),
                ("CH1.ARR[0]", 0x124, 16),
                ("CH1.ARR[1]", 0x126, 16),
            ]
        );
        // Arrays of peripherals: the base moves by dimIncrement, the registers are shared.
        let ports: Vec<(&str, u64)> = svd.peripherals.iter().filter(|p| p.name.starts_with("PORT")).map(|p| (p.name.as_str(), p.base)).collect();
        assert_eq!(ports, [("PORTA", 0x4800_0000), ("PORTB", 0x4800_0400)]);
        assert_eq!(register(svd.peripheral("PORTB").unwrap(), "ODR").size, 16);
    }

    #[test]
    fn fields_carry_their_bits_access_and_enumerated_values() {
        let svd = parse(CHIP).unwrap();
        let cr1 = register(svd.peripheral("TIM1").unwrap(), "CR1");
        assert_eq!((cr1.reset_value, cr1.access, cr1.read_action), (Some(0), Access::ReadWrite, false));
        let fields: Vec<(&str, u32, u32)> = cr1.fields.iter().map(|f| (f.name.as_str(), f.bit_offset, f.bit_width)).collect();
        assert_eq!(fields, [("CEN", 0, 1), ("CMS", 5, 2), ("CKD", 8, 2)], "bitOffset/bitWidth, bitRange [6:5] and lsb/msb");
        let cen = cr1.field("cen").unwrap();
        // A don't-care pattern (#x1) is no value; `#1` is binary 1.
        assert_eq!(cen.values.iter().map(|v| (v.value, v.name.as_str())).collect::<Vec<_>>(), [(0, "Disabled"), (1, "Enabled")]);
        assert_eq!(cen.values[1].description.as_deref(), Some("on"));
        assert_eq!(cr1.field("CMS").unwrap().values, cen.values, "enumeratedValues derivedFrom");
        assert!(cr1.field("CKD").unwrap().values.is_empty());
        // Reading and writing fields of a register value.
        assert_eq!(cr1.field("CMS").unwrap().extract(0x61), 3);
        assert_eq!(cen.value_name(1), Some("Enabled"));
        assert_eq!(cen.value_name(7), None);
        let cms = cr1.field("CMS").unwrap();
        assert_eq!(cr1.with_field(0x61, cms, 1).unwrap(), 0x21);
        assert_eq!(cr1.with_field(0x21, cms, 0).unwrap(), 0x01);
        assert!(cr1.with_field(0, cms, 4).unwrap_err().contains("2 bits wide"));
        assert!(cr1.with_field(0, cen, 2).unwrap_err().contains("1 bit wide"));
        assert_eq!(mask(64), u64::MAX);
        assert_eq!(mask(1), 1);
    }

    #[test]
    fn derived_peripherals_and_registers_inherit_and_override() {
        let svd = parse(CHIP).unwrap();
        let (tim1, tim2) = (svd.peripheral("TIM1").unwrap(), svd.peripheral("TIM2").unwrap());
        assert_eq!((tim2.base, tim2.group.as_deref(), tim2.description.as_deref()), (0x4000_0000, Some("TIM"), Some("Timer one")), "base is its own, the rest is TIM1's");
        // Inherited, a register derived from another (same fields, its own offset), and one replaced.
        assert_eq!(register(tim2, "CR1"), register(tim1, "CR1"));
        let (cr2, cr1) = (register(tim1, "CR2"), register(tim1, "CR1"));
        assert_eq!((cr2.offset, cr2.fields.clone()), (0x4, cr1.fields.clone()));
        let sr = register(tim2, "SR");
        assert_eq!((sr.size, sr.access, sr.read_action), (32, Access::ReadWrite, false), "its own definition replaces TIM1's whole");
        assert_eq!(sr.fields.iter().map(|f| (f.name.as_str(), f.bit_offset, f.bit_width)).collect::<Vec<_>>(), [("OVF", 3, 2)]);
        assert_eq!(register(tim2, "EXTRA").offset, 0x80);
        // TIM1's own SR: 16 bits, read-only, and reading it clears flags: the view will not read it unasked.
        let sr1 = register(tim1, "SR");
        assert_eq!((sr1.size, sr1.access, sr1.read_action), (16, Access::ReadOnly, true));
        assert!(sr1.access.readable() && !sr1.access.writable());
        // A peripheral listed before the one it derives from.
        let early = svd.peripheral("BEFORE_ITS_BASE").unwrap();
        assert_eq!((early.base, early.registers.len(), early.registers[0].name.as_str()), (0x5000_0000, 1, "R"));
    }

    #[test]
    fn bad_files_are_refused_with_a_reason_that_does_not_quote_them() {
        assert!(parse("<root/>").unwrap_err().contains("no <device>"));
        let secret = "<device><name>SECRETTEXT</name><peripherals><peripheral>";
        let e = parse(secret).unwrap_err();
        assert!(e.contains("ends inside an element") && !e.contains("SECRET"), "{e}");
        let e = parse("<device><name>SECRETTEXT</name></oops>").unwrap_err();
        assert!(e.contains("not valid XML") && !e.contains("SECRET"), "{e}");
        let missing = r#"<device><peripherals><peripheral derivedFrom="NOPE"><name>X</name><baseAddress>0</baseAddress></peripheral></peripherals></device>"#;
        assert!(parse(missing).unwrap_err().contains("derives from \"NOPE\""));
        let cycle = r#"<device><peripherals>
            <peripheral derivedFrom="B"><name>A</name><baseAddress>0</baseAddress></peripheral>
            <peripheral derivedFrom="A"><name>B</name><baseAddress>4</baseAddress></peripheral></peripherals></device>"#;
        assert!(parse(cycle).unwrap_err().contains("which is not in the file"));
        let nobase = r#"<device><peripherals><peripheral><name>X</name></peripheral></peripherals></device>"#;
        assert!(parse(nobase).unwrap_err().contains("no baseAddress"));
        let dim = r#"<device><peripherals><peripheral><name>X%s</name><dim>999999</dim><baseAddress>0</baseAddress></peripheral></peripherals></device>"#;
        assert!(parse(dim).unwrap_err().contains("dim 999999"));
        let nofield = r#"<device><peripherals><peripheral><name>X</name><baseAddress>0</baseAddress><registers><register><name>R</name><addressOffset>0</addressOffset><fields><field><name>F</name></field></fields></register></registers></peripheral></peripherals></device>"#;
        assert!(parse(nofield).unwrap_err().contains("field F: no bit range"));
        let badderived = r#"<device><peripherals><peripheral><name>X</name><baseAddress>0</baseAddress><registers><register derivedFrom="GONE"><name>R</name><addressOffset>0</addressOffset></register></registers></peripheral></peripherals></device>"#;
        assert!(parse(badderived).unwrap_err().contains("derivedFrom \"GONE\""));
        // Nesting is bounded.
        let deep = format!("<device>{}{}</device>", "<a>".repeat(MAX_DEPTH + 5), "</a>".repeat(MAX_DEPTH + 5));
        assert!(parse(&deep).unwrap_err().contains("nested deeper"));
    }

    /// A vendor's own file (`WORKBENCH_TEST_SVD=<path to STM32F407.svd>`; skipped without it):
    /// the facts of that chip, and a parse that takes well under a second.
    #[test]
    fn a_real_vendor_file_is_read() {
        let Some(path) = std::env::var_os("WORKBENCH_TEST_SVD") else {
            eprintln!("skipped: set WORKBENCH_TEST_SVD to STMicroelectronics' STM32F407.svd");
            return;
        };
        let started = std::time::Instant::now();
        let svd = load(Path::new(&path)).unwrap();
        let took = started.elapsed();
        let registers: usize = svd.peripherals.iter().map(|p| p.registers.len()).sum();
        let fields: usize = svd.peripherals.iter().flat_map(|p| &p.registers).map(|r| r.fields.len()).sum();
        eprintln!("{}: {} peripherals, {registers} registers, {fields} fields in {took:?}", svd.device, svd.peripherals.len());
        assert!(svd.device.starts_with("STM32F407"), "{}", svd.device);
        assert!(took < std::time::Duration::from_secs(2), "{took:?}");
        let gpioa = svd.peripheral("GPIOA").unwrap();
        assert_eq!(gpioa.base, 0x4002_0000);
        assert_eq!(register(gpioa, "MODER").offset, 0x0);
        assert_eq!(register(gpioa, "ODR").offset, 0x14);
        let moder0 = register(gpioa, "MODER").field("MODER0").unwrap();
        assert_eq!((moder0.bit_offset, moder0.bit_width), (0, 2));
        assert_eq!(moder0.description.as_deref(), Some("Port x configuration bits (y = 0..15)"), "a description that wraps lines is one line");
        // Peripherals derived from others (the other timers, the USARTs…) have registers too.
        let tim2 = svd.peripheral("TIM2").unwrap();
        assert_eq!(tim2.base, 0x4000_0000);
        assert!(tim2.registers.iter().any(|r| r.name == "CR1" && r.fields.iter().any(|f| f.name == "CEN")), "{:?}", tim2.registers.iter().map(|r| &r.name).take(8).collect::<Vec<_>>());
        assert!(svd.peripherals.len() > 60 && registers > 1000, "{} peripherals, {registers} registers", svd.peripherals.len());
    }

    #[test]
    fn files_are_read_only_when_they_are_svd_files_of_a_sensible_size() {
        let d = tempfile::tempdir().unwrap();
        let good = d.path().join("chip.SVD");
        std::fs::write(&good, CHIP).unwrap();
        assert_eq!(load(&good).unwrap().device, "TESTCHIP");
        // Anything else is not read as XML at all (a repository could point `svd` at any file).
        let other = d.path().join("passwd");
        std::fs::write(&other, "root:x:0:0\n").unwrap();
        assert!(load(&other).unwrap_err().contains("is not an SVD file"));
        let text = d.path().join("notes.xml");
        std::fs::write(&text, "root:x:0:0 SECRETTEXT").unwrap();
        let e = load(&text).unwrap_err();
        assert!(e.contains("no <device>") && !e.contains("SECRET"), "{e}");
        assert!(load(&d.path().join("missing.svd")).unwrap_err().contains("cannot read"));
        std::fs::create_dir(d.path().join("dir.svd")).unwrap();
        assert!(load(&d.path().join("dir.svd")).unwrap_err().contains("regular file"));
    }
}
