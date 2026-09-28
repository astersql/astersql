// Copyright 2026 AsterSQL.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::Duration;

use super::tikv_mode::{
    ModeClient, NewTiKVModeSwitcher, Store, StoreCatalog, StoreState, SwitchMode,
};
use super::{CancellationToken, KeyRange, Result};

struct Catalog {
    stores: Vec<Store>,
}

impl StoreCatalog for Catalog {
    fn Stores(&self, _token: &CancellationToken) -> Result<Vec<Store>> {
        Ok(self.stores.clone())
    }
}

struct Client {
    calls: Mutex<Vec<(u64, SwitchMode, Vec<KeyRange>)>>,
    entered: AtomicUsize,
    overlap: Barrier,
}

impl ModeClient for Client {
    fn SwitchMode(
        &self,
        _token: &CancellationToken,
        store: &Store,
        mode: SwitchMode,
        ranges: &[KeyRange],
    ) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push((store.id, mode, ranges.to_vec()));
        self.entered.fetch_add(1, Ordering::SeqCst);
        self.overlap.wait();
        Ok(())
    }
}

#[test]
fn switches_up_and_offline_stores_concurrently() {
    let catalog = Arc::new(Catalog {
        stores: vec![
            store(1, StoreState::Up),
            store(2, StoreState::Offline),
            store(3, StoreState::Tombstone),
            store(4, StoreState::Disconnected),
        ],
    });
    let client = Arc::new(Client {
        calls: Mutex::new(Vec::new()),
        entered: AtomicUsize::new(0),
        overlap: Barrier::new(2),
    });
    let ranges = vec![KeyRange {
        start: b"a".to_vec(),
        end: b"z".to_vec(),
    }];
    let switcher = NewTiKVModeSwitcher(catalog, client.clone());

    let token = CancellationToken::default();
    let worker = std::thread::spawn(move || switcher.ToImportMode(&token, &ranges));
    for _ in 0..100 {
        if client.entered.load(Ordering::SeqCst) == 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    assert_eq!(client.entered.load(Ordering::SeqCst), 2);
    worker.join().unwrap();
    let mut calls = client.calls.lock().unwrap().clone();
    calls.sort_by_key(|call| call.0);
    assert_eq!(
        calls,
        vec![
            (
                1,
                SwitchMode::Import,
                vec![KeyRange {
                    start: b"a".to_vec(),
                    end: b"z".to_vec()
                }]
            ),
            (
                2,
                SwitchMode::Import,
                vec![KeyRange {
                    start: b"a".to_vec(),
                    end: b"z".to_vec()
                }]
            )
        ]
    );
}

fn store(id: u64, state: StoreState) -> Store {
    Store {
        id,
        address: format!("tikv-{id}"),
        state,
    }
}
