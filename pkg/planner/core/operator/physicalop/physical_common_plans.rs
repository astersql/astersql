// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 物理计划公共骨架：表达式/属性/统计、算子种类枚举，以及简化的 Insert/Update/Delete 模型，
// 供同目录其它物理算子做选优与内存估算时复用。

// 规划、表达式解析和内存估算语义。
//
// use std::collections::{HashMap, HashSet};
//
/// InsertGeneratedColumns 保存 INSERT 中需要在执行器求值的生成列表达式和重复键赋值。
// pub struct InsertGeneratedColumns {
//     pub exprs: Vec<Box<dyn expression::Expression>>,
//     pub on_duplicates: Vec<Box<expression::Assignment>>,
// }
//
// impl InsertGeneratedColumns {
/// 计划缓存克隆时表达式与 Assignment 都需复制，避免参数重写污染原计划。
//     fn clone_for_plan_cache(&self) -> Self {
//         Self {
//             exprs: utilfuncp::CloneExpressionsForPlanCache(&self.exprs, None),
//             on_duplicates: util::CloneAssignments(&self.on_duplicates),
//         }
//     }
//
//     pub fn memory_usage(&self) -> i64 {
//         let mut sum = size::SizeOfSlice * 3
//             + self.on_duplicates.capacity() as i64 * size::SizeOfPointer
//             + self.exprs.capacity() as i64 * size::SizeOfInterface;
//         for expr in &self.exprs { sum += expr.MemoryUsage(); }
//         for assignment in &self.on_duplicates { sum += assignment.MemoryUsage(); }
//         sum
//     }
// }
//
/// Insert 表示 INSERT/REPLACE 计划，包含值列表、重复键更新、生成列和外键元数据。
// pub struct Insert {
//     pub simple_schema_producer: SimpleSchemaProducer,
//     pub table: Box<dyn table::Table>,
//     pub table_schema: Box<expression::Schema>,
//     pub table_col_names: types::NameSlice,
//     pub columns: Vec<Box<ast::ColumnName>>,
//     pub lists: Vec<Vec<Box<dyn expression::Expression>>>,
//     pub on_duplicate: Vec<Box<expression::Assignment>>,
//     pub schema4_on_duplicate: Box<expression::Schema>,
//     pub names4_on_duplicate: types::NameSlice,
//     pub gen_cols: InsertGeneratedColumns,
//     pub select_plan: Option<Box<dyn base::PhysicalPlan>>,
//     pub is_replace: bool,
//     pub ignore_err: bool,
// 值列表表达式引用其他列时，执行前必须补默认值。
//     pub need_fill_default_value: bool,
//     pub all_assignments_are_constant: bool,
//     pub row_len: i32,
// 计划缓存克隆时外键对象必须清空，由后续阶段重建。
//     pub fk_checks: Vec<Box<FKCheck>>,
//     pub fk_cascades: Vec<Box<FKCascade>>,
// }
//
// impl Insert {
//     pub fn init(mut self, ctx: base::PlanContext) -> Box<Self> {
//         self.simple_schema_producer.plan = baseimpl::NewBasePlan(ctx, plancodec::TypeInsert, 0);
//         Box::new(self)
//     }
//
/// 按 Go 的切片容量口径累计表 schema、值表达式、重复键赋值、选择子计划及外键检查。
//     pub fn memory_usage(&self) -> i64 {
//         let mut sum = self.simple_schema_producer.MemoryUsage() + size::SizeOfInterface
//             + size::SizeOfSlice * 7
//             + (self.table_col_names.capacity() + self.columns.capacity() + self.on_duplicate.capacity()
//                 + self.names4_on_duplicate.capacity() + self.fk_checks.capacity()) as i64 * size::SizeOfPointer
//             + self.gen_cols.memory_usage() + size::SizeOfInterface + size::SizeOfBool * 4 + size::SizeOfInt
//             + self.table_schema.MemoryUsage() + self.schema4_on_duplicate.MemoryUsage();
//         if let Some(plan) = &self.select_plan { sum += plan.MemoryUsage(); }
//         for name in &self.table_col_names { sum += name.MemoryUsage(); }
//         for exprs in &self.lists {
//             sum += size::SizeOfSlice + exprs.capacity() as i64 * size::SizeOfInterface;
//             for expr in exprs { sum += expr.MemoryUsage(); }
//         }
//         for assignment in &self.on_duplicate { sum += assignment.MemoryUsage(); }
//         for name in &self.names4_on_duplicate { sum += name.MemoryUsage(); }
//         for check in &self.fk_checks { sum += check.MemoryUsage(); }
//         sum
//     }
//
/// 解析重复键和生成列表达式；LazyErr 存在时 Expr 为 nil，必须跳过该分支。
//     pub fn resolve_indices(&mut self) -> Result<(), Error> {
//         self.simple_schema_producer.ResolveIndices()?;
//         for assignment in &mut self.on_duplicate {
//             assignment.Col = assignment.Col.ResolveIndices(&self.table_schema)?.downcast()?;
//             if let Some(expr) = &assignment.Expr {
//                 assignment.Expr = Some(expr.ResolveIndices(&self.schema4_on_duplicate)?);
//             }
//         }
//         for expr in &mut self.gen_cols.exprs {
//             *expr = expr.ResolveIndices(&self.table_schema)?;
//         }
//         for assignment in &mut self.gen_cols.on_duplicates {
//             assignment.Col = assignment.Col.ResolveIndices(&self.table_schema)?.downcast()?;
//             assignment.Expr = Some(assignment.Expr.as_ref().unwrap().ResolveIndices(&self.schema4_on_duplicate)?);
//         }
//         Ok(())
//     }
//
/// 把 AST 的 ON DUPLICATE 列表解析为执行表达式，同时返回被更新列名集合。
//     pub fn resolve_on_duplicate<F>(
//         &mut self,
//         on_dup: &mut [ast::Assignment],
//         tbl_info: &model::TableInfo,
//         mut yield_expr: F,
//     ) -> Result<HashSet<String>, Error>
//     where F: FnMut(&ast::ExprNode) -> Result<Box<dyn expression::Expression>, Error> {
//         let mut updated = HashSet::with_capacity(on_dup.len());
//         let col_map: HashMap<String, &table::Column> = self.table.Cols().iter()
//             .map(|col| (col.Name.L.clone(), col)).collect();
//         for assignment in on_dup {
// 先验证字段属于源表；隐藏列与不存在的列统一报告 UnknownColumn。
//             let idx = expression::FindFieldName(&self.table_col_names, &assignment.Column)?;
//             if idx < 0 {
//                 return Err(plannererrors::ErrUnknownColumn.GenWithStackByArgs(assignment.Column.OrigColName(), "field list"));
//             }
//             let column = col_map[&assignment.Column.Name.L];
//             if column.Hidden {
//                 return Err(plannererrors::ErrUnknownColumn.GenWithStackByArgs(&column.Name, "field list"));
//             }
// 生成列只允许赋 DEFAULT 或 DEFAULT(自身)，否则保持 MySQL 的 BadGeneratedColumn 错误。
//             if column.IsGenerated() {
//                 if is_default_expr_same_column(&self.table_col_names[idx as usize..idx as usize + 1], &assignment.Expr) { continue; }
//                 return Err(plannererrors::ErrBadGeneratedColumn.GenWithStackByArgs(&assignment.Column.Name.O, &tbl_info.Name.O));
//             }
//             if let Some(default_expr) = extract_default_expr(&mut assignment.Expr) {
//                 default_expr.Name = Some(assignment.Column.clone());
//             }
//             updated.insert(column.Name.L.clone());
//
// 多行子查询错误需延迟到重复键真正触发时，其余错误立即返回。
//             let (expr, lazy_err) = match yield_expr(&assignment.Expr) {
//                 Ok(expr) => (Some(expr), None),
//                 Err(err) if errors::Cause(&err).downcast_ref::<terror::Error>()
//                     .is_some_and(|e| e.Code() == plannererrors::ErrSubqueryMoreThan1Row.Code()) => (None, Some(err)),
//                 Err(err) => return Err(err),
//             };
//             self.on_duplicate.push(Box::new(expression::Assignment {
//                 Col: self.table_schema.Columns[idx as usize].Clone(),
//                 ColName: self.table_col_names[idx as usize].ColName.clone(),
//                 Expr: expr,
//                 LazyErr: lazy_err,
//             }));
//         }
//         Ok(updated)
//     }
// }
//
/// Update 表示更新计划，并记录多表更新的列区间、分区表、外键检查和级联动作。
// pub struct Update {
//     pub simple_schema_producer: SimpleSchemaProducer,
//     pub ordered_list: Vec<Box<expression::Assignment>>,
//     pub all_assignments_are_constant: bool,
//     pub ignore_error: bool,
//     pub virtual_assignments_offset: i32,
//     pub select_plan: Box<dyn base::PhysicalPlan>,
//     pub tbl_col_pos_infos: TblColPosInfoSlice,
//     pub partitioned_table: Vec<Box<dyn table::PartitionedTable>>,
//     pub tbl_id2_table: HashMap<i64, Box<dyn table::Table>>,
//     pub fk_checks: HashMap<i64, Vec<Box<FKCheck>>>,
//     pub fk_cascades: HashMap<i64, Vec<Box<FKCascade>>>,
// }
//
// impl Update {
//     pub fn init(mut self, ctx: base::PlanContext) -> Box<Self> {
//         self.simple_schema_producer.plan = baseimpl::NewBasePlan(ctx, plancodec::TypeUpdate, 0);
//         Box::new(self)
//     }
//
//     pub fn memory_usage(&self) -> i64 {
//         let mut sum = self.simple_schema_producer.MemoryUsage() + size::SizeOfSlice * 3
//             + self.ordered_list.capacity() as i64 * size::SizeOfPointer + size::SizeOfBool
//             + size::SizeOfInt + size::SizeOfInterface
//             + self.partitioned_table.capacity() as i64 * size::SizeOfInterface
//             + self.tbl_id2_table.len() as i64 * (size::SizeOfInt64 + size::SizeOfInterface)
//             + self.select_plan.MemoryUsage();
//         for assignment in &self.ordered_list { sum += assignment.MemoryUsage(); }
//         for info in &self.tbl_col_pos_infos.0 { sum += info.memory_usage(); }
//         for checks in self.fk_checks.values() {
//             sum += size::SizeOfInt64 + size::SizeOfSlice + checks.capacity() as i64 * size::SizeOfPointer;
//             for check in checks { sum += check.MemoryUsage(); }
//         }
//         sum
//     }
//
/// Assignment 的目标列与右侧表达式都必须相对 SelectPlan 输出 schema 重解索引。
//     pub fn resolve_indices(&mut self) -> Result<(), Error> {
//         self.simple_schema_producer.ResolveIndices()?;
//         let schema = self.select_plan.Schema();
//         for assignment in &mut self.ordered_list {
//             assignment.Col = assignment.Col.ResolveIndices(schema)?.downcast()?;
//             assignment.Expr = Some(assignment.Expr.as_ref().unwrap().ResolveIndices(schema)?);
//         }
//         Ok(())
//     }
// }
//
/// Delete 表示单表或多表删除计划；真正删除、外键检查与级联均由执行器完成。
// pub struct Delete {
//     pub simple_schema_producer: SimpleSchemaProducer,
//     pub is_multi_table: bool,
//     pub select_plan: Box<dyn base::PhysicalPlan>,
//     pub tbl_col_pos_infos: TblColPosInfoSlice,
//     pub fk_checks: HashMap<i64, Vec<Box<FKCheck>>>,
//     pub fk_cascades: HashMap<i64, Vec<Box<FKCascade>>>,
//     pub ignore_err: bool,
// }
//
// impl Delete {
//     pub fn init(mut self, ctx: base::PlanContext) -> Box<Self> {
//         self.simple_schema_producer.plan = baseimpl::NewBasePlan(ctx, plancodec::TypeDelete, 0);
//         Box::new(self)
//     }
//
//     pub fn memory_usage(&self) -> i64 {
//         let mut sum = self.simple_schema_producer.MemoryUsage() + size::SizeOfBool
//             + size::SizeOfInterface + size::SizeOfSlice + self.select_plan.MemoryUsage();
//         for info in &self.tbl_col_pos_infos.0 { sum += info.memory_usage(); }
//         sum
//     }
//
/// 清理 tblID->handle 映射，只保留确实来自待删除表输出列的 HandleCols。
//     pub fn clean_tbl_id2_handle_map(
//         &self,
//         tables_to_delete: &HashMap<i64, Vec<Box<resolve::TableNameW>>>,
//         mut handles: HashMap<i64, Vec<Box<dyn util::HandleCols>>>,
//         output_names: &[Box<types::FieldName>],
//     ) -> HashMap<i64, Vec<Box<dyn util::HandleCols>>> {
//         handles.retain(|table_id, cols| {
//             let Some(names) = tables_to_delete.get(table_id) else { return false; };
//             cols.retain(|handle_cols| handle_cols.IterColumns().any(|col| {
//                 self.matching_deleting_table(names, &output_names[col.Index as usize])
//             }));
//             !cols.is_empty()
//         });
//         handles
//     }
//
/// 数据库名为空可匹配当前库；表名始终按不区分大小写字段 L 比较。
//     fn matching_deleting_table(&self, names: &[Box<resolve::TableNameW>], name: &types::FieldName) -> bool {
//         names.iter().any(|n| (name.DBName.L.is_empty() || name.DBName.L == n.DBInfo.Name.L)
//             && name.TblName.L == n.Name.L)
//     }
// }
//
/// TblColPosInfo 把连续列区间映射到表 handle，并保留列裁剪后的索引行布局。
// pub struct TblColPosInfo {
//     pub tbl_id: i64,
// 半开区间 [start, end)。
//     pub start: i32,
//     pub end: i32,
//     pub handle_cols: Option<Box<dyn util::HandleCols>>,
// None 表示没有发生列裁剪。
//     pub indexes_row_layout: Option<table::IndexesLayout>,
// }
//
// impl TblColPosInfo {
//     pub fn memory_usage(&self) -> i64 {
//         size::SizeOfInt64 + size::SizeOfInt * 2
//             + self.handle_cols.as_ref().map_or(0, |v| v.MemoryUsage())
//     }
//
/// 按 Start 比较，供区间列表升序排序。
//     pub fn cmp(&self, other: &Self) -> std::cmp::Ordering { self.start.cmp(&other.start) }
// }
//
/// 为 Go 的 []TblColPosInfo 包装排序和查找方法。
// pub struct TblColPosInfoSlice(pub Vec<TblColPosInfo>);
//
// impl TblColPosInfoSlice {
//     pub fn len(&self) -> usize { self.0.len() }
//
/// 找到 Start 不大于 colOrdinal 的最后一个候选区间；partition_point 对应 Go sort.Search 的严格 > 谓词。
//     pub fn find_tbl_idx(&self, col_ordinal: i32) -> Option<usize> {
//         if self.0.is_empty() { return None; }
//         let behind = self.0.partition_point(|info| info.start <= col_ordinal);
//         behind.checked_sub(1)
//     }
// }
//
/// 判断赋值是否为 DEFAULT 或 DEFAULT(当前列)，用于生成列写入合法性检查。
// pub fn is_default_expr_same_column(names: &types::NameSlice, node: &ast::ExprNode) -> bool {
//     if let Some(expr) = node.downcast_ref::<ast::DefaultExpr>() {
//         if expr.Name.is_none() { return true; }
//         return expression::FindFieldName(names, expr.Name.as_ref().unwrap()).is_ok_and(|idx| idx == 0);
//     }
//     false
// }
//
/// 只提取不带列名的 DEFAULT 关键字；DEFAULT(a) 是函数语义，必须返回 None。
// pub fn extract_default_expr(node: &mut ast::ExprNode) -> Option<&mut ast::DefaultExpr> {
//     node.downcast_mut::<ast::DefaultExpr>().filter(|expr| expr.Name.is_none())
// }
// */

