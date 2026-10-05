// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

use crate::show_slow_queries::{ShowSlowExec, ShowSlowSource, SlowQueryInfo};
use astersql_types::{
    datum::{Duration as SqlDuration, Time},
    field::NewFieldType,
};
use astersql_util_execdetails::execdetails::util::ScanDetail;
use std::convert::Infallible;
use std::time::Duration;

#[derive(Default)]
struct EmptySource {
    opened: bool,
}

struct OneRowSource;

impl ShowSlowSource for OneRowSource {
    type Request = ();
    type Error = Infallible;

    fn open(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn show_slow_query(&mut self, _request: &Self::Request) -> Vec<SlowQueryInfo> {
        vec![SlowQueryInfo {
            sql: "select * from ia".into(),
            start: Time::default(),
            duration: SqlDuration::default(),
            detail: String::new(),
            scan_detail: Some(ScanDetail {
                IaRemoteReadSegmentCount: 4,
                IaRemoteReadSegmentBytes: 4096,
                IaRemoteReadSegmentDuration: Duration::from_millis(15),
                ..ScanDetail::default()
            }),
            success: true,
            connection_id: 1,
            transaction_ts: 2,
            user: "user".into(),
            database: "test".into(),
            table_ids: String::new(),
            index_names: String::new(),
            internal: false,
            digest: "digest".into(),
            session_alias: String::new(),
        }]
    }

    fn max_chunk_size(&self) -> usize {
        32
    }
}

#[test]
fn next_appends_ia_remote_read_stats_after_existing_columns() {
    let mut executor = ShowSlowExec {
        source: OneRowSource,
        show_slow: (),
        result: Vec::new(),
        cursor: 0,
    };
    executor.Open(()).unwrap();
    let field_types = [15, 7, 11, 15, 1, 8, 8, 15, 15, 15, 15, 1, 15, 15, 8, 8, 5]
        .into_iter()
        .map(|type_code| *NewFieldType(type_code))
        .collect();
    let mut output = *astersql_util_chunk::New(field_types, 1, 1);

    executor.Next((), &mut output).unwrap();

    assert_eq!(output.NumCols(), 17);
    assert_eq!(output.NumRows(), 1);
    let row = output.GetRow(0);
    assert_eq!(row.GetUint64(14), 4);
    assert_eq!(row.GetUint64(15), 4096);
    assert_eq!(row.GetFloat64(16), 0.015);
}

impl ShowSlowSource for EmptySource {
    type Request = ();
    type Error = Infallible;

    fn open(&mut self) -> Result<(), Self::Error> {
        self.opened = true;
        Ok(())
    }

    fn show_slow_query(&mut self, _request: &Self::Request) -> Vec<SlowQueryInfo> {
        Vec::new()
    }

    fn max_chunk_size(&self) -> usize {
        32
    }
}

#[test]
fn open_preserves_cursor_like_go_executor() {
    let mut executor = ShowSlowExec {
        source: EmptySource::default(),
        show_slow: (),
        result: Vec::new(),
        cursor: 7,
    };

    executor.Open(()).unwrap();

    assert!(executor.source.opened);
    assert_eq!(executor.cursor, 7);
}
