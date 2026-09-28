// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// 信息类内置函数标量内核回归测试。
//
// 对应 Go `builtin_info_test.go` 中的会话信息、资源组 hint、BENCHMARK 循环计数、
// 表达式元数据与 FORMAT_BYTES / FORMAT_NANO_TIME 边界用例。

use std::{cell::Cell, collections::HashMap};

use crate::builtin_info_kernel::*;

/// 构造带登录/认证身份、角色与资源组的典型会话快照。
fn populated_session() -> SessionInfo {
    SessionInfo {
        current_db: "test".into(),
        last_found_rows: 2,
        user: Some(UserIdentity {
            username: "login".into(),
            hostname: "client".into(),
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
        previous_last_insert_id: 7,
        resource_group_name: "default".into(),
        hinted_resource_group: Some("rg1".into()),
        ..SessionInfo::default()
    }
}

/// 会话信息取值与缺失状态报错对齐 Go。
#[test]
fn session_information_matches_go_values_and_missing_state_errors() {
    let session = populated_session();
    assert_eq!(database(&session).as_deref(), Some("test"));
    assert_eq!(found_rows(&session), 2);
    assert_eq!(user(&session).unwrap(), "login@client");
    assert_eq!(current_user(&session).unwrap(), "root@localhost");
    assert_eq!(
        current_role(&session).unwrap(),
        "`r_1`@`%`,`r_2`@`localhost`"
    );
    assert_eq!(current_resource_group(&session), "rg1");
    assert_eq!(connection_id(&session), 1);
    assert_eq!(row_count(&session), 10);
    assert_eq!(last_insert_id(&session), 7);

    let mut empty = SessionInfo::default();
    assert_eq!(database(&empty), None);
    assert_eq!(current_role(&empty).unwrap(), "NONE");
    assert!(user(&empty).is_err());
    assert!(current_user(&empty).is_err());
    empty.active_roles = None;
    assert!(current_role(&empty).is_err());
}

/// 资源组 hint 优先级与带参 LAST_INSERT_ID 的会话副作用。
#[test]
fn resource_group_hint_and_last_insert_id_preserve_session_side_effects() {
    let mut session = populated_session();
    session.hinted_resource_group = Some(String::new());
    assert_eq!(current_resource_group(&session), "");
    session.hinted_resource_group = None;
    assert_eq!(current_resource_group(&session), "default");

    assert_eq!(last_insert_id_with_id(&mut session, Some(-1)), Some(-1));
    assert_eq!(session.last_insert_id, u64::MAX);
    assert_eq!(last_insert_id_with_id(&mut session, None), None);
    assert_eq!(session.last_insert_id, u64::MAX);
    assert_eq!(tidb_is_ddl_owner(true), 1);
    assert_eq!(tidb_is_ddl_owner(false), 0);
}

/// BENCHMARK：负数 NULL、零次不求值、正数精确循环。
#[test]
fn benchmark_covers_negative_zero_and_repeated_evaluation() {
    let calls = Cell::new(0);
    assert_eq!(
        benchmark(-3, || panic!("negative count must not evaluate")).unwrap(),
        None
    );
    assert_eq!(
        benchmark(0, || panic!("zero count must not evaluate")).unwrap(),
        Some(0)
    );
    assert_eq!(
        benchmark(3, || {
            calls.set(calls.get() + 1);
            Ok(())
        })
        .unwrap(),
        Some(0)
    );
    assert_eq!(calls.get(), 3);
}

/// CHARSET/COLLATION/COERCIBILITY 与版本串非空。
#[test]
fn expression_metadata_and_version_helpers_return_go_fields() {
    let metadata = ExpressionMetadata {
        charset: "utf8mb4".into(),
        collation: "utf8mb4_general_ci".into(),
        coercibility: 4,
    };
    assert_eq!(charset(&metadata), "utf8mb4");
    assert_eq!(collation(&metadata), "utf8mb4_general_ci");
    assert_eq!(coercibility(&metadata), 4);
    assert!(!version().is_empty());
    assert!(!tidb_version().is_empty());
}

/// FORMAT_BYTES / FORMAT_NANO_TIME：NULL、单位边界与负值。
#[test]
fn format_bytes_and_nano_time_cover_null_units_boundaries_and_negative_values() {
    assert_eq!(format_bytes(None), None);
    assert_eq!(format_bytes(Some(0.0)).as_deref(), Some("0 bytes"));
    assert_eq!(format_bytes(Some(2048.0)).as_deref(), Some("2.00 KiB"));
    assert_eq!(
        format_bytes(Some(75_295_729.0)).as_deref(),
        Some("71.81 MiB")
    );
    assert_eq!(
        format_bytes(Some(-18_446_644_073_709_551_615.0)).as_deref(),
        Some("-16.00 EiB")
    );

    assert_eq!(format_nano_time(None), None);
    assert_eq!(format_nano_time(Some(0.0)).as_deref(), Some("0 ns"));
    assert_eq!(format_nano_time(Some(2_000.0)).as_deref(), Some("2.00 us"));
    assert_eq!(
        format_nano_time(Some(9_999_999_991.0)).as_deref(),
        Some("10.00 s")
    );
    assert_eq!(
        format_nano_time(Some(-9_999_999_991.0)).as_deref(),
        Some("-10.00 s")
    );
}

/// Go 按字节截断 SQL 文本；若截在 UTF-8 字符中间，JSON 编码会写入替换字符。
#[test]
fn sql_digest_truncation_preserves_go_byte_slice_semantics() {
    struct Retriever;

    impl SqlDigestRetriever for Retriever {
        fn retrieve_global(
            &self,
            digests: &[String],
        ) -> Result<HashMap<String, String>, crate::builtin_ilike_kernel::ExpressionError> {
            Ok(digests
                .iter()
                .map(|digest| (digest.clone(), "你好".to_owned()))
                .collect())
        }
    }

    assert_eq!(
        decode_sql_digests(Some("[\"digest\"]"), Some(1), true, &Retriever).unwrap(),
        Some("[\"�...\"]".to_owned())
    );
}
