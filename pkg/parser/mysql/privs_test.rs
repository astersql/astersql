// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 权限元数据单元测试：校验权限位字符串、系统表列名与 SET 枚举的往返映射。
//
// 对应 Go 的 `privs_test.go`。只检查进程内权限位与名称映射，
// 不连接 `mysql.user`、不执行 GRANT/REVOKE，也不产生外部 IO。

// 本文件对照 pkg/parser/mysql/privs_test.go 迁移，保留 Go 测试结构与行为。
// 这里只检查内存权限位、权限名和系统表列名映射，不连接 mysql.user、不执行授权 SQL，也不会产生外部 IO。

// test_priv_string 对应 Go 的 TestPrivString，遍历从 bit0 到 AllPriv 的所有权限位并确认 String 非空。
use crate::privs::*;

#[test]
fn test_priv_string() {
    let mut i = 0_u64;
    // 按位左移枚举每个 PrivilegeType，直到超过 AllPriv 哨兵位。
    loop {
        let p = PrivilegeType(1_u64 << i);
        if p.0 > AllPriv.0 {
            break;
        }
        // Go 的 NotEqualf 会带上 "%d-th" 提示；把序号放入失败消息。
        assert_ne!("", p.String(), "{i}-th");
        i += 1;
    }
}

// assert_column_round_trip 保留 Go 中三个 for 循环的共同语义：权限列名非空且可反向解析回原权限。
fn assert_column_round_trip(privileges: Privileges) {
    for p in privileges {
        let column = p.ColumnString();
        assert!(!column.is_empty(), "{:?}", p);
        // NewPrivFromColumn 从 mysql.user 风格列名反查权限位。
        let np = NewPrivFromColumn(column).expect("privilege column should round trip");
        assert_eq!(p, np);
    }
}

// test_priv_column 对应 Go 的 TestPrivColumn，覆盖全局、仅全局静态和 DB 级权限列名映射。
#[test]
fn test_priv_column() {
    assert_column_round_trip(AllGlobalPrivs());
    assert_column_round_trip(StaticGlobalOnlyPrivs());
    assert_column_round_trip(AllDBPrivs());
}

// assert_set_round_trip 保留 Go 中 table/column privilege set enum 的共同检查。
fn assert_set_round_trip(privileges: Privileges) {
    for p in privileges {
        let set_value = p.SetString();
        assert!(!set_value.is_empty(), "{:?}", p);
        // SET 枚举文本用于 information_schema 等表的权限列类型定义。
        let np = NewPrivFromSetEnum(set_value).expect("privilege set enum should round trip");
        assert_eq!(p, np);
    }
}

// test_priv_set_string 对应 Go 的 TestPrivSetString，确认表级和列级权限可以映射到 SET 枚举文本并反查。
#[test]
fn test_priv_set_string() {
    assert_set_round_trip(AllTablePrivs());
    assert_set_round_trip(AllColumnPrivs());
}

// test_privs_has 对应 Go 的 TestPrivsHas，只验证简单 helper，不处理 ALL 与 dynamic privilege 的完整授权语义。
#[test]
fn test_privs_has() {
    // it is a simple helper, does not handle all&dynamic privs
    // 单权限列表：Has 只做线性成员判断。
    let mut privs = vec![AllPriv];
    assert!(privs.Has(AllPriv));
    assert!(!privs.Has(InsertPriv));

    // multiple privs
    // 多权限列表：确认正负命中。
    privs = vec![InsertPriv, SelectPriv];
    assert!(privs.Has(SelectPriv));
    assert!(privs.Has(InsertPriv));
    assert!(!privs.Has(DropPriv));
}

// test_priv_all_consistency 对应 Go 的 TestPrivAllConsistency。
// 它确认 AllPriv 之前每个用户表权限都有列名，并保持 Priv2UserCol 与 Priv2Str 的长度关系。
#[test]
fn test_priv_all_consistency() {
    // AllPriv in mysql.user columns.
    // 从 CreatePriv 起按位枚举，直到 AllPriv；中间每位都应出现在 Priv2UserCol。
    let mut priv_value = CreatePriv.0;
    while priv_value != AllPriv.0 {
        let priv_type = PrivilegeType(priv_value);
        assert!(
            Priv2UserCol().contains_key(&priv_type),
            "priv fail {priv_value}"
        );
        // Go 用 priv = priv << 1 枚举 bit 位；这里保留同样的左移推进。
        priv_value <<= 1;
    }

    // 全局权限列表不含 AllPriv，因此比 Priv2UserCol 少一项。
    assert_eq!(AllGlobalPrivs().len() + 1, Priv2UserCol().len());

    // USAGE privilege doesn't have a column in Priv2UserCol
    // ALL privilege doesn't have a column in Priv2UserCol
    // so it's +2
    // Priv2Str 额外包含 USAGE 与 ALL 两项显示名。
    assert_eq!(Priv2UserCol().len() + 2, Priv2Str().len());
}
