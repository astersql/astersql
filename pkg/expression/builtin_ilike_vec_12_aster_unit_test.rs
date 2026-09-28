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

// ILIKE / 信息类内置函数的综合单元测试。
//
// 覆盖标量与向量 ILIKE、会话信息函数、digest/plan 解码、key 编解码与权限错误、
// MVCC 查询（含临时索引键）、序列权限以及向量化能力矩阵，对齐 Go 语义契约。

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Mutex;

use crate::builtin_ilike_kernel::{ExpressionError, IlikeSig};
use crate::builtin_ilike_vec_kernel::{EscapeParam, StringParam};
use crate::builtin_info_kernel::{
    InfoValue, KeyCodec, MvccProvider, MvccResponse, PlanDecoder, RoleIdentity, SequencePrivilege,
    SequenceService, SessionInfo, SqlDigestRetriever, UserIdentity, benchmark, connection_id,
    current_resource_group, current_role, current_user, database, decode_binary_plan,
    decode_binary_plan_with, decode_key, decode_plan, decode_plan_with, decode_sql_digests,
    encode_index_key, encode_record_key, encode_sql_digest, format_bytes, format_nano_time,
    found_rows, get_schema_and_sequence, last_insert_id, last_insert_id_with_id, last_val,
    next_val, row_count, set_val, tidb_mvcc_info, user,
};
use crate::builtin_info_vec_kernel::{
    InfoBuiltinKind, vec_database, vec_decode_key, vec_found_rows, vec_last_insert_id,
    vec_last_insert_id_with_id, vec_row_count,
};

/// 标量 ILIKE：ASCII/escape/NULL 语义及克隆清空缓存。
#[test]
fn ilike_scalar_matches_go_ascii_escape_and_null_semantics() {
    let cases = [
        ("a", "", 0, false),
        ("aA", "Aa", 0, true),
        ("áAb", "%ab%", 0, true),
        ("ß", "_", 0, true),
        ("ß", "__", 0, false),
        ("abc", "ABC", 'a' as i64, true),
        ("abc", "ABC", 'A' as i64, false),
        ("a", "AA", 'A' as i64, true),
        ("啊aAa啊啊啊aA", "啊AAA啊啊啊AA", 'a' as i64, true),
    ];
    let sig = IlikeSig::new("utf8mb4_general_ci", true, true);
    for (value, pattern, escape, expected) in cases {
        assert_eq!(
            sig.eval_int(Some(value), Some(pattern), Some(escape))
                .unwrap(),
            Some(i64::from(expected)),
            "value={value:?}, pattern={pattern:?}, escape={escape:?}"
        );
    }
    // 任一实参为 NULL → SQL NULL。
    assert_eq!(sig.eval_int(None, Some("%"), Some(0)).unwrap(), None);
    assert_eq!(sig.eval_int(Some("a"), None, Some(0)).unwrap(), None);
    assert_eq!(sig.eval_int(Some("a"), Some("a"), None).unwrap(), None);
    assert!(sig.cache_initialized());

    let cloned = sig.clone();
    assert!(
        !cloned.cache_initialized(),
        "Clone must not copy the runtime cache"
    );
}

/// 向量 ILIKE：列×常量、常量×列、NULL escape 与非常量 escape 报错。
#[test]
fn ilike_vector_matches_scalar_for_constant_and_column_paths() {
    let sig = IlikeSig::new("utf8mb4_general_ci", true, true);
    let expr = StringParam::Column(vec![
        Some("a".into()),
        Some("A".into()),
        Some("aa".into()),
        None,
    ]);
    let pattern = StringParam::Constant(Some("A".into()));
    assert_eq!(
        sig.vec_eval_int(&expr, &pattern, EscapeParam::Constant(Some('\\' as i64)), 4)
            .unwrap(),
        vec![Some(1), Some(1), Some(0), None]
    );
    assert!(sig.cache_initialized());

    let expr = StringParam::Constant(Some("Aa".into()));
    let pattern = StringParam::Column(vec![
        Some("A".into()),
        Some("AA".into()),
        Some("B".into()),
        Some("%a%".into()),
    ]);
    assert_eq!(
        sig.vec_eval_int(&expr, &pattern, EscapeParam::Constant(Some('\\' as i64)), 4)
            .unwrap(),
        vec![Some(0), Some(1), Some(0), Some(1)]
    );

    assert_eq!(
        sig.vec_eval_int(&expr, &pattern, EscapeParam::Constant(None), 4)
            .unwrap(),
        vec![None; 4]
    );
    assert!(matches!(
        sig.vec_eval_int(&expr, &pattern, EscapeParam::Column, 4),
        Err(ExpressionError::EscapeMustBeConstant)
    ));
}

