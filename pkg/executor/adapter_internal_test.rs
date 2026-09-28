// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// `adapter` 模块内部辅助逻辑的单元测试。
//
// 覆盖：
// - `FormatSQL`：将多行/含制表符的 SQL 规范为空格分隔单行，便于日志与摘要展示；
// - `calculateStatementTotalRUV2`：按 RU（Request Unit，资源计量单位）权重汇总
//   读写字节、请求次数与 CPU 相关分量，得到语句总 RU。

use crate::adapter::{
    AdapterResult, FinishErrorRFCCode, FormatSQL, Key, RUDetails, RUV2Metrics, RUV2Weights,
    calculateStatementTotalRUV2, moveWrittenSharedLockKeysToExclusive, pessimisticTxn,
};
use std::collections::{HashMap, HashSet};

struct SharedLockTxnForTest {
    written: HashSet<Key>,
    errors: HashMap<Key, astersql_errors::SharedError>,
}

impl pessimisticTxn for SharedLockTxnForTest {
    fn KeysNeedToLock(&mut self) -> AdapterResult<Vec<Key>> {
        Ok(Vec::new())
    }

    fn IsValid(&self) -> bool {
        true
    }

    fn IsKeyWritten(&self, key: &[u8]) -> AdapterResult<bool> {
        if let Some(error) = self.errors.get(key) {
            return Err(error.clone());
        }
        Ok(self.written.contains(key))
    }
}

#[test]
fn finish_tiflash_error_label_reads_formal_rfc_code_and_defaults_for_plain_errors() {
    let normalized = astersql_errors::SharedError::new(astersql_errors::Normalize(
        "failed",
        &[astersql_errors::RFCCodeText("global:2")],
    ));
    assert_eq!(FinishErrorRFCCode(&normalized).as_deref(), Some("global:2"));
    assert_eq!(FinishErrorRFCCode(&astersql_errors::New("plain")), None);
}

/// 验证 FormatSQL 折叠空白，以及真实 RU V2 指标和 TiKV RU 的汇总。
#[test]
fn adapter_formats_multiline_sql_and_calculates_all_ru_components() {
    assert_eq!(
        FormatSQL("select\t1\r\nfrom t").to_string(),
        "select 1  from t"
    );
    let metrics = RUV2Metrics::default();
    metrics.AddResultChunkCells(10);
    let weights = RUV2Weights {
        RUScale: 1.0,
        ResultChunkCells: 0.5,
        ..RUV2Weights::default()
    };
    let details = RUDetails::default();
    details.AddTiKVRUV2(6.0);
    assert_eq!(
        calculateStatementTotalRUV2(Some(&metrics), weights, Some(&details)),
        11.0
    );
    assert_eq!(
        calculateStatementTotalRUV2(None, weights, Some(&details)),
        6.0
    );
    assert_eq!(calculateStatementTotalRUV2(None, weights, None), 0.0);
    metrics.SetBypass(true);
    assert_eq!(
        calculateStatementTotalRUV2(Some(&metrics), weights, Some(&details)),
        0.0
    );
}

#[test]
fn written_shared_lock_keys_are_promoted_deduplicated_and_errors_propagate() {
    let key = |value: &str| value.as_bytes().to_vec();

    let empty = SharedLockTxnForTest {
        written: HashSet::new(),
        errors: HashMap::new(),
    };
    let (exclusive, shared) =
        moveWrittenSharedLockKeysToExclusive(&empty, vec![key("exclusive")], Vec::new())
            .expect("empty shared keys must be unchanged");
    assert_eq!(exclusive, vec![key("exclusive")]);
    assert!(shared.is_empty());

    let transaction = SharedLockTxnForTest {
        written: HashSet::from([key("written")]),
        errors: HashMap::new(),
    };
    let (exclusive, shared) = moveWrittenSharedLockKeysToExclusive(
        &transaction,
        vec![key("exclusive")],
        vec![key("exclusive"), key("written"), key("shared")],
    )
    .expect("written keys must be promoted");
    assert_eq!(exclusive, vec![key("exclusive"), key("written")]);
    assert_eq!(shared, vec![key("shared")]);

    let injected = astersql_errors::New("injected get local error");
    let failing = SharedLockTxnForTest {
        written: HashSet::new(),
        errors: HashMap::from([(key("bad"), injected.clone())]),
    };
    let error = moveWrittenSharedLockKeysToExclusive(&failing, Vec::new(), vec![key("bad")])
        .expect_err("transaction lookup errors must be returned");
    assert_eq!(error.to_string(), injected.to_string());
}
