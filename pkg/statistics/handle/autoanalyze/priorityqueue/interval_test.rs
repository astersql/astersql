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

use crate::{
    AVG_DURATION_QUERY_FOR_PARTITION, AVG_DURATION_QUERY_FOR_TABLE, AnalysisHistoryReader,
    DEFAULT_FAILED_ANALYSIS_WAIT_TIME, GetAverageAnalysisDuration, GetLastFailedAnalysisDuration,
    LAST_FAILED_DURATION_QUERY_FOR_PARTITION, LAST_FAILED_DURATION_QUERY_FOR_TABLE,
};
use std::{cell::RefCell, time::Duration};

#[derive(Clone, Debug, Eq, PartialEq)]
struct QueryCall {
    sql: String,
    params: Vec<String>,
}

struct History {
    average: Result<Option<f64>, String>,
    failed: Result<Option<i64>, String>,
    calls: RefCell<Vec<QueryCall>>,
}

impl History {
    fn new(average: Result<Option<f64>, String>, failed: Result<Option<i64>, String>) -> Self {
        Self {
            average,
            failed,
            calls: RefCell::new(Vec::new()),
        }
    }

    fn take_call(&self) -> QueryCall {
        self.calls.borrow_mut().pop().expect("a query was executed")
    }
}

impl AnalysisHistoryReader for History {
    fn query_optional_f64(&self, sql: &str, params: &[String]) -> Result<Option<f64>, String> {
        self.calls.borrow_mut().push(QueryCall {
            sql: sql.to_owned(),
            params: params.to_vec(),
        });
        self.average.clone()
    }

    fn query_optional_i64(&self, sql: &str, params: &[String]) -> Result<Option<i64>, String> {
        self.calls.borrow_mut().push(QueryCall {
            sql: sql.to_owned(),
            params: params.to_vec(),
        });
        self.failed.clone()
    }
}

#[test]
fn average_duration_matches_go_result_mapping() {
    let history = History::new(Ok(Some(12.9)), Ok(None));
    assert_eq!(
        Some(Duration::from_secs(12)),
        GetAverageAnalysisDuration(&history, "db", "t", &[]).unwrap()
    );
    assert_eq!(
        QueryCall {
            sql: AVG_DURATION_QUERY_FOR_TABLE.to_owned(),
            params: vec!["db".into(), "t".into()]
        },
        history.take_call()
    );

    for average in [None, Some(-1.0)] {
        let history = History::new(Ok(average), Ok(None));
        assert_eq!(
            None,
            GetAverageAnalysisDuration(&history, "db", "t", &[]).unwrap()
        );
    }
}

#[test]
fn partition_queries_preserve_names_and_order() {
    let partitions = vec!["p0".to_owned(), "p1".to_owned()];
    let history = History::new(Ok(Some(3600.0)), Ok(Some(86_400)));

    assert_eq!(
        Some(Duration::from_secs(3600)),
        GetAverageAnalysisDuration(&history, "db", "t", &partitions).unwrap()
    );
    assert_eq!(
        QueryCall {
            sql: AVG_DURATION_QUERY_FOR_PARTITION.to_owned(),
            params: vec!["db".into(), "t".into(), "p0".into(), "p1".into()]
        },
        history.take_call()
    );

    assert_eq!(
        Some(Duration::from_secs(86_400)),
        GetLastFailedAnalysisDuration(&history, "db", "t", &partitions).unwrap()
    );
    assert_eq!(
        QueryCall {
            sql: LAST_FAILED_DURATION_QUERY_FOR_PARTITION.to_owned(),
            params: vec!["db".into(), "t".into(), "p0".into(), "p1".into()]
        },
        history.take_call()
    );
}

#[test]
fn last_failed_duration_matches_go_sentinels_and_fallback() {
    for (failed, expected) in [
        (None, None),
        (Some(0), Some(Duration::ZERO)),
        (Some(-1), Some(DEFAULT_FAILED_ANALYSIS_WAIT_TIME)),
        (Some(10), Some(Duration::from_secs(10))),
    ] {
        let history = History::new(Ok(None), Ok(failed));
        assert_eq!(
            expected,
            GetLastFailedAnalysisDuration(&history, "db", "t", &[]).unwrap()
        );
        assert_eq!(
            QueryCall {
                sql: LAST_FAILED_DURATION_QUERY_FOR_TABLE.to_owned(),
                params: vec!["db".into(), "t".into()]
            },
            history.take_call()
        );
    }
}

#[test]
fn query_errors_are_propagated_unchanged() {
    let average_error = History::new(Err("average query failed".into()), Ok(None));
    assert_eq!(
        "average query failed",
        GetAverageAnalysisDuration(&average_error, "db", "t", &[]).unwrap_err()
    );

    let failed_error = History::new(Ok(None), Err("failed query failed".into()));
    assert_eq!(
        "failed query failed",
        GetLastFailedAnalysisDuration(&failed_error, "db", "t", &[]).unwrap_err()
    );
}
