// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0

// 逻辑算子 Hash64 / Equals 生成实现。
//
// 为各类逻辑计划节点提供 64 位哈希与结构相等判定，供 Cascades 记忆化、
// 计划去重等场景使用。哈希对表达式使用 CanonicalHashCode，对列使用 UniqueID。

use crate::*;
use std::hash::Hasher;

// 写入长度前缀再写字节，避免拼接歧义。
fn hash_bytes(h: &mut dyn Hasher, value: &[u8]) {
    h.write_usize(value.len());
    h.write(value);
}
// 按规范哈希码序列化表达式列表。
fn hash_exprs(h: &mut dyn Hasher, values: &[Expression]) {
    h.write_usize(values.len());
    for value in values {
        hash_bytes(h, &value.CanonicalHashCode());
    }
}
// 以 CanonicalHashCode 比较表达式语义相等。
fn equal_exprs(left: &[Expression], right: &[Expression]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(a, b)| a.CanonicalHashCode() == b.CanonicalHashCode())
}
// 列列表哈希：UniqueID + 物理列 ID。
fn hash_columns(h: &mut dyn Hasher, values: &[Column]) {
    h.write_usize(values.len());
    for value in values {
        h.write_i64(value.UniqueID);
        h.write_i64(value.ID);
    }
}
// 列列表相等判定。
fn equal_columns(left: &[Column], right: &[Column]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(a, b)| a.UniqueID == b.UniqueID && a.ID == b.ID)
}
// 二维列序列（如聚合可保留的排序属性）。
fn hash_column_groups(h: &mut dyn Hasher, values: &[Vec<Column>]) {
    h.write_usize(values.len());
    for value in values {
        hash_columns(h, value);
    }
}
fn equal_column_groups(left: &[Vec<Column>], right: &[Vec<Column>]) -> bool {
    left.len() == right.len() && left.iter().zip(right).all(|(a, b)| equal_columns(a, b))
}
// SortItem：排序列 UniqueID 与升降序标志。
fn hash_sort_items(h: &mut dyn Hasher, values: &[SortItem]) {
    h.write_usize(values.len());
    for item in values {
        h.write_i64(item.Col.UniqueID);
        h.write_u8(item.Desc as u8)
    }
}
// SortItem 列表相等。
fn equal_sort_items(a: &[SortItem], b: &[SortItem]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| x.Col.UniqueID == y.Col.UniqueID && x.Desc == y.Desc)
}
// ByItems：ORDER BY 表达式规范哈希 + 方向。
fn hash_by_items(h: &mut dyn Hasher, values: &[ByItems]) {
    h.write_usize(values.len());
    for item in values {
        hash_bytes(h, &item.Expr.CanonicalHashCode());
        h.write_u8(item.Desc as u8)
    }
}
// ByItems 列表相等。
fn equal_by_items(a: &[ByItems], b: &[ByItems]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.Desc == y.Desc && x.Expr.CanonicalHashCode() == y.Expr.CanonicalHashCode()
        })
}

/// Schema 列集合的 Hash64/Equals。
impl LogicalSchemaProducer {
    pub fn Hash64(&self, h: &mut dyn Hasher) {
        hash_columns(h, &self.Schema().Columns)
    }
    pub fn Equals(&self, other: &Self) -> bool {
        self.Schema().Equal(other.Schema())
    }
}

