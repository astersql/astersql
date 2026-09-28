// Copyright 2026 AsterSQL.

use crate::*;

struct CapturingIterator {
    keys: Vec<Key>,
    position: usize,
}

impl Iterator for CapturingIterator {
    fn Valid(&self) -> bool {
        self.position < self.keys.len()
    }

    fn Key(&self) -> Key {
        self.keys[self.position].clone()
    }

    fn Value(&self) -> Vec<u8> {
        Vec::new()
    }

    fn Next(&mut self) -> Result<(), errors::SharedError> {
        self.position += 1;
        Ok(())
    }

    fn Close(&mut self) {}
}

#[test]
fn next_until_accepts_a_state_capturing_predicate() {
    let mut iter = CapturingIterator {
        keys: vec![Key(vec![1]), Key(vec![2]), Key(vec![3])],
        position: 0,
    };
    let stop_key = Key(vec![2]);
    let mut comparisons = 0;

    NextUntil(&mut iter, |key| {
        comparisons += 1;
        key == stop_key
    })
    .expect("stop at the captured key");

    assert_eq!(iter.position, 1);
    assert_eq!(comparisons, 2);
}