// --- 可运行的简化实现 ---
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq)]
/// 简化 Datum：常量求值与 EXPLAIN 展示用。
pub enum Datum {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Text(String),
}

#[derive(Clone, Debug, PartialEq)]
/// 物理层表达式树：列、相关列、常量、标量函数与 DEFAULT。
pub enum PhysicalExpr {
    Column(i64),
    CorrelatedColumn(i64),
    Constant(Datum),
    Scalar {
        function: String,
        args: Vec<PhysicalExpr>,
    },
    Default {
        column_name: Option<String>,
    },
}
impl PhysicalExpr {
    /// 收集表达式引用的列/相关列 ID。
    pub fn columns(&self) -> BTreeSet<i64> {
        match self {
            Self::Column(id) | Self::CorrelatedColumn(id) => BTreeSet::from([*id]),
            Self::Scalar { args, .. } => args.iter().flat_map(Self::columns).collect(),
            _ => BTreeSet::new(),
        }
    }
    /// 校验列引用落在孩子 schema 中，并递归处理标量参数。
    pub fn resolve_indices(&mut self, schema: &[i64]) -> Result<(), String> {
        match self {
            // 列必须出现在输入 schema，否则报缺失。
            Self::Column(id) | Self::CorrelatedColumn(id) => {
                if schema.contains(id) {
                    Ok(())
                } else {
                    Err(format!("column {id} is absent from child schema"))
                }
            }
            Self::Scalar { args, .. } => args
                .iter_mut()
                .try_for_each(|arg| arg.resolve_indices(schema)),
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// 物理任务类型：TiDB Root / TiKV Coprocessor / TiFlash MPP。
pub enum TaskType {
    #[default]
    Root,
    Cop,
    Mpp,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// MPP 数据分布：任意、Hash、广播、单分区。
pub enum PartitionType {
    #[default]
    Any,
    Hash,
    Broadcast,
    Single,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// CTE producer 的 MPP 可用性，语义对应 Go `CTEProducerStatus`。
pub enum CteProducerStatus {
    #[default]
    Unknown,
    AllCanMpp,
    SomeFailedMpp,
}
#[derive(Clone, Debug, Default, PartialEq)]
/// 排序项：列 ID + 是否降序。
pub struct SortItem {
    pub column: i64,
    pub descending: bool,
}
#[derive(Clone, Debug, PartialEq)]
/// 子节点需满足的物理属性（任务层、排序、期望行数、分区）。
pub struct PhysicalProperty {
    pub task_type: TaskType,
    pub sort_items: Vec<SortItem>,
    pub expected_count: f64,
    pub partition_type: PartitionType,
    pub partition_columns: Vec<i64>,
    pub can_add_enforcer: bool,
    pub cte_producer_status: CteProducerStatus,
    pub no_cop_push_down: bool,
}
/// 默认：Root、无序、期望行数无穷、可加 enforcer。
impl Default for PhysicalProperty {
    fn default() -> Self {
        Self {
            task_type: TaskType::Root,
            sort_items: Vec::new(),
            expected_count: f64::MAX,
            partition_type: PartitionType::Any,
            partition_columns: Vec::new(),
            can_add_enforcer: true,
            cte_producer_status: CteProducerStatus::Unknown,
            no_cop_push_down: false,
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq)]
/// 简化统计：行数与版本号。
pub struct Stats {
    pub row_count: f64,
    pub version: u64,
}

#[derive(Clone, Debug, PartialEq)]
/// 物理算子种类标签，驱动选优与 EXPLAIN 分类。
pub enum PhysicalKind {
    Scan {
        table_id: i64,
    },
    Sort {
        by: Vec<SortItem>,
        partial: bool,
    },
    ExchangeSender {
        partition_type: PartitionType,
        columns: Vec<i64>,
        compression: String,
    },
    ExchangeReceiver,
    Selection {
        predicates: Vec<PhysicalExpr>,
    },
    Projection {
        expressions: Vec<PhysicalExpr>,
    },
    Insert,
    Update,
    Delete,
    Cte {
        id: i64,
    },
    CteStorage {
        id: i64,
    },
    CteSink {
        id: i64,
    },
    CteSource {
        id: i64,
    },
    CteTable {
        id: i64,
    },
    Expand,
    IndexHashJoin,
    IndexMergeJoin,
    LocalIndexLookup,
    Lock,
    Sequence,
    Shuffle,
    ShuffleReceiver,
    Other(String),
}

#[derive(Clone, Debug, PartialEq)]
/// 物理计划树节点：种类、schema、孩子、统计与子属性要求。
pub struct PhysicalPlanNode {
    pub id: i64,
    pub kind: PhysicalKind,
    pub schema: Vec<i64>,
    pub children: Vec<PhysicalPlanNode>,
    pub stats: Stats,
    pub required_properties: Vec<PhysicalProperty>,
}
impl PhysicalPlanNode {
    /// 递归累计节点与 schema 容量。
    pub fn memory_usage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64
            + (self.schema.capacity() * 8) as i64
            + self.children.iter().map(Self::memory_usage).sum::<i64>()
    }
    /// 前序遍历重分配计划 ID，避免克隆冲突。
    pub fn reset_ids(&mut self, next: &mut i64) {
        self.id = *next;
        *next += 1;
        for child in &mut self.children {
            child.reset_ids(next);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
/// INSERT 生成列表达式与 ON DUPLICATE 侧生成列赋值。
pub struct InsertGeneratedColumns {
    pub columns: Vec<i64>,
    pub expressions: Vec<PhysicalExpr>,
    pub on_duplicate_expressions: Vec<PhysicalExpr>,
}
impl InsertGeneratedColumns {
    /// 按切片容量估算。
    pub fn memory_usage(&self) -> i64 {
        (self.columns.capacity() * 8
            + (self.expressions.capacity() + self.on_duplicate_expressions.capacity())
                * std::mem::size_of::<PhysicalExpr>()) as i64
    }
}
#[derive(Clone, Debug, PartialEq)]
/// 列赋值：目标列名 + 右侧表达式。
pub struct Assignment {
    pub column_name: String,
    pub expression: PhysicalExpr,
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 简化 INSERT/REPLACE：值列表、SET、重复键更新、生成列与外键元数据。
pub struct Insert {
    /// 目标表 ID。
    pub table_id: i64,
    /// 表全部列名（校验用）。
    pub table_columns: Vec<String>,
    /// INSERT 显式列列表。
    pub columns: Vec<String>,
    /// VALUES 多行表达式。
    pub lists: Vec<Vec<PhysicalExpr>>,
    /// INSERT ... SET 赋值。
    pub set_list: Vec<Assignment>,
    /// ON DUPLICATE KEY UPDATE 列表。
    pub on_duplicate: Vec<Assignment>,
    /// 生成列相关表达式。
    pub generated: InsertGeneratedColumns,
    /// INSERT...SELECT 的子计划。
    pub select_plan: Option<PhysicalPlanNode>,
    /// 外键检查。
    pub fk_checks: Vec<crate::foreign_key::FkCheck>,
    /// 外键级联。
    pub fk_cascades: Vec<crate::foreign_key::FkCascade>,
}
impl Insert {
    /// 累计值列表与生成列内存。
    pub fn memory_usage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64
            + self.lists.iter().flatten().count() as i64
                * std::mem::size_of::<PhysicalExpr>() as i64
            + self.generated.memory_usage()
    }
    /// 相对 SELECT 子计划 schema（若有）解析赋值与生成列表达式。
    pub fn resolve_indices(&mut self) -> Result<(), String> {
        let schema = self
            .select_plan
            .as_ref()
            .map(|p| p.schema.as_slice())
            .unwrap_or(&[]);
        self.set_list
            .iter_mut()
            .chain(&mut self.on_duplicate)
            .try_for_each(|a| a.expression.resolve_indices(schema))?;
        self.generated
            .expressions
            .iter_mut()
            .chain(&mut self.generated.on_duplicate_expressions)
            .try_for_each(|e| e.resolve_indices(schema))
    }
    /// 校验 ON DUPLICATE 列属于表，并返回小写列名集合。
    pub fn resolve_on_duplicate(&self) -> Result<BTreeSet<String>, String> {
        let mut result = BTreeSet::new();
        for assignment in &self.on_duplicate {
            if !self
                .table_columns
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&assignment.column_name))
            {
                return Err(format!("unknown column {}", assignment.column_name));
            }
            result.insert(assignment.column_name.to_ascii_lowercase());
        }
        Ok(result)
    }
}
#[derive(Clone, Debug, Default, PartialEq)]
/// 简化 UPDATE：赋值列表与外键动作。
pub struct Update {
    /// 逻辑赋值列表。
    pub assignments: Vec<Assignment>,
    /// 执行顺序赋值。
    pub ordered_list: Vec<Assignment>,
    /// 是否全部为常量赋值。
    pub all_assignments_are_constant: bool,
    /// 外键检查。
    pub fk_checks: Vec<crate::foreign_key::FkCheck>,
    /// 外键级联。
    pub fk_cascades: Vec<crate::foreign_key::FkCascade>,
}
impl Update {
    /// 相对给定 schema 解析两侧赋值表达式。
    pub fn resolve_indices(&mut self, schema: &[i64]) -> Result<(), String> {
        self.assignments
            .iter_mut()
            .chain(&mut self.ordered_list)
            .try_for_each(|a| a.expression.resolve_indices(schema))
    }
}
#[derive(Clone, Debug, Default, PartialEq)]
/// 简化 DELETE：表到 handle 列映射与外键动作。
pub struct Delete {
    /// 表 ID → handle 列 ID 列表。
    pub table_id_to_handles: BTreeMap<i64, Vec<i64>>,
    /// 外键检查。
    pub fk_checks: Vec<crate::foreign_key::FkCheck>,
    /// 外键级联。
    pub fk_cascades: Vec<crate::foreign_key::FkCascade>,
}
impl Delete {
    /// 只保留真正待删除表的 handle 映射。
    pub fn clean_table_handles(&mut self, deleting_tables: &BTreeSet<i64>) {
        self.table_id_to_handles
            .retain(|id, _| deleting_tables.contains(id));
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// 混合行中某表的半开列区间 [start, end)。
pub struct TableColumnPosition {
    pub table_id: i64,
    pub start: usize,
    pub end: usize,
}
/// 按列序号查找所属表区间下标。
pub fn find_table_index(positions: &[TableColumnPosition], ordinal: usize) -> Option<usize> {
    positions.iter().rposition(|item| item.start <= ordinal)
}
/// 判断是否为 DEFAULT 或 DEFAULT(当前列)，用于生成列写入合法性。
pub fn is_default_expr_same_column(names: &[String], expression: &PhysicalExpr) -> bool {
    let PhysicalExpr::Default { column_name } = expression else {
        return false;
    };
    match column_name {
        Some(name) => names
            .first()
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(name)),
        None => true,
    }
}
