//! Scratch coercions and a compact ABI value. Numbers travel through JIT code as
//! unboxed IEEE-754 bits; nonnumeric values use reserved quiet-NaN payloads.
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
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
/// A bounded, stable-address coercion table for native loads. Larger string
/// handles retain the helper fallback; text and comparison semantics stay intact.
pub(crate) const NUMERIC_CACHE_LEN: usize = 65_536;
pub(crate) struct Values {
    strings: Vec<Option<InternedString>>,
    interned: HashMap<String, usize>,
    free: Vec<usize>,
    dirty: bool,
    numeric_cache: Box<[f64]>,
}
impl Default for Values {
    fn default() -> Self {
        Self {
            strings: Vec::new(),
            interned: HashMap::new(),
            free: Vec::new(),
            dirty: false,
            numeric_cache: vec![0.; NUMERIC_CACHE_LEN].into_boxed_slice(),
        }
    }
}
struct InternedString {
    text: String,
    // Empty/whitespace strings participate in lexical comparisons, while
    // arithmetic converts them to zero. Invalid numeric strings do likewise.
    comparison_number: Option<f64>,
}
impl Values {
    pub fn numeric_cache_ptr(&self) -> *const f64 {
        self.numeric_cache.as_ptr()
    }
    pub fn encode(&mut self, value: ScratchValue) -> u64 {
        match value {
            ScratchValue::Number(n) => number(n),
            ScratchValue::Boolean(b) => BOOL_TAG | u64::from(b),
            ScratchValue::String(s) => {
                let id = if let Some(id) = self.interned.get(&s) {
                    *id
                } else {
                    self.dirty = true;
                    let id = self.free.pop().unwrap_or(self.strings.len());
                    let entry = InternedString {
                        comparison_number: if s.trim().is_empty() {
                            None
                        } else {
                            parse_number(&s)
                        },
                        text: s.clone(),
                    };
                    if let Some(cached) = self.numeric_cache.get_mut(id) {
                        *cached = entry.comparison_number.unwrap_or(0.);
                    }
                    if id == self.strings.len() {
                        self.strings.push(Some(entry));
                    } else {
                        self.strings[id] = Some(entry);
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
                    .text
                    .clone(),
            ),
            BOOL_TAG => ScratchValue::Boolean(value & 1 != 0),
            _ => ScratchValue::Number(f64::from_bits(value)),
        }
    }
    fn string(&self, value: u64) -> &InternedString {
        self.strings[(value & 0xffff_ffff) as usize]
            .as_ref()
            .expect("live string handle")
    }
    pub fn comparison_number(&self, value: u64) -> Option<f64> {
        match value & 0xffff_0000_0000_0000 {
            STRING_TAG => self.string(value).comparison_number,
            BOOL_TAG => Some((value & 1) as f64),
            _ => {
                let n = f64::from_bits(value);
                Some(if n.is_nan() { 0.0 } else { n })
            }
        }
    }
    pub fn to_number(&self, value: u64) -> f64 {
        self.comparison_number(value).unwrap_or(0.0)
    }
    pub fn to_bool(&self, value: u64) -> bool {
        match value & 0xffff_0000_0000_0000 {
            STRING_TAG => {
                let s = &self.string(value).text;
                !s.is_empty() && s != "0" && !s.eq_ignore_ascii_case("false")
            }
            BOOL_TAG => value & 1 != 0,
            _ => {
                let n = f64::from_bits(value);
                n != 0.0 && !n.is_nan()
            }
        }
    }
    fn text(&self, value: u64) -> Cow<'_, str> {
        match value & 0xffff_0000_0000_0000 {
            STRING_TAG => Cow::Borrowed(&self.string(value).text),
            BOOL_TAG => Cow::Borrowed(if value & 1 != 0 { "true" } else { "false" }),
            _ => Cow::Owned(f64::from_bits(value).to_string()),
        }
    }
    pub fn compare(&self, a: u64, b: u64) -> std::cmp::Ordering {
        if let (Some(a), Some(b)) = (self.comparison_number(a), self.comparison_number(b)) {
            return a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal);
        }
        let a = self.text(a);
        let b = self.text(b);
        if a.is_ascii() && b.is_ascii() {
            a.bytes()
                .map(|c| c.to_ascii_lowercase())
                .cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
        } else {
            a.to_lowercase().cmp(&b.to_lowercase())
        }
    }
    pub fn collect(&mut self, roots: impl Iterator<Item = u64>) {
        // No new strings means no memory growth. In particular, numeric RAM
        // operations and broadcast RPCs need no root walk or arena scan.
        if !std::mem::take(&mut self.dirty) {
            return;
        }
        let live: HashSet<usize> = roots
            .filter(|v| v & 0xffff_0000_0000_0000 == STRING_TAG)
            .map(|v| (v & 0xffff_ffff) as usize)
            .collect();
        for (id, entry) in self.strings.iter_mut().enumerate() {
            if !live.contains(&id)
                && let Some(s) = entry.take()
            {
                self.interned.remove(&s.text);
                self.free.push(id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cached_coercions_and_borrowed_comparisons_match_scratch_values() {
        let mut values = Values::default();
        let mut samples: Vec<_> = [
            "",
            " ",
            "\t\n",
            "0",
            "-0",
            "0.0",
            "0x10",
            "0b11",
            "0o17",
            "-2.5",
            "1e3",
            "NaN",
            "Infinity",
            "-Infinity",
            "false",
            "FALSE",
            "true",
            "Stone",
            "STONE",
            "stoned",
            "é",
            "É",
            "İ",
            "i\u{307}",
        ]
        .into_iter()
        .map(|s| ScratchValue::String(s.into()))
        .collect();
        samples.extend([ScratchValue::Boolean(false), ScratchValue::Boolean(true)]);
        samples.extend(
            [
                0.0,
                -0.0,
                1.0,
                -1.0,
                16.0,
                1e100,
                f64::NAN,
                f64::INFINITY,
                f64::NEG_INFINITY,
            ]
            .map(ScratchValue::Number),
        );
        let encoded: Vec<_> = samples.iter().cloned().map(|s| values.encode(s)).collect();
        for (i, a) in samples.iter().enumerate() {
            assert_eq!(values.to_number(encoded[i]), a.to_number(), "{a:?}");
            if encoded[i] & 0xffff_0000_0000_0000 == STRING_TAG {
                let id = (encoded[i] & 0xffff_ffff) as usize;
                assert_eq!(
                    values.numeric_cache[id],
                    a.to_number(),
                    "native cache: {a:?}"
                );
            }
            assert_eq!(values.to_bool(encoded[i]), a.to_bool(), "{a:?}");
            for (j, b) in samples.iter().enumerate() {
                assert_eq!(
                    values.compare(encoded[i], encoded[j]),
                    a.compare(b),
                    "{a:?} vs {b:?}"
                );
            }
        }
        let mut values = Values::default();
        let old = values.encode(ScratchValue::String("42".into()));
        values.collect(std::iter::empty());
        let reused = values.encode(ScratchValue::String("FALSE".into()));
        assert_eq!(old, reused);
        assert_eq!(values.to_number(reused), 0.0);
        assert!(!values.to_bool(reused));
        assert_eq!(values.decode(reused), ScratchValue::String("FALSE".into()));
        assert_eq!(values.numeric_cache[(reused & 0xffff_ffff) as usize], 0.);
    }
}
