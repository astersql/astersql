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

// 规划器杂项访问与谓词提取接口。
//
// 覆盖 Access Object、SHOW/内存表谓词提取器、数据访问者（DataAccesser）及分区表能力。
// 这些接口让 EXPLAIN、下推过滤与分区裁剪在抽象层协作，而不依赖具体物理算子实现。

// expression、tables、types、collate 与 tipb 类型均复用对应依赖 crate 的公开契约。

use crate::types;
use expression::collate;

/// 对应 Go 的 `AccessObject`：表示执行算子访问的表、分区或索引，也是 EXPLAIN 的 access object 列。
pub trait AccessObject {
    /// 返回面向用户展示的访问对象文本。
    fn string(&self) -> String;

    /// 返回用于归一化 EXPLAIN 或摘要计算的稳定文本。
    fn normalized_string(&self) -> String;

    /// 将访问对象写入二进制计划的 protobuf 算子。
    ///
    // / Go 传入 `*tipb.ExplainOperator` 并原地修改；用可变引用保留该副作用边界，
    /// 这里只声明转换，不会实际序列化或发送 protobuf。
    fn set_into_pb(&self, operator: &mut tipb::ExplainOperator);
}

/// 对应 Go 的 `ShowPredicateExtractor`：从 SHOW 语句的 LIKE/ILIKE 条件中提取可下推模式。
///
/// 例如 `SHOW COLUMNS FROM t LIKE '%abc%'` 原本需要读取全部内存表结果再过滤；提取器让读取阶段
/// 提前掌握过滤模式。接口只提供信息，不在此处访问任何组件。
pub trait ShowPredicateExtractor {
    /// 执行提取并返回是否成功获得可下推谓词。
    fn extract(&mut self) -> bool;

    /// 返回用于 EXPLAIN 的提取器说明。
    fn explain_info(&self) -> String;

    /// 返回谓词作用的字段名。
    fn field(&self) -> String;

    /// 返回按字段排序规则编译后的通配符模式。
    fn field_pattern_like(&self) -> Box<dyn collate::WildcardPattern>;
}

/// 对应 Go 的 `MemTablePredicateExtractor`：从 WHERE 条件提取内存表读取阶段可下推的谓词。
///
/// 典型场景是先提取 cluster_config 的 type/instance 条件，再由执行器仅请求目标组件。
/// 提取后的返回值必须是“仍需上层计算”的表达式，不能静默丢弃未识别条件。
pub trait MemTablePredicateExtractor {
    /// 接收规划上下文、输入 schema、字段名和原始谓词，返回无法下推的剩余谓词。
    ///
    /// `NameSlice` 保留 Go `[]*types.FieldName` 中元素可空的语义。
    fn extract(
        &mut self,
        ctx: &dyn crate::PlanContext,
        schema: &expression::Schema,
        names: &types::NameSlice,
        predicates: &[expression::ExprBox],
    ) -> Vec<expression::ExprBox>;

    /// 根据物理计划返回该提取器的基础说明。
    fn explain_info(&self, plan: &dyn crate::PhysicalPlan) -> String;
}

/// 对应可选接口 `MemTableRowLimitHintSetter`，为内存表扫描设置提前停止提示。
/// 此 limit 仅允许优化读取过程，不能单独改变查询语义或最终返回行数。
pub trait MemTableRowLimitHintSetter {
    /// 设置读取阶段可提前停止的行数上限提示。
    fn set_row_limit_hint(&mut self, limit: u64);
}

/// 对应可选接口 `MemTableDescHintSetter`，提示执行器在支持时按降序产生数据。
/// 此标志同样只是执行提示，不得自行改变查询语义。
pub trait MemTableDescHintSetter {
    /// 设置是否按降序产出内存表行。
    fn set_desc(&mut self, desc: bool);
}

/// 对应 Go 的 `DataAccesser`：标识可以访问底层数据的计划节点。
/// PhysicalTableScan、PhysicalIndexScan、PointGetPlan、BatchPointScan 和 PhysicalMemTable 均可实现它。
pub trait DataAccesser {
    /// 返回计划访问的表、分区与索引信息。
    fn access_object(&self) -> &dyn AccessObject;

    /// 返回 access object 之外的算子说明；normalized 控制是否生成稳定的归一化文本。
    fn operator_info(&self, normalized: bool) -> String;
}

/// 对应 Go 的 `PartitionAccesser`：表示计划能够访问分区数据。
pub trait PartitionAccesser {
    /// 访问对象的计算依赖规划上下文；这里只传递上下文，不执行实际分区访问。
    fn access_object(&self, ctx: &dyn crate::PlanContext) -> &dyn AccessObject;
}

/// 对应 Go 的 `PartitionTable`：为实现分区能力的表暴露分区表达式。
pub trait PartitionTable {
    /// Go 返回 `*tables.PartitionExpr`；Rust 使用共享引用表达只读、可空语义由后续接线确认。
    fn partition_expr(&self) -> Option<&tables::PartitionExpr>;
}
