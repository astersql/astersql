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

use crate::adapter::{
    AdapterResult, FinishErrorRFCCode, FormatSQL, Key, moveWrittenSharedLockKeysToExclusive,
    pessimisticTxn,
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

/// Verify SQL formatting remains independent of RU accounting.
#[test]
fn adapter_formats_multiline_sql() {
    assert_eq!(
        FormatSQL("select\t1\r\nfrom t").to_string(),
        "select 1  from t"
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
