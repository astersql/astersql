// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 信息类内置函数的向量化（vectorized）求值单元测试。
//
// 对应 Go `builtin_info_vec_test.go`：验证 DATABASE / CONNECTION_ID / FOUND_ROWS 等
// 会话级标量在列上按行数广播，以及 BENCHMARK、LAST_INSERT_ID 与向量化白名单。
// 向量化指一次对整列（Chunk）求值，而非逐行标量调用。

use std::cell::Cell;

use crate::builtin_ilike_kernel::ExpressionError;
use crate::builtin_info_kernel::{InfoValue, KeyCodec, RoleIdentity, SessionInfo, UserIdentity};
use crate::builtin_info_vec_kernel::*;

/// 构造带典型会话字段的测试用 SessionInfo（当前库、用户、角色、连接 ID 等）。
fn session() -> SessionInfo {
    SessionInfo {
        current_db: "test".into(),
        last_found_rows: 2,
        user: Some(UserIdentity {
            username: "login".into(),
            hostname: "client".into(),
            auth_username: "root".into(),
            auth_hostname: "localhost".into(),
        }),
        active_roles: Some(vec![RoleIdentity {
            username: "r_1".into(),
            hostname: "%".into(),
        }]),
        connection_id: 8,
        previous_affected_rows: 10,
        previous_last_insert_id: 12,
        resource_group_name: "rg1".into(),
        ..SessionInfo::default()
    }
}

#[test]
/// 会话内置函数按行数重复同一值；行数为 0 时返回空列；缺省会话触发错误或 NULL。
fn vectorized_session_builtins_repeat_values_and_preserve_zero_rows() {
    let session = session();
    assert_eq!(vec_database(&session, 3), vec![Some("test".into()); 3]);
    assert_eq!(vec_connection_id(&session, 3), vec![8; 3]);
    assert_eq!(vec_found_rows(&session, 3), vec![2; 3]);
    assert_eq!(vec_row_count(&session, 3), vec![10; 3]);
    assert_eq!(
        vec_current_user(&session, 2).unwrap(),
        vec!["root@localhost"; 2]
    );
    assert_eq!(vec_user(&session, 2).unwrap(), vec!["login@client"; 2]);
    assert_eq!(vec_current_role(&session, 2).unwrap(), vec!["`r_1`@`%`"; 2]);
    assert_eq!(vec_current_resource_group(&session, 2), vec!["rg1"; 2]);
    assert!(vec_version(0).is_empty());
    assert!(vec_tidb_version(0).is_empty());

    let empty = SessionInfo::default();
    assert_eq!(vec_database(&empty, 2), vec![None, None]);
    assert!(vec_current_user(&empty, 1).is_err());
}

#[test]
/// BENCHMARK：对闭包执行固定次数并返回 0 列；次数 ≤0 应报错。
fn vectorized_benchmark_counts_calls_and_rejects_non_positive_counts() {
    let calls = Cell::new(0);
    assert_eq!(
        vec_benchmark(4, 3, || {
            calls.set(calls.get() + 1);
            Ok(())
        })
        .unwrap(),
        vec![0; 4]
    );
    assert_eq!(calls.get(), 3);
    let error = vec_benchmark(0, 2, || {
        calls.set(calls.get() + 1);
        Ok(())
    })
    .unwrap();
    assert!(error.is_empty());
    assert_eq!(calls.get(), 5);
    assert!(matches!(
        vec_benchmark(2, 3, || Err(ExpressionError::InvalidArgument(
            "child".into()
        ))),
        Err(ExpressionError::InvalidArgument(message)) if message == "child"
    ));
    assert!(vec_benchmark(1, 0, || Ok(())).is_err());
    assert!(vec_benchmark(1, -1, || Ok(())).is_err());
}

