// Copyright 2026 AsterSQL.

use crate::ssts::{
    CompactedSSTs, CompactedSSTsType, CopiedSST, CopiedSSTsType, RewrittenSSTs, SSTs,
};
use crate::stubs::backuppb::{
    File, LogFileSubcompaction, LogFileSubcompactionMeta, RewrittenTableID,
};
use crate::stubs::tablecodec;

fn file_for_table(table_id: i64, name: &str) -> File {
    let mut end_key = tablecodec::EncodeTablePrefix(table_id);
    end_key.push(1);
    File {
        Name: name.into(),
        StartKey: tablecodec::EncodeTablePrefix(table_id),
        EndKey: end_key,
        TotalBytes: 42,
        ..Default::default()
    }
}

#[test]
#[should_panic]
fn copied_sst_table_id_panics_without_file_like_go() {
    let copied = CopiedSST::new(None, RewrittenTableID::default());

    copied.TableID();
}

#[test]
fn compacted_ssts_matches_go_collection_contract() {
    let first = file_for_table(7, "first.sst");
    let replacement = file_for_table(7, "replacement.sst");
    let mut compacted = CompactedSSTs::new(LogFileSubcompaction {
        Meta: LogFileSubcompactionMeta { TableId: 7 },
        SstOutputs: vec![first],
    });

    assert_eq!(compacted.Type(), CompactedSSTsType);
    assert_eq!(compacted.TableID(), 7);
    assert_eq!(compacted.GetSSTs().len(), 1);
    assert_eq!(compacted.to_string(), "CompactedSSTs: table:7");

    compacted.SetSSTs(vec![replacement.clone()]);
    assert_eq!(compacted.GetSSTs(), vec![replacement]);
}

#[test]
fn copied_sst_matches_go_rewrite_and_cached_table_contract() {
    let original = file_for_table(11, "original.sst");
    let mut copied = CopiedSST::new(
        Some(original.clone()),
        RewrittenTableID {
            Upstream: 21,
            Downstream: 31,
        },
    );

    assert_eq!(copied.Type(), CopiedSSTsType);
    assert_eq!(copied.TableID(), 11);
    assert_eq!(copied.RewrittenTo(), 21);
    assert_eq!(copied.GetSSTs(), vec![original]);
    assert!(copied.to_string().contains("original.sst"));
    assert!(copied.to_string().contains("TotalBytes: 42"));

    // Go deliberately retains cachedTableID when the selected file is replaced.
    copied.SetSSTs(vec![file_for_table(12, "replacement.sst")]);
    assert_eq!(copied.TableID(), 11);
    copied.SetSSTs(vec![]);
    assert!(copied.GetSSTs().is_empty());
    assert_eq!(copied.to_string(), "CopiedSSTs: <nil>");
}

#[test]
fn copied_sst_rewritten_to_falls_back_to_physical_table_id() {
    let copied = CopiedSST::new(Some(file_for_table(13, "fallback.sst")), Default::default());

    assert_eq!(copied.as_rewritten().unwrap().RewrittenTo(), 13);
}

#[test]
#[should_panic(expected = "yet restoring a SST with two adjacent tables not supported")]
fn copied_sst_rejects_cross_table_range_like_go() {
    let mut file = file_for_table(1, "cross-table.sst");
    file.EndKey = tablecodec::EncodeTablePrefix(2);
    let copied = CopiedSST::new(Some(file), Default::default());

    copied.TableID();
}

#[test]
#[should_panic(expected = "Too many files passed to AddedSSTs.SetSSTs.")]
fn copied_sst_rejects_multiple_files_like_go() {
    let mut copied = CopiedSST::new(Some(file_for_table(1, "one.sst")), Default::default());

    copied.SetSSTs(vec![
        file_for_table(1, "one.sst"),
        file_for_table(1, "two.sst"),
    ]);
}
