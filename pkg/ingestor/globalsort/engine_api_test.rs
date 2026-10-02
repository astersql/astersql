// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::*;
use api::Engine as _;

fn source() -> ExternalEngineAdapter {
    let store = Arc::new(crate::MemoryStorage::default());
    let pairs = (1u8..=3)
        .map(|key| crate::KvPair {
            key: vec![key],
            value: vec![key + 10],
        })
        .collect::<Vec<_>>();
    store.write("data", crate::encode_kvs(&pairs)).unwrap();
    store.write("stat", Vec::new()).unwrap();
    let engine = NewExternalEngine(
        store,
        vec!["data".into()],
        vec!["stat".into()],
        vec![1],
        vec![4],
        (1u8..=4).map(|key| vec![key]).collect(),
        vec![vec![1], vec![4]],
        1,
        123,
        6,
        3,
        false,
        1024,
        OnDuplicateKey::Error,
        "adapter".into(),
    )
    .unwrap();
    ExternalEngineAdapter::new(engine, Default::default())
}

#[test]
fn native_api_consumes_bounded_batches_and_shared_import_statistics() {
    let source = Arc::new(source());
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let context = api::Context::background();
    let producer = std::thread::spawn({
        let source = source.clone();
        let context = context.clone();
        move || source.LoadIngestData(&context, &tx)
    });
    let mut pool = astersql_lightning_membuf::NewPool(Vec::new());
    let mut rows = Vec::new();
    let mut ranges = Vec::new();
    for batch in rx {
        assert_eq!(batch.SortedRanges.len(), 1);
        assert_eq!(batch.Data.GetTS(), 123);
        batch.Data.IncRef();
        let bounds = &batch.SortedRanges[0];
        ranges.push((bounds.Start.clone(), bounds.End.clone()));
        let mut iter = batch.Data.NewIter(
            &context,
            &bounds.Start,
            &bounds.End,
            Arc::get_mut(&mut pool).unwrap(),
        );
        let mut valid = iter.First();
        while valid {
            rows.push((iter.Key().to_vec(), iter.Value().to_vec()));
            valid = iter.Next();
        }
        assert!(iter.Error().is_none());
        iter.Close().unwrap();
        batch.Data.Finish(2, 1);
        batch.Data.DecRef();
    }
    producer.join().unwrap().unwrap();
    assert_eq!(
        rows,
        vec![
            (vec![1], vec![11]),
            (vec![2], vec![12]),
            (vec![3], vec![13])
        ]
    );
    assert_eq!(
        ranges,
        vec![(vec![1], vec![2]), (vec![2], vec![3]), (vec![3], vec![4])]
    );
    assert_eq!(source.KVStatistics(), (6, 3));
    assert_eq!(source.ImportedStatistics(), (6, 3));
}

#[test]
fn native_api_cancels_a_full_output_channel_and_releases_unsent_data() {
    let source = Arc::new(source());
    let (tx, _rx) = std::sync::mpsc::sync_channel(0);
    let context = api::Context::background();
    let producer = std::thread::spawn({
        let source = source.clone();
        let context = context.clone();
        move || source.LoadIngestData(&context, &tx)
    });
    std::thread::sleep(Duration::from_millis(20));
    context.cancel();
    let error = producer.join().unwrap().unwrap_err();
    assert!(matches!(
        error.downcast_ref::<Error>(),
        Some(Error::Cancelled)
    ));
    let engine = source.engine.lock().unwrap();
    assert_eq!(engine.in_flight_data_count.load(Ordering::Acquire), 0);
    assert_eq!(source.ImportedStatistics(), (0, 0));
}

#[test]
fn native_api_discarded_unreferenced_batches_release_memory_budget() {
    let source = source();
    let (tx, rx) = std::sync::mpsc::sync_channel(3);
    source.LoadIngestData(&Default::default(), &tx).unwrap();
    drop(rx);
    let engine = source.engine.lock().unwrap();
    assert_eq!(engine.in_flight_data_count.load(Ordering::Acquire), 0);
    assert_eq!(engine.in_flight_bytes.load(Ordering::Acquire), 0);
    assert_eq!(source.ImportedStatistics(), (0, 0));
}
