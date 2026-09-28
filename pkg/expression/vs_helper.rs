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

// 向量搜索（Vector Search）表达式解释辅助。
//
// 识别 L1/L2/余弦/负内积等距离函数，并抽出向量常量与向量列，供优化器判断能否走向量索引。

use crate::*;
use protobuf::ProtobufEnum;

use std::collections::HashSet;
use std::sync::LazyLock;

// vsDistanceFnNamesLower 对应 Go 的包级 map，统一存放小写后的四种向量距离函数名。
// LazyLock 保留 Go 包初始化时构造集合的效果，同时避免手写未初始化全局可变状态。
static vsDistanceFnNamesLower: LazyLock<HashSet<String>> = LazyLock::new(|| {
    [
        ast::VecL1Distance,
        ast::VecL2Distance,
        ast::VecCosineDistance,
        ast::VecNegativeInnerProduct,
    ]
    .into_iter()
    .map(|name| name.to_lowercase())
    .collect()
});

// VSInfo 对应 Go 的同名结构，集中保存调用方解释向量搜索表达式所需的信息。
// 并非所有向量搜索函数都能使用索引，调用方仍须检查 DistanceFnName。
/// 向量搜索表达式的解释结果：距离函数名、Protobuf 算子码、查询向量与被查列。
pub struct VSInfo<'a> {
    pub DistanceFnName: ast::CIStr,
    pub FnPbCode: tipb::ScalarFuncSig,
    pub Vec: types::VectorFloat32,
    // Go 用 *Column 借用原表达式树中的列；用共享引用显式表达该生命周期。
    pub Column: &'a Column,
}

// InterpretVectorSearchExpr 对应 Go 的解释入口。
// 成功时返回 VSInfo；类型、函数名、参数数量或参数类型不符合约束时均返回 None。
/// 尝试将表达式解释为向量距离调用；形态不符时返回 `None`。
pub fn InterpretVectorSearchExpr(expr: &dyn Expression) -> Option<VSInfo<'_>> {
    // Go 使用类型断言 *ScalarFunction；这里保留通过 Any 下转型的预期接口形状。
    let x = expr.as_any().downcast_ref::<ScalarFunction>()?;

    if !vsDistanceFnNamesLower.contains(&x.FuncName.L) {
        return None;
    }

    let args = x.GetArgs();
    // 两个参数中必须恰好包含一个向量列引用和一个向量常量；其他参数不会被计数。
    let mut vectorConstant: Option<&Constant> = None;
    let mut vectorColumn: Option<&Column> = None;
    let mut nVectorColumns = 0usize;
    let mut nVectorConstants = 0usize;

    for arg in args {
        if let Some(v) = arg.as_any().downcast_ref::<Column>() {
            // 名义上是 Column 仍不够，底层字段类型也必须是 TiDB VECTOR FLOAT32。
            if v.RetType.as_ref()?.GetType() != mysql::TypeTiDBVectorFloat32 {
                return None;
            }
            vectorColumn = Some(v);
            nVectorColumns += 1;
        } else if let Some(v) = arg.as_any().downcast_ref::<Constant>() {
            if v.RetType.as_ref()?.GetType() != mysql::TypeTiDBVectorFloat32 {
                return None;
            }
            vectorConstant = Some(v);
            nVectorConstants += 1;
        }
    }

    if nVectorColumns != 1 || nVectorConstants != 1 {
        return None;
    }

    // 计数检查保证这两个 Option 已有值；若表达式树违反内部约束，仍安全返回 None。
    let vectorConstant = vectorConstant?;
    let vectorColumn = vectorColumn?;
    intest::Assert(
        vectorConstant.Value.Kind() == types::KindVectorFloat32,
        &[
            intest::AssertArg::from("internal: expect vectorFloat32 constant, but got %s"),
            intest::AssertArg::from(vectorConstant.Value.String()),
        ],
    );

    let pb_code = tipb::ScalarFuncSig::from_i32(x.Function.PbCode())?;

    Some(VSInfo {
        DistanceFnName: x.FuncName.clone(),
        FnPbCode: pb_code,
        Vec: vectorConstant.Value.GetVectorFloat32(),
        Column: vectorColumn,
    })
}