/// 信息类标量：DATABASE/USER/ROLE/资源组/LAST_INSERT_ID 与格式化辅助。
#[test]
fn information_scalars_match_go_session_and_formatting_behavior() {
    let mut session = SessionInfo {
        current_db: String::new(),
        last_found_rows: 2,
        user: Some(UserIdentity {
            username: "root".into(),
            hostname: "localhost".into(),
            auth_username: "root".into(),
            auth_hostname: "localhost".into(),
        }),
        active_roles: Some(vec![
            RoleIdentity {
                username: "r_2".into(),
                hostname: "localhost".into(),
            },
            RoleIdentity {
                username: "r_1".into(),
                hostname: "%".into(),
            },
        ]),
        connection_id: 1,
        previous_affected_rows: 10,
        previous_last_insert_id: u64::MAX,
        resource_group_name: "rg1".into(),
        hinted_resource_group: Some("hinted".into()),
        ..SessionInfo::default()
    };

    assert_eq!(database(&session), None);
    session.current_db = "test".into();
    assert_eq!(database(&session), Some("test".into()));
    assert_eq!(found_rows(&session), 2);
    assert_eq!(user(&session).unwrap(), "root@localhost");
    assert_eq!(current_user(&session).unwrap(), "root@localhost");
    assert_eq!(
        current_role(&session).unwrap(),
        "`r_1`@`%`,`r_2`@`localhost`"
    );
    assert_eq!(current_resource_group(&session), "hinted");
    assert_eq!(connection_id(&session), 1);
    assert_eq!(row_count(&session), 10);
    assert_eq!(last_insert_id(&session), u64::MAX as i64);
    assert_eq!(last_insert_id_with_id(&mut session, Some(-1)), Some(-1));
    assert_eq!(session.last_insert_id, u64::MAX);
    assert_eq!(last_insert_id_with_id(&mut session, None), None);

    assert_eq!(format_bytes(Some(0.0)), Some("0 bytes".into()));
    assert_eq!(format_bytes(Some(2048.0)), Some("2.00 KiB".into()));
    assert_eq!(format_bytes(Some(75_295_729.0)), Some("71.81 MiB".into()));
    assert_eq!(format_nano_time(Some(2_000.0)), Some("2.00 us".into()));
    assert_eq!(
        format_nano_time(Some(9_999_999_991.0)),
        Some("10.00 s".into())
    );
    assert_eq!(format_nano_time(None), None);
    assert_eq!(
        get_schema_and_sequence("db.seq.extra"),
        ("db".into(), "seq".into())
    );
}

/// 向量信息函数重复标量值；带参 LAST_INSERT_ID 取列中最后一个非 NULL。
#[test]
fn information_vectors_repeat_values_and_last_insert_id_uses_last_non_null() {
    let mut session = SessionInfo {
        current_db: "test".into(),
        last_found_rows: 7,
        previous_affected_rows: -1,
        previous_last_insert_id: 9,
        ..SessionInfo::default()
    };
    assert_eq!(vec_database(&session, 3), vec![Some("test".into()); 3]);
    assert_eq!(vec_found_rows(&session, 3), vec![7; 3]);
    assert_eq!(vec_row_count(&session, 2), vec![-1; 2]);
    assert_eq!(vec_last_insert_id(&session, 2), vec![9; 2]);

    let values = vec![Some(11), None, Some(22), None];
    assert_eq!(
        vec_last_insert_id_with_id(&mut session, values.clone()),
        values
    );
    assert_eq!(session.last_insert_id, 22);

    let calls = Cell::new(0);
    assert_eq!(
        benchmark(3, || {
            calls.set(calls.get() + 1);
            Ok(())
        })
        .unwrap(),
        Some(0)
    );
    assert_eq!(calls.get(), 3);
    assert_eq!(benchmark(-1, || Ok(())).unwrap(), None);
}

/// 内存版 SQL digest 检索器，按 digests 过滤 HashMap。
struct MapRetriever(HashMap<String, String>);

impl SqlDigestRetriever for MapRetriever {
    fn retrieve_global(
        &self,
        digests: &[String],
    ) -> Result<HashMap<String, String>, ExpressionError> {
        Ok(digests
            .iter()
            .filter_map(|digest| self.0.get(digest).map(|sql| (digest.clone(), sql.clone())))
            .collect())
    }
}

