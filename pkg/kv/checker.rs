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

// 请求与表达式下推白名单检查器（RequestTypeSupportedChecker）。
//
// 在将算子/表达式下推到存储层（TiKV / TiFlash 等）前，按请求大类
// （Select / Index / DAG / Analyze）与子类型（tipb.ExprType 或 KV 子类型）
// 判断是否受支持。数值常量来自 tipb `expression.proto`，避免耦合 gRPC 运行时。

// 对齐 pkg/kv/checker.go 的请求与表达式下推白名单。

/// RequestTypeSupportedChecker 对应 Go 的零字段检查器，用于判断表达式是否允许下推。
pub struct RequestTypeSupportedChecker;

/// Numeric values from tipb `expression.proto`. Keeping the values here avoids
/// coupling this pure capability check to a gRPC runtime.
///
/// tipb 表达式类型数值枚举：常量/列引用、聚合与窗口函数等，供白名单匹配。
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i64)]
pub enum ExprType {
    /// NULL 常量。
    Null = 0,
    /// Int64 常量。
    Int64 = 1,
    /// Uint64 常量。
    Uint64 = 2,
    /// Float32 常量。
    Float32 = 3,
    /// Float64 常量。
    Float64 = 4,
    /// 字符串常量。
    String = 5,
    /// 字节串常量。
    Bytes = 6,
    /// MySQL BIT 类型。
    MysqlBit = 101,
    /// MySQL DECIMAL 类型。
    MysqlDecimal = 102,
    /// MySQL DURATION（时间间隔）类型。
    MysqlDuration = 103,
    /// MySQL ENUM 类型。
    MysqlEnum = 104,
    /// MySQL 日期时间类型。
    MysqlTime = 107,
    /// TiDB 向量 Float32 类型。
    TiDBVectorFloat32 = 121,
    /// 列引用。
    ColumnRef = 201,
    /// COUNT 聚合。
    Count = 3001,
    /// SUM 聚合。
    Sum = 3002,
    /// AVG 聚合。
    Avg = 3003,
    /// MIN 聚合。
    Min = 3004,
    /// MAX 聚合。
    Max = 3005,
    /// FIRST 聚合。
    First = 3006,
    /// GROUP_CONCAT 聚合（通常仅 TiFlash 支持）。
    GroupConcat = 3007,
    /// 按位与聚合。
    Agg_BitAnd = 3008,
    /// 按位或聚合。
    Agg_BitOr = 3009,
    /// 按位异或聚合。
    Agg_BitXor = 3010,
    /// 近似去重计数。
    ApproxCountDistinct = 3020,
    /// 整型 SUM 特化聚合。
    SumInt = 3021,
    /// 窗口函数 ROW_NUMBER。
    RowNumber = 4001,
    /// 窗口函数 RANK。
    Rank = 4002,
    /// 窗口函数 DENSE_RANK。
    DenseRank = 4003,
    /// 窗口函数 CUME_DIST。
    CumeDist = 4004,
    /// 窗口函数 PERCENT_RANK。
    PercentRank = 4005,
    /// 窗口函数 NTILE。
    Ntile = 4006,
    /// 窗口函数 LEAD。
    Lead = 4007,
    /// 窗口函数 LAG。
    Lag = 4008,
    /// 窗口函数 FIRST_VALUE。
    FirstValue = 4009,
    /// 窗口函数 LAST_VALUE。
    LastValue = 4010,
    /// 窗口函数 NTH_VALUE。
    NthValue = 4011,
}

impl RequestTypeSupportedChecker {
    /// IsRequestTypeSupported 按请求大类和子类型判断存储层是否支持。
    pub fn IsRequestTypeSupported(&self, reqType: i64, subType: i64) -> bool {
        match reqType {
            ReqTypeSelect | ReqTypeIndex => {
                // Select/Index 的基础、分组和 TopN 子类型无需转换为 tipb 表达式。
                match subType {
                    ReqSubTypeGroupBy | ReqSubTypeBasic | ReqSubTypeTopN => true,
                    _ => self.supportExpr(subType),
                }
            }
            // DAG 的 subType 本身就是 tipb.ExprType 数值，直接复用表达式白名单。
            ReqTypeDAG => self.supportExpr(subType),
            // Analyze 在 Go 中不检查 subType。
            ReqTypeAnalyze => true,
            _ => false,
        }
    }

    /// supportExpr 保留 Go 的历史下推白名单。
    /// 原实现标记为待废弃：存在多个下推存储引擎后，更准确的检查应在 planner 阶段完成；
    /// 聚合下推目前另由 `CheckAggCanPushCop` 检查。
    fn supportExpr(&self, exprType: i64) -> bool {
        // Go converts the subtype to tipb.ExprType (int32), truncating high bits.
        let exprType = exprType as i32 as i64;
        match exprType {
            // 常量、列引用以及基础 MySQL/TiDB 数据类型。
            value
                if matches!(
                    value,
                    x if x == ExprType::Null as i64
                        || x == ExprType::Int64 as i64
                        || x == ExprType::Uint64 as i64
                        || x == ExprType::String as i64
                        || x == ExprType::Bytes as i64
                        || x == ExprType::MysqlDuration as i64
                        || x == ExprType::MysqlTime as i64
                        || x == ExprType::MysqlDecimal as i64
                        || x == ExprType::Float32 as i64
                        || x == ExprType::Float64 as i64
                        || x == ExprType::ColumnRef as i64
                        || x == ExprType::MysqlEnum as i64
                        || x == ExprType::MysqlBit as i64
                        || x == ExprType::TiDBVectorFloat32 as i64
                ) =>
            {
                true
            }

            // 聚合函数。GroupConcat 仅由 TiFlash 支持，TiKV 场景需在本函数外另行检查。
            value
                if matches!(
                    value,
                    x if x == ExprType::Count as i64
                        || x == ExprType::First as i64
                        || x == ExprType::Max as i64
                        || x == ExprType::Min as i64
                        || x == ExprType::Sum as i64
                        || x == ExprType::Avg as i64
                        || x == ExprType::SumInt as i64
                        || x == ExprType::Agg_BitXor as i64
                        || x == ExprType::Agg_BitAnd as i64
                        || x == ExprType::Agg_BitOr as i64
                        || x == ExprType::ApproxCountDistinct as i64
                        || x == ExprType::GroupConcat as i64
                ) =>
            {
                true
            }

            // 窗口函数白名单与 Go switch 顺序一致。
            value
                if matches!(
                    value,
                    x if x == ExprType::RowNumber as i64
                        || x == ExprType::Rank as i64
                        || x == ExprType::DenseRank as i64
                        || x == ExprType::CumeDist as i64
                        || x == ExprType::PercentRank as i64
                        || x == ExprType::Ntile as i64
                        || x == ExprType::Lead as i64
                        || x == ExprType::Lag as i64
                        || x == ExprType::FirstValue as i64
                        || x == ExprType::LastValue as i64
                        || x == ExprType::NthValue as i64
                ) =>
            {
                true
            }

            // Go 允许两个并非 protobuf ExprType 成员的 KV 子类型常量。
            value if value == ReqSubTypeDesc => true,
            value if value == ReqSubTypeSignature => true,
            _ => false,
        }
    }
}
