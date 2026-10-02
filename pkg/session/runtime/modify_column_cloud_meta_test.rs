// Copyright 2026 AsterSQL.
use super::modify_column_cloud_meta::{ExternalFields, SortedMeta, decode, encode, read, write};
use astersql_ingestor_globalsort::{MemoryStorage, Storage};
#[test]
fn cloud_meta_roundtrips_go_pointer_and_external_field_json() {
    let store = MemoryStorage::default();
    // Produced by Go's marshalInternalFields/marshalExternalFields contracts.
    let row=br#"{"ExternalPath":"7/9/meta.json","physical_table_id":42,"row_start":"AQ==","row_end":"Ag==","ts":123}"#;
    let external=br#"{"range_job_keys":["eA==","eg=="],"range_split_keys":["eA==","eg=="],"data-files":["p00000000/7/9/a"],"stat-files":["p00000000/7/9/a_stat"],"meta_groups":[{"start-key":"eA==","end-key":"eg==","total-kv-size":11,"total-kv-cnt":2,"multiple-files-stats":[{"min-key":"eA==","max-key":"eQ==","filenames":[["p00000000/7/9/a","p00000000/7/9/a_stat"]],"max-overlapping-num":1}],"conflict-info":{}}],"ele_ids":[55],"start-key":null,"end-key":null,"total-kv-size":0,"total-kv-cnt":0,"multiple-files-stats":null,"conflict-info":{}}"#;
    store.write("7/9/meta.json", external.to_vec()).unwrap();
    let mut merged = read(&store, row).unwrap();
    let fields: ExternalFields = serde_json::from_value(merged.clone()).unwrap();
    assert_eq!(fields.ele_ids, vec![55]);
    assert_eq!(
        fields.meta_groups[0].sort_files()[0].filenames[0].data_file,
        "p00000000/7/9/a"
    );
    assert_eq!(merged["physical_table_id"], 42);
    assert_eq!(merged["ts"], 123);
    assert_eq!(
        decode(fields.meta_groups[0].start.as_deref().unwrap()).unwrap(),
        b"x"
    );
    // A fresh durable row holds only internal fields plus the published pointer.
    merged = serde_json::from_slice(row).unwrap();
    let pointer = write(&store, &mut merged, &fields, "7/10/meta.json".into()).unwrap();
    assert!(
        serde_json::from_slice::<serde_json::Value>(&pointer)
            .unwrap()
            .get("meta_groups")
            .is_none()
    );
    let reread: ExternalFields = serde_json::from_value(read(&store, &pointer).unwrap()).unwrap();
    assert_eq!(reread.meta_groups[0].count, 2);
    assert_eq!(reread.meta_groups[0].size, 11);
    assert_eq!(reread.range_job_keys, vec!["eA==", "eg=="]);
}
#[test]
fn cloud_meta_merge_compares_binary_keys_and_preserves_go_uint64_wrap() {
    let mut meta = SortedMeta {
        start: Some(encode(&[255])),
        end: Some(encode(&[255, 0])),
        size: u64::MAX,
        count: u64::MAX,
        ..Default::default()
    };
    meta.merge(&SortedMeta {
        start: Some(encode(&[0])),
        end: Some(encode(&[0, 0])),
        size: 2,
        count: 3,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(decode(meta.start.as_deref().unwrap()).unwrap(), vec![0]);
    assert_eq!(decode(meta.end.as_deref().unwrap()).unwrap(), vec![255, 0]);
    assert_eq!(meta.size, 1);
    assert_eq!(meta.count, 2);
}