/// Join 类型与各类连接条件表达式的 Hash64/Equals。
impl LogicalJoin {
    pub fn Hash64(&self, h: &mut dyn Hasher) {
        hash_bytes(h, self.JoinType.to_string().as_bytes());
        self.LogicalSchemaProducer.Hash64(h);
        for values in [
            &self.EqualConditions,
            &self.NAEQConditions,
            &self.LeftConditions,
            &self.RightConditions,
            &self.OtherConditions,
        ] {
            hash_exprs(h, values)
        }
    }
    pub fn Equals(&self, o: &Self) -> bool {
        self.JoinType == o.JoinType
            && self.LogicalSchemaProducer.Equals(&o.LogicalSchemaProducer)
            && equal_exprs(&self.EqualConditions, &o.EqualConditions)
            && equal_exprs(&self.NAEQConditions, &o.NAEQConditions)
            && equal_exprs(&self.LeftConditions, &o.LeftConditions)
            && equal_exprs(&self.RightConditions, &o.RightConditions)
            && equal_exprs(&self.OtherConditions, &o.OtherConditions)
    }
}
/// 聚合函数名/参数/DISTINCT 与 GROUP BY 的 Hash64/Equals。
impl LogicalAggregation {
    pub fn Hash64(&self, h: &mut dyn Hasher) {
        self.LogicalSchemaProducer.Hash64(h);
        h.write_usize(self.AggFuncs.len());
        for function in &self.AggFuncs {
            hash_bytes(h, function.Name.as_bytes());
            hash_exprs(h, &function.Args);
            h.write_u8(function.Mode as u8);
            h.write_u8(function.HasDistinct as u8);
            hash_by_items(h, &function.OrderByItems)
        }
        hash_exprs(h, &self.GroupByItems);
        hash_column_groups(h, &self.PossibleProperties)
    }
    pub fn Equals(&self, o: &Self) -> bool {
        self.LogicalSchemaProducer.Equals(&o.LogicalSchemaProducer)
            && self.AggFuncs.len() == o.AggFuncs.len()
            && self
                .AggFuncs
                .iter()
                .zip(&o.AggFuncs)
                .all(|(a, b)| a.Equals(&*b as &dyn std::any::Any))
            && equal_exprs(&self.GroupByItems, &o.GroupByItems)
            && equal_column_groups(&self.PossibleProperties, &o.PossibleProperties)
    }
}
/// 在 LogicalJoin 基础上叠加关联列与 Lateral/去关联标志。
impl LogicalApply {
    pub fn Hash64(&self, h: &mut dyn Hasher) {
        self.LogicalJoin.Hash64(h);
        for c in &self.CorCols {
            h.write_i64(c.column.UniqueID)
        }
        h.write_u8(self.NoDecorrelate as u8);
        h.write_u8(self.IsLateral as u8)
    }
    pub fn Equals(&self, o: &Self) -> bool {
        self.LogicalJoin.Equals(&o.LogicalJoin)
            && self.CorCols.len() == o.CorCols.len()
            && self
                .CorCols
                .iter()
                .zip(&o.CorCols)
                .all(|(a, b)| a.column.UniqueID == b.column.UniqueID)
            && self.NoDecorrelate == o.NoDecorrelate
            && self.IsLateral == o.IsLateral
    }
}
/// ROLLUP/Expand 分组列、层级表达式与 GID/GPos 的 Hash64/Equals。
impl LogicalExpand {
    pub fn Hash64(&self, h: &mut dyn Hasher) {
        self.LogicalSchemaProducer.Hash64(h);
        hash_columns(h, &self.DistinctGroupByCol);
        hash_exprs(h, &self.DistinctGbyExprs);
        h.write_usize(self.DistinctSize);
        for set in &self.RollupGroupingSets.0 {
            for id in &set.ColumnIDs {
                h.write_i64(*id)
            }
        }
        for level in &self.LevelExprs {
            hash_exprs(h, level)
        }
        h.write_i64(self.GID.as_ref().map_or(0, |c| c.UniqueID));
        h.write_i64(self.GPos.as_ref().map_or(0, |c| c.UniqueID))
    }
    pub fn Equals(&self, o: &Self) -> bool {
        self.LogicalSchemaProducer.Equals(&o.LogicalSchemaProducer)
            && equal_columns(&self.DistinctGroupByCol, &o.DistinctGroupByCol)
            && equal_exprs(&self.DistinctGbyExprs, &o.DistinctGbyExprs)
            && self.DistinctSize == o.DistinctSize
            && self.RollupGroupingSets.0.iter().map(|s| &s.ColumnIDs).eq(o
                .RollupGroupingSets
                .0
                .iter()
                .map(|s| &s.ColumnIDs))
            && self.LevelExprs.len() == o.LevelExprs.len()
            && self
                .LevelExprs
                .iter()
                .zip(&o.LevelExprs)
                .all(|(a, b)| equal_exprs(a, b))
            && self.GID.as_ref().map(|c| c.UniqueID) == o.GID.as_ref().map(|c| c.UniqueID)
            && self.GPos.as_ref().map(|c| c.UniqueID) == o.GPos.as_ref().map(|c| c.UniqueID)
    }
}
/// Limit 的 PartitionBy/Offset/Count 哈希与相等。
impl LogicalLimit {
    pub fn Hash64(&self, h: &mut dyn Hasher) {
        self.LogicalSchemaProducer.Hash64(h);
        hash_sort_items(h, &self.PartitionBy);
        h.write_u64(self.Offset);
        h.write_u64(self.Count)
    }
    pub fn Equals(&self, o: &Self) -> bool {
        self.LogicalSchemaProducer.Equals(&o.LogicalSchemaProducer)
            && equal_sort_items(&self.PartitionBy, &o.PartitionBy)
            && self.Offset == o.Offset
            && self.Count == o.Count
    }
}
/// 仅按输出 schema 列判定。
impl LogicalMaxOneRow {
    pub fn Hash64(&self, h: &mut dyn Hasher) {
        hash_columns(h, &LogicalPlan::Schema(self).Columns)
    }
    pub fn Equals(&self, o: &Self) -> bool {
        LogicalPlan::Schema(self).Equal(LogicalPlan::Schema(o))
    }
}
/// 表 ID、别名、下推/全部谓词与物理表偏好等。
impl DataSource {
    pub fn Hash64(&self, h: &mut dyn Hasher) {
        self.LogicalSchemaProducer.Hash64(h);
        h.write_i64(self.TableInfo.ID);
        hash_bytes(
            h,
            self.TableAsName
                .as_ref()
                .map_or("", |n| n.L.as_str())
                .as_bytes(),
        );
        hash_exprs(h, &self.PushedDownConds);
        hash_exprs(h, &self.AllConds);
        h.write_i64(self.PhysicalTableID);
        h.write_i32(self.PreferStoreType);
        h.write_u8(self.IsForUpdateRead as u8)
    }
    pub fn Equals(&self, o: &Self) -> bool {
        self.LogicalSchemaProducer.Equals(&o.LogicalSchemaProducer)
            && self.TableInfo.ID == o.TableInfo.ID
            && self.TableAsName.as_ref().map(|n| &n.L) == o.TableAsName.as_ref().map(|n| &n.L)
            && equal_exprs(&self.PushedDownConds, &o.PushedDownConds)
            && equal_exprs(&self.AllConds, &o.AllConds)
            && self.PhysicalTableID == o.PhysicalTableID
            && self.PreferStoreType == o.PreferStoreType
            && self.IsForUpdateRead == o.IsForUpdateRead
    }
}
/// 内存表：库名与 TableInfo.ID。
impl LogicalMemTable {
    pub fn Hash64(&self, h: &mut dyn Hasher) {
        self.LogicalSchemaProducer.Hash64(h);
        hash_bytes(h, self.DBName.L.as_bytes());
        h.write_i64(self.TableInfo.ID)
    }
    pub fn Equals(&self, o: &Self) -> bool {
        self.LogicalSchemaProducer.Equals(&o.LogicalSchemaProducer)
            && self.DBName.L == o.DBName.L
            && self.TableInfo.ID == o.TableInfo.ID
    }
}
/// UnionAll：按 schema 生产者相等。
impl LogicalUnionAll {
    pub fn Hash64(&self, h: &mut dyn Hasher) {
        self.LogicalSchemaProducer.Hash64(h)
    }
    pub fn Equals(&self, o: &Self) -> bool {
        self.LogicalSchemaProducer.Equals(&o.LogicalSchemaProducer)
    }
}
/// 分区 UnionAll：委托内嵌 LogicalUnionAll。
impl LogicalPartitionUnionAll {
    pub fn Hash64(&self, h: &mut dyn Hasher) {
        self.LogicalUnionAll.Hash64(h)
    }
    pub fn Equals(&self, o: &Self) -> bool {
        self.LogicalUnionAll.Equals(&o.LogicalUnionAll)
    }
}
/// 投影表达式列表。
impl LogicalProjection {
    pub fn Hash64(&self, h: &mut dyn Hasher) {
        self.LogicalSchemaProducer.Hash64(h);
        hash_exprs(h, &self.Exprs);
        h.write_u8(self.CalculateNoDelay as u8);
        h.write_u8(self.Proj4Expand as u8)
    }
    pub fn Equals(&self, o: &Self) -> bool {
        self.LogicalSchemaProducer.Equals(&o.LogicalSchemaProducer)
            && equal_exprs(&self.Exprs, &o.Exprs)
            && self.CalculateNoDelay == o.CalculateNoDelay
            && self.Proj4Expand == o.Proj4Expand
    }
}
/// Selection：用节点 HashCode 作为相等依据。
impl LogicalSelection {
    pub fn Hash64(&self, h: &mut dyn Hasher) {
        hash_bytes(h, &self.HashCode())
    }
    pub fn Equals(&self, o: &Self) -> bool {
        self.HashCode() == o.HashCode()
    }
}
/// Sequence：按 schema 列。
impl LogicalSequence {
    pub fn Hash64(&self, h: &mut dyn Hasher) {
        hash_columns(h, &self.Schema().Columns)
    }
    pub fn Equals(&self, o: &Self) -> bool {
        self.Schema().Equal(o.Schema())
    }
}
/// SHOW 类型与库名。
impl LogicalShow {
    pub fn Hash64(&self, h: &mut dyn Hasher) {
        self.LogicalSchemaProducer.Hash64(h)
    }
    pub fn Equals(&self, o: &Self) -> bool {
        self.LogicalSchemaProducer.Equals(&o.LogicalSchemaProducer)
    }
}
/// SHOW DDL JOBS 的 JobNumber。
impl LogicalShowDDLJobs {
    pub fn Hash64(&self, h: &mut dyn Hasher) {
        self.LogicalSchemaProducer.Hash64(h)
    }
    pub fn Equals(&self, o: &Self) -> bool {
        self.LogicalSchemaProducer.Equals(&o.LogicalSchemaProducer)
    }
}
/// 排序 ByItems。
impl LogicalSort {
    pub fn Hash64(&self, h: &mut dyn Hasher) {
        hash_by_items(h, &self.ByItems)
    }
    pub fn Equals(&self, o: &Self) -> bool {
        equal_by_items(&self.ByItems, &o.ByItems)
    }
}
/// TableDual（常量行源）的 RowCount。
impl LogicalTableDual {
    pub fn Hash64(&self, h: &mut dyn Hasher) {
        self.LogicalSchemaProducer.Hash64(h);
        h.write_i32(self.RowCount)
    }
    pub fn Equals(&self, o: &Self) -> bool {
        self.LogicalSchemaProducer.Equals(&o.LogicalSchemaProducer) && self.RowCount == o.RowCount
    }
}
/// TopN：排序项、分区、Offset/Count。
impl LogicalTopN {
    pub fn Hash64(&self, h: &mut dyn Hasher) {
        self.LogicalSchemaProducer.Hash64(h);
        hash_by_items(h, &self.ByItems);
        hash_sort_items(h, &self.PartitionBy);
        h.write_u64(self.Offset);
        h.write_u64(self.Count);
        h.write_u8(self.PreferLimitToCop as u8)
    }
    pub fn Equals(&self, o: &Self) -> bool {
        self.LogicalSchemaProducer.Equals(&o.LogicalSchemaProducer)
            && equal_by_items(&self.ByItems, &o.ByItems)
            && equal_sort_items(&self.PartitionBy, &o.PartitionBy)
            && self.Offset == o.Offset
            && self.Count == o.Count
            && self.PreferLimitToCop == o.PreferLimitToCop
    }
}
/// UnionScan 条件与 Handle 列。
impl LogicalUnionScan {
    pub fn Hash64(&self, h: &mut dyn Hasher) {
        hash_exprs(h, &self.Conditions);
        for column in self.HandleCols.IterColumns() {
            h.write_i64(column.UniqueID)
        }
    }
    pub fn Equals(&self, o: &Self) -> bool {
        equal_exprs(&self.Conditions, &o.Conditions)
            && self.HandleCols.NumCols() == o.HandleCols.NumCols()
            && self
                .HandleCols
                .IterColumns()
                .zip(o.HandleCols.IterColumns())
                .all(|(a, b)| a.UniqueID == b.UniqueID)
    }
}
/// 窗口函数描述与哈希一致性检查。
impl LogicalWindow {
    pub fn Equals(&self, o: &Self) -> bool {
        self.Hash64() == o.Hash64()
            && self.WindowFuncDescs.len() == o.WindowFuncDescs.len()
            && self
                .WindowFuncDescs
                .iter()
                .zip(&o.WindowFuncDescs)
                .all(|(a, b)| a.Name == b.Name && equal_exprs(&a.Args, &b.Args))
    }
}
/// 行锁类型与表 Handle 映射键集合。
impl LogicalLock {
    pub fn Hash64(&self, h: &mut dyn Hasher) {
        h.write_i32(self.Lock.LockType as i32);
        // 对表 ID 键排序后再哈希，保证键集合遍历顺序无关。
        let mut keys = self.TblID2Handle.keys().copied().collect::<Vec<_>>();
        keys.sort_unstable();
        for key in keys {
            h.write_i64(key)
        }
    }
    pub fn Equals(&self, o: &Self) -> bool {
        self.Lock.LockType == o.Lock.LockType
            && self
                .TblID2Handle
                .keys()
                .collect::<std::collections::BTreeSet<_>>()
                == o.TblID2Handle
                    .keys()
                    .collect::<std::collections::BTreeSet<_>>()
    }
}
