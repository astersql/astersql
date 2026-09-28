// Copyright 2026 AsterSQL.

use std::sync::Mutex;

use crate::prefix_scanner::{PrefixNextKey, Source, scanPrefix};
use crate::stubs::Entry;

struct FailingSecondPage {
    calls: Mutex<usize>,
}

impl Source for FailingSecondPage {
    fn Scan(&self, _from: &[u8], _to: &[u8], _limit: i32) -> Result<(Vec<Entry>, bool), String> {
        let mut calls = self.calls.lock().unwrap();
        *calls += 1;
        if *calls == 1 {
            return Ok((
                vec![Entry {
                    Key: b"p/1".to_vec(),
                    Value: b"value".to_vec(),
                }],
                true,
            ));
        }
        Err("second page failed".to_string())
    }
}

#[test]
fn prefix_next_key_matches_tikv_overflow_contract() {
    assert_eq!(PrefixNextKey(b""), Vec::<u8>::new());
    assert_eq!(PrefixNextKey(&[0xff]), Vec::<u8>::new());
    assert_eq!(PrefixNextKey(&[0x01, 0xff]), vec![0x02]);
}

#[test]
fn all_pages_error_preserves_entries_from_completed_pages() {
    let source = FailingSecondPage {
        calls: Mutex::new(0),
    };
    let mut scanner = scanPrefix(&source, "p/");

    let error = scanner.AllPages(1).unwrap_err();
    assert_eq!(error.entries.len(), 1);
    assert_eq!(error.entries[0].Key, b"p/1");
    assert_eq!(error.source, "second page failed");
    assert!(!scanner.Done());
}
