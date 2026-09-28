// Copyright 2026 AsterSQL.

use super::multi_way_merge::{multiWayMergeSource, newMultiWayMerger};
use super::sort_util::{Result, Row, SortError, SortKey, SortValue, comparator};

struct failOnceSource {
    rows: Vec<Option<Row>>,
    failed: bool,
}

impl multiWayMergeSource for failOnceSource {
    fn init(&mut self) -> Result<()> {
        Ok(())
    }

    fn next(&mut self, partition_id: usize) -> Result<Option<Row>> {
        if self.rows[partition_id].is_none() && !self.failed {
            self.failed = true;
            return Err(SortError("injected source error".into()));
        }
        Ok(self.rows[partition_id].take())
    }

    fn getPartitionNum(&self) -> usize {
        self.rows.len()
    }
}

#[test]
fn next_preserves_heap_top_when_source_errors() {
    let source = failOnceSource {
        rows: vec![Some(Row(vec![SortValue::Int(1)]))],
        failed: false,
    };
    let mut merger = newMultiWayMerger(source, comparator(vec![SortKey::asc(0)]));

    assert_eq!(merger.next().unwrap_err().0, "injected source error");
    assert_eq!(
        merger.next().unwrap(),
        Some(Row(vec![SortValue::Int(1)])),
        "Go keeps the current heap element intact when fetching its successor fails"
    );
    assert_eq!(merger.next().unwrap(), None);
}