#[test]
/// LAST_INSERT_ID：无参读会话；有参时取最后一个非 NULL，并写回会话（含负数为按位解释）。
fn vectorized_last_insert_id_uses_last_non_null_argument() {
    let mut session = session();
    assert_eq!(vec_last_insert_id(&session, 2), vec![12; 2]);
    let values = vec![Some(1), None, Some(-1), None];
    assert_eq!(
        vec_last_insert_id_with_id(&mut session, values.clone()),
        values
    );
    assert_eq!(session.last_insert_id, u64::MAX);

    let only_nulls = vec![None, None];
    assert_eq!(
        vec_last_insert_id_with_id(&mut session, only_nulls.clone()),
        only_nulls
    );
    assert_eq!(session.last_insert_id, u64::MAX);
}

#[test]
/// 与 Go 一致：部分信息函数可向量化，MVCC/编码键类不可向量化。
/// MVCC（多版本并发控制）用于按时间戳读历史版本。
fn vectorization_matrix_matches_go_exceptions() {
    for kind in [
        InfoBuiltinKind::Database,
        InfoBuiltinKind::ConnectionId,
        InfoBuiltinKind::TiDBVersion,
        InfoBuiltinKind::RowCount,
        InfoBuiltinKind::CurrentUser,
        InfoBuiltinKind::CurrentResourceGroup,
        InfoBuiltinKind::CurrentRole,
        InfoBuiltinKind::User,
        InfoBuiltinKind::TiDBIsDdlOwner,
        InfoBuiltinKind::FoundRows,
        InfoBuiltinKind::Benchmark,
        InfoBuiltinKind::LastInsertId,
        InfoBuiltinKind::LastInsertIdWithId,
        InfoBuiltinKind::Version,
        InfoBuiltinKind::TiDBDecodeKey,
    ] {
        assert!(kind.vectorized());
    }
    for kind in [
        InfoBuiltinKind::TiDBMvccInfo,
        InfoBuiltinKind::TiDBEncodeRecordKey,
        InfoBuiltinKind::TiDBEncodeIndexKey,
    ] {
        assert!(!kind.vectorized());
    }
}

struct PrefixCodec;

impl KeyCodec for PrefixCodec {
    fn encode_record_key(
        &self,
        _arguments: &[InfoValue],
    ) -> Result<Option<Vec<u8>>, ExpressionError> {
        unreachable!("decode-only test codec")
    }

    fn encode_index_key(
        &self,
        _arguments: &[InfoValue],
    ) -> Result<Option<Vec<u8>>, ExpressionError> {
        unreachable!("decode-only test codec")
    }

    fn decode_key(&self, source: &str) -> Result<String, ExpressionError> {
        if source == "bad" {
            Err(ExpressionError::InvalidArgument("bad key".into()))
        } else {
            Ok(format!("decoded:{source}"))
        }
    }
}

#[test]
/// 解码逐行保留 NULL、在无解码器时回退原文，并在首个错误处停止。
fn vectorized_decode_key_preserves_null_fallback_and_errors() {
    let values = vec![Some("a".into()), None, Some("b".into())];
    assert_eq!(vec_decode_key(&values, None).unwrap(), values);
    assert_eq!(
        vec_decode_key(&values, Some(&PrefixCodec)).unwrap(),
        vec![Some("decoded:a".into()), None, Some("decoded:b".into())]
    );
    assert!(matches!(
        vec_decode_key(&[Some("a".into()), Some("bad".into())], Some(&PrefixCodec)),
        Err(ExpressionError::InvalidArgument(message)) if message == "bad key"
    ));
}

#[test]
/// DDL owner 结果与 Go 一样广播为非 NULL 的 0/1 整数列。
fn vectorized_ddl_owner_broadcasts_boolean_as_integer() {
    assert_eq!(vec_tidb_is_ddl_owner(true, 3), vec![1, 1, 1]);
    assert_eq!(vec_tidb_is_ddl_owner(false, 2), vec![0, 0]);
    assert!(vec_tidb_is_ddl_owner(true, 0).is_empty());
}
