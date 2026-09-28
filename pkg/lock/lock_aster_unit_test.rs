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

// Aster 迁移补充单测：`checkLockTpMeetPrivilege` 与 Go 锁类型/权限矩阵对齐。
//
// 覆盖写锁/本地写锁放行、读锁仅 SELECT、以及与会话无关或未知类型在此函数中一律拒绝。

use astersql_lock_context::ast;
use astersql_parser_mysql::privs as mysql;

use crate::checkLockTpMeetPrivilege;

/// Write / WriteLocal 应对常见表级权限全部返回 true。
#[test]
fn write_and_write_local_locks_allow_all_table_privileges() {
    for lock_type in [ast::TableLockWrite, ast::TableLockWriteLocal] {
        for privilege in [
            mysql::SelectPriv,
            mysql::InsertPriv,
            mysql::UpdatePriv,
            mysql::DeletePriv,
            mysql::DropPriv,
            mysql::AlterPriv,
        ] {
            assert!(
                checkLockTpMeetPrivilege(lock_type, privilege),
                "{lock_type} must permit {}",
                privilege.String()
            );
        }
    }
}

/// 读锁只允许 SELECT，拒绝 INSERT/UPDATE/DELETE/DROP/ALTER。
#[test]
fn read_lock_allows_select_and_rejects_write_privileges() {
    assert!(checkLockTpMeetPrivilege(
        ast::TableLockRead,
        mysql::SelectPriv
    ));

    for privilege in [
        mysql::InsertPriv,
        mysql::UpdatePriv,
        mysql::DeletePriv,
        mysql::DropPriv,
        mysql::AlterPriv,
    ] {
        assert!(
            !checkLockTpMeetPrivilege(ast::TableLockRead, privilege),
            "read lock must reject {}",
            privilege.String()
        );
    }
}

/// None / ReadLocal / ReadOnly / 未知类型不由本函数放行（由上层另行处理）。
#[test]
fn session_unrelated_and_unknown_lock_types_are_not_accepted_here() {
    for lock_type in [
        ast::TableLockNone,
        ast::TableLockReadLocal,
        ast::TableLockReadOnly,
        ast::TableLockType(u8::MAX),
    ] {
        assert!(!checkLockTpMeetPrivilege(lock_type, mysql::SelectPriv));
        assert!(!checkLockTpMeetPrivilege(lock_type, mysql::UpdatePriv));
    }
}
