//! Scratch coercions and a compact ABI value. Numbers travel through JIT code as
//! unboxed IEEE-754 bits; nonnumeric values use reserved quiet-NaN payloads.
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScratchValue {
    Number(f64),
    String(String),
    Boolean(bool),
}
impl ScratchValue {
    pub fn to_number(&self) -> f64 {
        match self {
            Self::Number(n) => {
                if n.is_nan() {
                    0.0
                } else {
                    *n
                }
            }
            Self::Boolean(b) => u8::from(*b) as f64,
            Self::String(s) => parse_number(s).unwrap_or(0.0),
        }
    }
    pub fn to_bool(&self) -> bool {
        match self {
            Self::Number(n) => *n != 0.0 && !n.is_nan(),
            Self::Boolean(b) => *b,
            Self::String(s) => !s.is_empty() && s != "0" && !s.eq_ignore_ascii_case("false"),
        }
    }
    pub fn text(&self) -> String {
        match self {
            Self::Number(n) => n.to_string(),
            Self::String(s) => s.clone(),
            Self::Boolean(b) => b.to_string(),
        }
    }
    pub fn compare(&self, rhs: &Self) -> std::cmp::Ordering {
        fn numeric(v: &ScratchValue) -> Option<f64> {
            match v {
                ScratchValue::String(s) if s.trim().is_empty() => None,
                ScratchValue::String(s) => parse_number(s),
                _ => Some(v.to_number()),
            }
        }
        if let (Some(a), Some(b)) = (numeric(self), numeric(rhs)) {
            a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
        } else {
            self.text().to_lowercase().cmp(&rhs.text().to_lowercase())
        }
    }
}
fn parse_number(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return Some(0.0);
    }
    let radix = if s.starts_with("0x") || s.starts_with("0X") {
        Some(16)
    } else if s.starts_with("0b") || s.starts_with("0B") {
        Some(2)
    } else if s.starts_with("0o") || s.starts_with("0O") {
        Some(8)
    } else {
        None
    };
    let n = if let Some(radix) = radix {
        u64::from_str_radix(&s[2..], radix).ok()? as f64
    } else {
        s.parse().ok()?
    };
    if n.is_nan() { None } else { Some(n) }
}

pub(crate) const STRING_TAG: u64 = 0x7ffc_0000_0000_0000;
pub(crate) const BOOL_TAG: u64 = 0x7ffd_0000_0000_0000;
pub(crate) fn number(n: f64) -> u64 {
    if n.is_nan() {
        f64::NAN.to_bits()
    } else {
        n.to_bits()
    }
}
#[derive(Default)]
pub(crate) struct Values {
    strings: Vec<Option<String>>,
    interned: HashMap<String, usize>,
    free: Vec<usize>,
}
impl Values {
    pub fn encode(&mut self, value: ScratchValue) -> u64 {
        match value {
            ScratchValue::Number(n) => number(n),
            ScratchValue::Boolean(b) => BOOL_TAG | u64::from(b),
            ScratchValue::String(s) => {
                let id = if let Some(id) = self.interned.get(&s) {
                    *id
                } else {
                    let id = self.free.pop().unwrap_or(self.strings.len());
                    if id == self.strings.len() {
                        self.strings.push(Some(s.clone()));
                    } else {
                        self.strings[id] = Some(s.clone());
                    }
                    self.interned.insert(s, id);
                    id
                };
                STRING_TAG | id as u64
            }
        }
    }
    pub fn decode(&self, value: u64) -> ScratchValue {
        match value & 0xffff_0000_0000_0000 {
            STRING_TAG => ScratchValue::String(
                self.strings[(value & 0xffff_ffff) as usize]
                    .as_ref()
                    .expect("live string handle")
                    .clone(),
            ),
            BOOL_TAG => ScratchValue::Boolean(value & 1 != 0),
            _ => ScratchValue::Number(f64::from_bits(value)),
        }
    }
    pub fn collect(&mut self, roots: impl Iterator<Item = u64>) {
        let live: HashSet<usize> = roots
            .filter(|v| v & 0xffff_0000_0000_0000 == STRING_TAG)
            .map(|v| (v & 0xffff_ffff) as usize)
            .collect();
        for (id, entry) in self.strings.iter_mut().enumerate() {
            if !live.contains(&id)
                && let Some(s) = entry.take()
            {
                self.interned.remove(&s);
                self.free.push(id);
            }
        }
    }
}