/// digest 编解码截断、JSON 失败、PROCESS 权限与 plan 解码错误契约。
#[test]
fn digest_and_plan_helpers_preserve_go_error_contracts() {
    let digest = encode_sql_digest(Some("select * from t where id = 1")).unwrap();
    assert_eq!(digest.as_ref().map(String::len), Some(64));
    let retriever = MapRetriever(HashMap::from([(
        digest.clone().unwrap(),
        "select * from t".into(),
    )]));
    let input = serde_json::to_string(&vec![digest, None, Some("missing".to_owned())]).unwrap();
    assert_eq!(
        decode_sql_digests(Some(&input), Some(6), true, &retriever).unwrap(),
        Some("[\"select...\",null,null]".into())
    );
    assert!(
        decode_sql_digests(Some("not json"), None, true, &retriever)
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        decode_sql_digests(Some("[]"), None, false, &retriever),
        Err(ExpressionError::AccessDenied(_))
    ));

    // 文本 plan 解码失败返回原文；二进制失败写 warning 并返回空串。
    assert_eq!(decode_plan(Some("not-a-plan")), Some("not-a-plan".into()));
    let mut warnings = Vec::new();
    assert_eq!(
        decode_binary_plan(Some("not-a-plan"), &mut warnings),
        Some(String::new())
    );
    assert_eq!(warnings.len(), 1);
}

/// 成功路径的 key 编解码桩：record 返回固定字节，index 返回 None。
struct TestKeyCodec;

impl KeyCodec for TestKeyCodec {
    fn encode_record_key(
        &self,
        arguments: &[InfoValue],
    ) -> Result<Option<Vec<u8>>, ExpressionError> {
        assert_eq!(arguments, &[InfoValue::Int(7)]);
        Ok(Some(vec![0x74, 0x72]))
    }

    fn encode_index_key(
        &self,
        _arguments: &[InfoValue],
    ) -> Result<Option<Vec<u8>>, ExpressionError> {
        Ok(None)
    }

    fn decode_key(&self, source: &str) -> Result<String, ExpressionError> {
        Ok(format!("decoded:{source}"))
    }
}

/// 始终拒绝访问的 key 编解码桩，用于验证表级权限错误映射。
struct DeniedKeyCodec;

impl KeyCodec for DeniedKeyCodec {
    fn encode_record_key(
        &self,
        _arguments: &[InfoValue],
    ) -> Result<Option<Vec<u8>>, ExpressionError> {
        Err(ExpressionError::AccessDenied("SELECT".into()))
    }
    fn encode_index_key(
        &self,
        _arguments: &[InfoValue],
    ) -> Result<Option<Vec<u8>>, ExpressionError> {
        Err(ExpressionError::AccessDenied("SELECT".into()))
    }
    fn decode_key(&self, source: &str) -> Result<String, ExpressionError> {
        Ok(source.into())
    }
}

/// key 回调：hex 编码、NULL、缺 codec、AccessDenied→TableAccessDenied。
#[test]
fn key_callbacks_keep_go_null_hex_decode_and_access_error_contracts() {
    let session = SessionInfo {
        user: Some(UserIdentity {
            auth_username: "root".into(),
            auth_hostname: "localhost".into(),
            ..UserIdentity::default()
        }),
        ..SessionInfo::default()
    };
    assert_eq!(
        encode_record_key(
            Some(&TestKeyCodec),
            &session,
            Some("t"),
            &[InfoValue::Int(7)]
        )
        .unwrap(),
        Some("7472".into())
    );
    assert_eq!(
        encode_index_key(Some(&TestKeyCodec), &session, Some("t"), &[]).unwrap(),
        None
    );
    assert_eq!(
        decode_key(Some("7480"), Some(&TestKeyCodec)).unwrap(),
        Some("decoded:7480".into())
    );
    assert_eq!(decode_key(Some("7480"), None).unwrap(), Some("7480".into()));
    assert!(matches!(
        encode_record_key(Some(&DeniedKeyCodec), &session, Some("t"), &[]),
        Err(ExpressionError::TableAccessDenied { table, .. }) if table == "t"
    ));
    assert!(matches!(
        encode_record_key(None, &session, Some("t"), &[]),
        Err(ExpressionError::External(message)) if message.contains("not initialized")
    ));
}

/// 记录查询过的 key；普通索引键会再查一次临时索引键（末尾 0xff）。
struct TestMvcc {
    seen: Mutex<Vec<Vec<u8>>>,
}

impl MvccProvider for TestMvcc {
    fn get_mvcc_by_encoded_key(&self, key: &[u8]) -> Result<MvccResponse, ExpressionError> {
        self.seen.lock().unwrap().push(key.to_vec());
        Ok(MvccResponse {
            value: serde_json::json!({"writes": key.len()}),
            has_entries: key.len() > 1,
        })
    }
    fn is_index_key(&self, _key: &[u8]) -> bool {
        true
    }
    fn is_temp_index_key(&self, key: &[u8]) -> bool {
        key.last() == Some(&0xff)
    }
    fn to_temp_index_key(&self, key: &mut Vec<u8>) {
        key.push(0xff);
    }
}

