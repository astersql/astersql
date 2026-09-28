// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// 规划器 rewriter 支撑 API 的 Aster 单元测试。
//
// 校验错误文案与 MySQL 兼容、Datum 转常量保留类型标志，
// 以及常量折叠作用域集合与 Go 函数特征表一致。

use crate::*;

/// 操作数列数与内建函数参数个数错误应保留 MySQL 风格消息。
#[test]
fn planner_rewriter_error_contract_keeps_mysql_messages() {
    assert_eq!(
        ErrOperandColumns.GenWithStackByArgs(3).to_string(),
        "Operand should contain 3 column(s)"
    );
    assert_eq!(
        ErrIncorrectParameterCount
            .GenWithStackByArgs("ifnull")
            .to_string(),
        "Incorrect parameter count in the call to native function 'ifnull'"
    );
}

/// DatumToConstant 应写入指定 MySQL 类型与无符号标志。
#[test]
fn datum_to_constant_preserves_type_and_flags() {
    let constant = crate::core_support::DatumToConstant(
        types::NewIntDatum(7),
        mysql::TypeLonglong,
        mysql::UnsignedFlag as u64,
    );
    let constant = constant.as_constant().expect("constant expression");
    assert_eq!(constant.Value.GetInt64(), 7);
    let ret_type = constant.RetType.as_ref().expect("constant return type");
    assert_eq!(ret_type.GetType(), mysql::TypeLonglong);
    assert!(mysql::HasUnsignedFlag(ret_type.GetFlag()));
}

/// DisableFold / TryFold 函数集合应与 Go 侧特征表对齐。
#[test]
fn folding_scope_sets_match_go_function_traits() {
    // DisableFold：永远不参与常量折叠（如 benchmark）。
    assert_eq!(
        DisableFoldFunctions.keys().copied().collect::<Vec<_>>(),
        ["benchmark"]
    );
    // TryFold：短路/分支类函数仅在参数允许时尝试折叠。
    for name in ["if", "ifnull", "case", "and", "or", "coalesce", "interval"] {
        assert!(TryFoldFunctions.contains_key(name), "missing {name}");
    }
    assert_eq!(TryFoldFunctions.len(), 7);
}
