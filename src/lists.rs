//! List storage with incremental string roots. Numeric RAM never needs a GC scan.
use crate::value::STRING_TAG;
use std::{collections::HashMap, ops::Deref};

pub(crate) struct RuntimeList {
    values: Vec<u64>,
    strings: HashMap<u64, usize>,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct ListView {
    pub data: *mut u64,
    pub len: usize,
}

impl RuntimeList {
    pub fn new(values: Vec<u64>) -> Self {
        let mut list = Self {
            values,
            strings: HashMap::new(),
        };
        for &value in &list.values {
            if value & 0xffff_0000_0000_0000 == STRING_TAG {
                *list.strings.entry(value).or_default() += 1;
            }
        }
        list
    }
    pub fn view(&mut self) -> ListView {
        ListView {
            data: self.values.as_mut_ptr(),
            len: self.values.len(),
        }
    }
    pub fn roots(&self) -> impl Iterator<Item = u64> + '_ {
        self.strings.keys().copied()
    }
    fn retain(&mut self, value: u64) {
        if value & 0xffff_0000_0000_0000 == STRING_TAG {
            *self.strings.entry(value).or_default() += 1;
        }
    }
    fn release(&mut self, value: u64) {
        if let Some(count) = self.strings.get_mut(&value) {
            *count -= 1;
            if *count == 0 {
                self.strings.remove(&value);
            }
        }
    }
    pub fn push(&mut self, value: u64) {
        self.retain(value);
        self.values.push(value);
    }
    pub fn insert(&mut self, index: usize, value: u64) {
        self.retain(value);
        self.values.insert(index, value);
    }
    pub fn replace(&mut self, index: usize, value: u64) {
        let old = self.values[index];
        if old != value {
            self.release(old);
            self.retain(value);
            self.values[index] = value;
        }
    }
    pub fn remove(&mut self, index: usize) {
        let old = self.values.remove(index);
        self.release(old);
    }
    pub fn pop(&mut self) {
        if let Some(old) = self.values.pop() {
            self.release(old);
        }
    }
    pub fn clear(&mut self) {
        self.values.clear();
        self.strings.clear();
    }
}
impl Deref for RuntimeList {
    type Target = [u64];
    fn deref(&self) -> &Self::Target {
        &self.values
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roots_track_duplicate_strings_replacements_and_removals() {
        let a = STRING_TAG | 1;
        let b = STRING_TAG | 2;
        let mut list = RuntimeList::new(vec![a, a, 0]);
        assert_eq!(list.roots().collect::<Vec<_>>(), vec![a]);
        list.remove(0);
        assert_eq!(list.roots().count(), 1);
        list.replace(0, 0);
        assert_eq!(list.roots().count(), 0);
        list.insert(0, b);
        list.push(b);
        list.pop();
        assert_eq!(list.roots().collect::<Vec<_>>(), vec![b]);
        list.clear();
        assert_eq!(list.roots().count(), 0);
        let view = list.view();
        assert_eq!(view.len, 0);
    }
}