/// TIDB_MVCC_INFO：校验 SUPER 权限，并先后查询普通键与临时索引键。
#[test]
fn mvcc_info_checks_super_and_queries_normal_then_temp_index_key() {
    let provider = TestMvcc {
        seen: Mutex::new(Vec::new()),
    };
    assert!(matches!(
        tidb_mvcc_info(Some("aa"), false, &provider),
        Err(ExpressionError::AccessDenied(privilege)) if privilege == "SUPER"
    ));
    let output = tidb_mvcc_info(Some("aa"), true, &provider)
        .unwrap()
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(json.as_array().unwrap().len(), 2);
    assert_eq!(json[0]["key"], "aa");
    assert_eq!(json[1]["key"], "aaff");
    assert_eq!(
        *provider.seen.lock().unwrap(),
        vec![vec![0xaa], vec![0xaa, 0xff]]
    );
}

/// 可控权限的序列服务桩：校验库名/序列名并维护当前值。
#[derive(Default)]
struct TestSequence {
    value: i64,
    allow: bool,
}

impl SequenceService for TestSequence {
    fn sequence_id(&mut self, database: &str, sequence: &str) -> Result<i64, ExpressionError> {
        assert_eq!((database, sequence), ("test", "seq"));
        Ok(42)
    }
    fn next_value(&mut self, _database: &str, _sequence: &str) -> Result<i64, ExpressionError> {
        self.value += 1;
        Ok(self.value)
    }
    fn set_value(
        &mut self,
        _database: &str,
        _sequence: &str,
        value: i64,
    ) -> Result<Option<i64>, ExpressionError> {
        self.value = value;
        Ok(Some(value))
    }
    fn verify(&self, _database: &str, _sequence: &str, _privilege: SequencePrivilege) -> bool {
        self.allow
    }
}

/// 成功解码文本/二进制执行计划的桩。
struct TestPlanDecoder;

impl PlanDecoder for TestPlanDecoder {
    fn decode_plan(&self, source: &str) -> Result<String, ExpressionError> {
        Ok(format!("text:{source}"))
    }
    fn decode_binary_plan(&self, source: &str) -> Result<String, ExpressionError> {
        Ok(format!("binary:{source}"))
    }
}

/// 序列 NEXT/LAST/SET 权限、plan 注入解码与向量化能力矩阵。
#[test]
fn sequence_plan_and_vector_capabilities_preserve_go_dispatch() {
    let mut session = SessionInfo {
        current_db: "test".into(),
        user: Some(UserIdentity {
            auth_username: "u".into(),
            auth_hostname: "h".into(),
            ..UserIdentity::default()
        }),
        ..SessionInfo::default()
    };
    let mut sequence = TestSequence {
        value: 9,
        allow: true,
    };
    assert_eq!(
        next_val(Some("seq"), &mut session, &mut sequence).unwrap(),
        Some(10)
    );
    assert_eq!(
        last_val(Some("test.seq"), &session, &mut sequence).unwrap(),
        Some(10)
    );
    assert_eq!(
        set_val(Some("seq"), Some(20), &session, &mut sequence).unwrap(),
        Some(20)
    );
    sequence.allow = false;
    assert!(matches!(
        next_val(Some("seq"), &mut session, &mut sequence),
        Err(ExpressionError::SequenceAccessDenied {
            operation: "INSERT",
            ..
        })
    ));

    assert_eq!(
        decode_plan_with(Some("p"), &TestPlanDecoder),
        Some("text:p".into())
    );
    let mut warnings = Vec::new();
    assert_eq!(
        decode_binary_plan_with(Some("p"), &TestPlanDecoder, &mut warnings),
        Some("binary:p".into())
    );
    assert!(warnings.is_empty());
    assert!(!InfoBuiltinKind::TiDBMvccInfo.vectorized());
    assert!(!InfoBuiltinKind::TiDBEncodeRecordKey.vectorized());
    assert!(!InfoBuiltinKind::TiDBEncodeIndexKey.vectorized());
    assert!(InfoBuiltinKind::TiDBDecodeKey.vectorized());
    assert_eq!(
        vec_decode_key(&[Some("aa".into()), None], Some(&TestKeyCodec)).unwrap(),
        vec![Some("decoded:aa".into()), None]
    );
    assert_eq!(
        format_bytes(Some(287_952_852_482_075_252_752_429_875.0)),
        Some("2.50e+08 EiB".into())
    );
}
