// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 外键检查与级联（FK Check / Cascade）规划骨架。
//
// 在 DML（INSERT/UPDATE/DELETE）规划阶段，根据外键元数据生成存在性检查
//（`FkCheck`）或 ON DELETE/UPDATE 级联动作（`FkCascade`）。下方大段行注释保留
// 更完整的 Go 控制流对照，文件后半为可解析的简化实现。

// 保留 DML 规划阶段生成外键检查与级联计划的完整控制流。
// property、table、plannererrors、dbutil、plancodec。
//
// use std::collections::{HashMap, HashSet};
//
/// FKCheck 表示一次外键存在性或不存在性检查。
// pub struct FKCheck {
//     pub base_physical_plan: BasePhysicalPlan,
//     pub fk: Option<FKInfoRef>,
//     pub referred_fk: Option<ReferredFKInfoRef>,
//     pub table: TableRef,
//     pub index: Option<IndexRef>,
//     pub columns: Vec<CIStr>,
//     pub index_is_primary_key: bool,
//     pub index_is_exclusive: bool,
//     pub check_exist: bool,
//     pub failed_error: PlannerError,
// }
//
// impl FKCheck {
/// Init 对应 Go 值接收者初始化：建立 BasePhysicalPlan 并设置空统计信息。
//     pub fn init(mut self, ctx: PlanContextRef) -> FKCheckRef {
//         let self_ref = FKCheckRef::new_cyclic(|me| {
//             self.base_physical_plan = new_base_physical_plan(
//                 ctx,
//                 TYPE_FOREIGN_KEY_CHECK,
//                 me.clone().into_physical_plan(),
//                 0,
//             );
//             self.base_physical_plan.plan.set_stats(StatsInfo::default());
//             self
//         });
//         self_ref
//     }
//
/// AccessObject 输出被检查的表；存在普通索引时同时输出索引名。
//     pub fn access_object(&self) -> AccessObject {
//         let table_name = self.table.meta().name.to_string();
//         match &self.index {
//             None => other_access_object(format!("table:{table_name}")),
//             Some(index) => other_access_object(format!(
//                 "table:{table_name}, index:{}",
//                 index.meta().name
//             )),
//         }
//     }
//
/// OperatorInfo 根据 FK/ReferredFK 区分 check_exist 与 check_not_exist。
//     pub fn operator_info(&self, _normalized: bool) -> String {
//         if let Some(fk) = &self.fk {
//             return format!("foreign_key:{}, check_exist", fk.name);
//         }
//         if let Some(referred) = &self.referred_fk {
//             return format!(
//                 "foreign_key:{}, check_not_exist",
//                 referred.child_fk_name
//             );
//         }
//         String::new()
//     }
//
//     pub fn explain_info(&self) -> String {
//         format!("{}, {}", self.access_object(), self.operator_info(false))
//     }
//
/// MemoryUsage 保留 Go 口径：只计结构体浅层大小与 Cols 中 CIStr 的动态占用。
//     pub fn memory_usage(&self) -> i64 {
//         EMPTY_FK_CHECK_SIZE + self.columns.iter().map(CIStr::memory_usage).sum::<i64>()
//     }
// }
//
/// FKCascade 表示 ON DELETE/UPDATE 的级联动作。
// pub struct FKCascade {
//     pub base_physical_plan: BasePhysicalPlan,
//     pub cascade_type: FKCascadeType,
//     pub referred_fk: ReferredFKInfoRef,
//     pub child_table: TableRef,
//     pub fk: FKInfoRef,
//     pub fk_columns: Vec<ColumnInfoRef>,
//     pub fk_index: Option<IndexInfoRef>,
// CascadePlans 在执行期填充，因此普通 EXPLAIN 看不到，只有 EXPLAIN ANALYZE 包含它。
//     pub cascade_plans: Vec<PlanRef>,
// }
//
// impl FKCascade {
//     pub fn init(mut self, ctx: PlanContextRef) -> FKCascadeRef {
//         FKCascadeRef::new_cyclic(|me| {
//             self.base_physical_plan = new_base_physical_plan(
//                 ctx,
//                 TYPE_FOREIGN_KEY_CASCADE,
//                 me.clone().into_physical_plan(),
//                 0,
//             );
//             self.base_physical_plan.plan.set_stats(StatsInfo::default());
//             self
//         })
//     }
//
//     pub fn access_object(&self) -> AccessObject {
//         let table_name = self.child_table.meta().name.to_string();
//         match &self.fk_index {
//             None => other_access_object(format!("table:{table_name}")),
//             Some(index) => other_access_object(format!(
//                 "table:{table_name}, index:{}",
//                 index.name
//             )),
//         }
//     }
//
//     pub fn operator_info(&self, _normalized: bool) -> String {
//         match self.cascade_type {
//             FKCascadeType::OnDelete => format!(
//                 "foreign_key:{}, on_delete:{}",
//                 self.fk.name,
//                 ReferOptionType::from(self.fk.on_delete)
//             ),
//             FKCascadeType::OnUpdate => format!(
//                 "foreign_key:{}, on_update:{}",
//                 self.fk.name,
//                 ReferOptionType::from(self.fk.on_update)
//             ),
//         }
//     }
//
//     pub fn explain_info(&self) -> String {
//         format!("{}, {}", self.access_object(), self.operator_info(false))
//     }
//
// / Go 当前只计算 FKCascade 结构体浅层大小，不额外扩展口径。
//     pub fn memory_usage(&self) -> i64 {
//         EMPTY_FK_CASCADE_SIZE
//     }
// }
//
/// FKCascadeType 区分触发级联动作的 DML 类型。
// #[derive(Clone, Copy, PartialEq, Eq)]
// #[repr(i8)]
// pub enum FKCascadeType {
//     OnDelete = 1,
//     OnUpdate = 2,
// }
//
// pub const EMPTY_FK_CHECK_SIZE: i64 = std::mem::size_of::<FKCheck>() as i64;
// pub const EMPTY_FK_CASCADE_SIZE: i64 = std::mem::size_of::<FKCascade>() as i64;
//
// impl Insert {
/// BuildOnInsertFKTriggers 为 INSERT/REPLACE/ON DUPLICATE KEY UPDATE 构造外键触发器。
//     pub fn build_on_insert_fk_triggers(
//         &mut self,
//         ctx: PlanContextRef,
//         info_schema: &dyn InfoSchema,
//         db_name: &str,
//     ) -> Result<(), PlannerError> {
//         if !ctx.session_vars().foreign_key_checks {
//             return Ok(());
//         }
//
//         let table_info = self.table.meta();
//         let mut checks = Vec::with_capacity(table_info.foreign_keys.len());
//         let mut cascades = Vec::with_capacity(table_info.foreign_keys.len());
//         let update_columns = self.build_on_duplicate_update_columns();
//
//         if !update_columns.is_empty() {
// ON DUPLICATE KEY UPDATE 可能修改父表被引用列，需构造 update 方向触发器。
//             let (referred_checks, referred_cascades) = build_on_update_referred_fk_triggers(
//                 ctx.clone(),
//                 info_schema,
//                 db_name,
//                 table_info,
//                 &update_columns,
//             )?;
//             checks.extend(referred_checks);
//             cascades.extend(referred_cascades);
//         } else if self.is_replace {
// REPLACE 会先删除冲突行，所以按 ON DELETE 规则处理父表引用。
//             let (referred_checks, referred_cascades) = self
//                 .build_on_replace_referred_fk_triggers(
//                     ctx.clone(),
//                     info_schema,
//                     db_name,
//                     table_info,
//                 )?;
//             checks.extend(referred_checks);
//             cascades.extend(referred_cascades);
//         }
//
//         for fk in &table_info.foreign_keys {
// Version < 1 的旧元数据不启用真正的外键检查。
//             if fk.version < 1 {
//                 continue;
//             }
//             let failed_error = err_no_referenced_row2(fk.to_string(db_name, &table_info.name.lower));
//             if let Some(check) = build_fk_check_on_modify_child_table(
//                 ctx.clone(),
//                 info_schema,
//                 fk.clone(),
//                 failed_error,
//             )? {
//                 checks.push(check);
//             }
//         }
//
//         if !checks.is_empty() || !cascades.is_empty() {
//             self.fk_checks = checks;
//             self.fk_cascades = cascades;
//         }
//         Ok(())
//     }
//
/// 收集 ON DUPLICATE 赋值目标列，使用小写名保持 Go CIStr.L 的比较规则。
//     fn build_on_duplicate_update_columns(&self) -> HashSet<String> {
//         self.on_duplicate
//             .iter()
//             .map(|assignment| assignment.column_name.lower.clone())
//             .collect()
//     }
//
//     fn build_on_replace_referred_fk_triggers(
//         &self,
//         ctx: PlanContextRef,
//         info_schema: &dyn InfoSchema,
//         db_name: &str,
//         table_info: &TableInfo,
//     ) -> Result<(Vec<FKCheckRef>, Vec<FKCascadeRef>), PlannerError> {
//         let referred_fks = info_schema
//             .get_table_referred_foreign_keys(db_name, &table_info.name.lower);
//         let mut checks = Vec::with_capacity(referred_fks.len());
//         let mut cascades = Vec::with_capacity(referred_fks.len());
//         for referred_fk in referred_fks {
//             let (check, cascade) = build_on_delete_or_update_fk_trigger(
//                 ctx.clone(),
//                 info_schema,
//                 referred_fk,
//                 FKCascadeType::OnDelete,
//             )?;
//             checks.extend(check);
//             cascades.extend(cascade);
//         }
//         Ok((checks, cascades))
//     }
// }
//
// impl Update {
/// BuildOnUpdateFKTriggers 按表构造父表引用触发器和子表存在性检查。
//     pub fn build_on_update_fk_triggers(
//         &mut self,
//         ctx: PlanContextRef,
//         info_schema: &dyn InfoSchema,
//         table_by_id: &HashMap<i64, TableRef>,
//     ) -> Result<(), PlannerError> {
//         if !ctx.session_vars().foreign_key_checks {
//             return Ok(());
//         }
//
//         let update_columns_by_table = self.build_table_to_update_columns();
//         let mut checks: HashMap<i64, Vec<FKCheckRef>> = HashMap::new();
//         let mut cascades: HashMap<i64, Vec<FKCascadeRef>> = HashMap::new();
//         for (table_id, table) in table_by_id {
//             let table_info = table.meta();
// 理论上执行到这里表一定属于某个 schema；显式检查避免原 Go 的 panic。
//             let db_info = schema_by_table(info_schema, table_info)
//                 .ok_or_else(database_not_exists_error)?;
//             let Some(update_columns) = update_columns_by_table.get(table_id) else {
//                 continue;
//             };
//             if update_columns.is_empty() {
//                 continue;
//             }
//
//             let (referred_checks, referred_cascades) = build_on_update_referred_fk_triggers(
//                 ctx.clone(),
//                 info_schema,
//                 &db_info.name.lower,
//                 table_info,
//                 update_columns,
//             )?;
//             checks.entry(*table_id).or_default().extend(referred_checks);
//             cascades
//                 .entry(*table_id)
//                 .or_default()
//                 .extend(referred_cascades);
//
//             let child_checks = build_on_update_child_fk_checks(
//                 ctx.clone(),
//                 info_schema,
//                 &db_info.name.lower,
//                 table_info,
//                 update_columns,
//             )?;
//             checks.entry(*table_id).or_default().extend(child_checks);
//         }
//
//         if !checks.is_empty() || !cascades.is_empty() {
//             self.fk_checks = checks;
//             self.fk_cascades = cascades;
//         }
//         Ok(())
//     }
//
/// buildTbl2UpdateColumns 把 assignment 的 Schema 下标还原为表列，并传播到存储生成列。
//     fn build_table_to_update_columns(&self) -> HashMap<i64, HashSet<String>> {
//         let columns = get_update_columns_info(
//             &self.table_by_id,
//             &self.table_column_positions,
//             self.select_plan.schema().columns.len(),
//         );
//         let mut result: HashMap<i64, HashSet<String>> = HashMap::new();
//
//         for assignment in &self.ordered_list {
//             let column = columns[assignment.column.index]
//                 .as_ref()
//                 .expect("update assignment must map to a writable column");
//             for position in &self.table_column_positions {
//                 if assignment.column.index >= position.start
//                     && assignment.column.index < position.end
//                 {
//                     result
//                         .entry(position.table_id)
//                         .or_default()
//                         .insert(column.name.lower.clone());
//                     break;
//                 }
//             }
//         }
//
//         for (table_id, table) in &self.table_by_id {
//             let Some(updated) = result.get_mut(table_id) else {
//                 continue;
//             };
//             for column in table.writable_columns() {
//                 if !column.is_generated() || !column.generated_stored {
//                     continue;
//                 }
// 任一依赖列被更新时，存储生成列也属于实际更新列。
//                 if column.dependencies.keys().any(|name| updated.contains(name)) {
//                     updated.insert(column.name.lower.clone());
//                 }
//             }
//         }
//         result
//     }
// }
//
// impl Delete {
/// BuildOnDeleteFKTriggers 为每张被删表的所有引用外键生成限制检查或级联动作。
//     pub fn build_on_delete_fk_triggers(
//         &mut self,
//         ctx: PlanContextRef,
//         info_schema: &dyn InfoSchema,
//         table_by_id: &HashMap<i64, TableRef>,
//     ) -> Result<(), PlannerError> {
//         if !ctx.session_vars().foreign_key_checks {
//             return Ok(());
//         }
//
//         let mut checks: HashMap<i64, Vec<FKCheckRef>> = HashMap::new();
//         let mut cascades: HashMap<i64, Vec<FKCascadeRef>> = HashMap::new();
//         for (table_id, table) in table_by_id {
//             let table_info = table.meta();
//             let db_info = schema_by_table(info_schema, table_info)
//                 .ok_or_else(database_not_exists_error)?;
//             let referred_fks = info_schema.get_table_referred_foreign_keys(
//                 &db_info.name.lower,
//                 &table_info.name.lower,
//             );
//             for referred_fk in referred_fks {
//                 let (check, cascade) = build_on_delete_or_update_fk_trigger(
//                     ctx.clone(),
//                     info_schema,
//                     referred_fk,
//                     FKCascadeType::OnDelete,
//                 )?;
//                 checks.entry(*table_id).or_default().extend(check);
//                 cascades.entry(*table_id).or_default().extend(cascade);
//             }
//         }
//         if !checks.is_empty() || !cascades.is_empty() {
//             self.fk_checks = checks;
//             self.fk_cascades = cascades;
//         }
//         Ok(())
//     }
// }
//
// fn build_on_update_referred_fk_triggers(
//     ctx: PlanContextRef,
//     info_schema: &dyn InfoSchema,
//     db_name: &str,
//     table_info: &TableInfo,
//     update_columns: &HashSet<String>,
// ) -> Result<(Vec<FKCheckRef>, Vec<FKCascadeRef>), PlannerError> {
//     let referred_fks =
//         info_schema.get_table_referred_foreign_keys(db_name, &table_info.name.lower);
//     let mut checks = Vec::with_capacity(referred_fks.len());
//     let mut cascades = Vec::with_capacity(referred_fks.len());
//     for referred_fk in referred_fks {
//         if !map_contains_any_columns(update_columns, &referred_fk.columns) {
//             continue;
//         }
//         let (check, cascade) = build_on_delete_or_update_fk_trigger(
//             ctx.clone(),
//             info_schema,
//             referred_fk,
//             FKCascadeType::OnUpdate,
//         )?;
//         checks.extend(check);
//         cascades.extend(cascade);
//     }
//     Ok((checks, cascades))
// }
//
// fn build_on_update_child_fk_checks(
//     ctx: PlanContextRef,
//     info_schema: &dyn InfoSchema,
//     db_name: &str,
//     table_info: &TableInfo,
//     update_columns: &HashSet<String>,
// ) -> Result<Vec<FKCheckRef>, PlannerError> {
//     let mut checks = Vec::with_capacity(table_info.foreign_keys.len());
//     for fk in &table_info.foreign_keys {
//         if fk.version < 1 || !map_contains_any_columns(update_columns, &fk.columns) {
//             continue;
//         }
//         let failed_error = err_no_referenced_row2(fk.to_string(db_name, &table_info.name.lower));
//         if let Some(check) = build_fk_check_on_modify_child_table(
//             ctx.clone(),
//             info_schema,
//             fk.clone(),
//             failed_error,
//         )? {
//             checks.push(check);
//         }
//     }
//     Ok(checks)
// }
//
/// GetUpdateColumnsInfo 按 TblColPosInfo 描述把各表 writable columns 放回联合 Schema 位置。
// pub fn get_update_columns_info(
//     table_by_id: &HashMap<i64, TableRef>,
//     positions: &[TableColumnPosition],
//     size: usize,
// ) -> Vec<Option<ColumnRef>> {
//     let mut columns = vec![None; size];
//     for position in positions {
//         let table = &table_by_id[&position.table_id];
//         for (offset, column) in table.writable_columns().iter().enumerate() {
//             columns[position.start + offset] = Some(column.clone());
//         }
//     }
//     columns
// }
//
// fn build_on_delete_or_update_fk_trigger(
//     ctx: PlanContextRef,
//     info_schema: &dyn InfoSchema,
//     referred_fk: ReferredFKInfoRef,
//     cascade_type: FKCascadeType,
// ) -> Result<(Option<FKCheckRef>, Option<FKCascadeRef>), PlannerError> {
// Go 使用 context.Background 查子表；查不到表时静默跳过，兼容陈旧引用元数据。
//     let Ok(child_table) = info_schema.table_by_name(
//         BackgroundContext,
//         &referred_fk.child_schema,
//         &referred_fk.child_table,
//     ) else {
//         return Ok((None, None));
//     };
//     let Some(fk) = find_fk_info_by_name(
//         &child_table.meta().foreign_keys,
//         &referred_fk.child_fk_name.lower,
//     ) else {
//         return Ok((None, None));
//     };
//     if fk.version < 1 {
//         return Ok((None, None));
//     }
//     check_table_mode_is_normal(&child_table.meta().name, child_table.meta().mode)?;
//
//     let refer_option = if fk.state != SchemaState::Public {
// 非 public 外键统一按 RESTRICT 处理，不能提前执行级联。
//         ReferOptionType::Restrict
//     } else {
//         match cascade_type {
//             FKCascadeType::OnDelete => ReferOptionType::from(fk.on_delete),
//             FKCascadeType::OnUpdate => ReferOptionType::from(fk.on_update),
//         }
//     };
//
//     match refer_option {
//         ReferOptionType::Cascade | ReferOptionType::SetNull => Ok((
//             None,
//             Some(build_fk_cascade(
//                 ctx,
//                 cascade_type,
//                 referred_fk,
//                 child_table,
//                 fk,
//             )?),
//         )),
//         _ => Ok((
//             Some(build_fk_check_for_referred_fk(
//                 ctx,
//                 child_table,
//                 fk,
//                 referred_fk,
//             )?),
//             None,
//         )),
//     }
// }
//
// fn map_contains_any_columns(columns: &HashSet<String>, candidates: &[CIStr]) -> bool {
//     candidates
//         .iter()
//         .any(|column| columns.contains(&column.lower))
// }
//
// fn build_fk_check_on_modify_child_table(
//     ctx: PlanContextRef,
//     info_schema: &dyn InfoSchema,
//     fk: FKInfoRef,
//     failed_error: PlannerError,
// ) -> Result<Option<FKCheckRef>, PlannerError> {
// 被引用表缺失时 Go 静默跳过，让其它 DDL/元数据路径处理该状态。
//     let Ok(referenced_table) = info_schema.table_by_name(
//         BackgroundContext,
//         &fk.ref_schema,
//         &fk.ref_table,
//     ) else {
//         return Ok(None);
//     };
//     let mut check = build_fk_check(ctx, referenced_table, &fk.ref_columns, failed_error)?;
//     check.make_mut().check_exist = true;
//     check.make_mut().fk = Some(fk);
//     Ok(Some(check))
// }
//
// fn build_fk_check_for_referred_fk(
//     ctx: PlanContextRef,
//     child_table: TableRef,
//     fk: FKInfoRef,
//     referred_fk: ReferredFKInfoRef,
// ) -> Result<FKCheckRef, PlannerError> {
//     let failed_error = err_row_is_referenced2(fk.to_string(
//         &referred_fk.child_schema.lower,
//         &referred_fk.child_table.lower,
//     ));
//     let mut check = build_fk_check(ctx, child_table, &fk.columns, failed_error)?;
//     check.make_mut().check_exist = false;
//     check.make_mut().referred_fk = Some(referred_fk);
//     Ok(check)
// }
//
// fn build_fk_check(
//     ctx: PlanContextRef,
//     table: TableRef,
//     columns: &[CIStr],
//     failed_error: PlannerError,
// ) -> Result<FKCheckRef, PlannerError> {
//     let table_info = table.meta();
//     if table_info.pk_is_handle && columns.len() == 1 {
//         if let Some(column) = find_column_info(&table_info.columns, &columns[0].lower) {
//             if has_primary_key_flag(column.flag()) {
//                 return Ok(FKCheck {
//                     table,
//                     index_is_primary_key: true,
//                     index_is_exclusive: true,
//                     failed_error,
//                     ..FKCheck::default()
//                 }
//                 .init(ctx));
//             }
//         }
//     }
//
//     let Some(index_info) = find_index_by_columns_for_foreign_key(
//         table_info,
//         &table_info.indices,
//         columns,
//     ) else {
//         return Err(failed_error);
//     };
//     let Some(table_index) = table
//         .indices()
//         .iter()
//         .find(|index| index.meta().id == index_info.id)
//         .cloned()
//     else {
//         return Err(failed_error);
//     };
//
//     Ok(FKCheck {
//         table,
//         index: Some(table_index),
//         index_is_exclusive: columns.len() == index_info.columns.len(),
//         index_is_primary_key: index_info.primary && table_info.is_common_handle,
//         failed_error,
//         ..FKCheck::default()
//     }
//     .init(ctx))
// }
//
// fn build_fk_cascade(
//     ctx: PlanContextRef,
//     cascade_type: FKCascadeType,
//     referred_fk: ReferredFKInfoRef,
//     child_table: TableRef,
//     fk: FKInfoRef,
// ) -> Result<FKCascadeRef, PlannerError> {
//     let child_columns = &child_table.meta().columns;
//     let mut columns = Vec::with_capacity(fk.columns.len());
//     for name in &fk.columns {
//         let column = find_column_info(child_columns, &name.lower).ok_or_else(|| {
//             PlannerError::message(format!(
//                 "foreign key column {} is not found in table {}",
//                 name.lower,
//                 child_table.meta().name
//             ))
//         })?;
//         columns.push(column);
//     }
//
//     let mut cascade = FKCascade {
//         cascade_type,
//         referred_fk,
//         child_table: child_table.clone(),
//         fk: fk.clone(),
//         fk_columns: columns.clone(),
//         ..FKCascade::default()
//     }
//     .init(ctx);
//
//     if child_table.meta().pk_is_handle && columns.len() == 1 {
//         let column = find_column_info(child_columns, &columns[0].name.lower);
//         if column.is_some_and(|column| has_primary_key_flag(column.flag())) {
//             return Ok(cascade);
//         }
//     }
//
//     let index = find_index_by_columns_for_foreign_key(
//         child_table.meta(),
//         &child_table.meta().indices,
//         &fk.columns,
//     )
//     .ok_or_else(|| {
//         PlannerError::message(format!(
//             "Missing index for '{}' foreign key columns in the table '{}'",
//             fk.name,
//             child_table.meta().name
//         ))
//     })?;
//     cascade.make_mut().fk_index = Some(index);
//     Ok(cascade)
// }
// */
use std::collections::{BTreeMap, BTreeSet};

/// 外键引用动作：RESTRICT/NO ACTION 走检查，CASCADE/SET NULL 走级联。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReferentialAction {
    #[default]
    Restrict,
    NoAction,
    Cascade,
    SetNull,
    SetDefault,
}
impl ReferentialAction {
    fn as_sql(self) -> &'static str {
        match self {
            Self::Restrict => "RESTRICT",
            Self::NoAction => "NO ACTION",
            Self::Cascade => "CASCADE",
            Self::SetNull => "SET NULL",
            Self::SetDefault => "SET DEFAULT",
        }
    }
}
/// 级联触发来源：删除父行或更新父行被引用列。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CascadeType {
    OnDelete,
    OnUpdate,
}
/// 简化外键元数据：子/父表、列映射与引用动作。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ForeignKeyInfo {
    pub name: String,
    pub child_table_id: i64,
    pub parent_table_id: i64,
    pub child_columns: Vec<String>,
    pub parent_columns: Vec<String>,
    pub on_delete: ReferentialAction,
    pub on_update: ReferentialAction,
    /// 对应 Go `FKInfo.State == StatePublic`；非 public 外键强制按 RESTRICT 处理。
    pub public: bool,
    pub enabled: bool,
}
/// 简化表元数据：列、本表外键与被引用外键列表。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableInfo {
    pub id: i64,
    pub name: String,
    pub columns: Vec<String>,
    pub foreign_keys: Vec<ForeignKeyInfo>,
    pub referred_foreign_keys: Vec<ForeignKeyInfo>,
}

/// 外键存在性检查计划节点的规划期描述。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FkCheck {
    pub name: String,
    pub table_id: i64,
    pub columns: Vec<String>,
    /// 与 Go `FKCheck.CheckExist` 一致：检查目标行应存在还是不应存在。
    pub check_exist: bool,
    pub failed_error: String,
}
impl FkCheck {
    /// EXPLAIN 访问对象：被检查的表 ID。
    pub fn access_object(&self) -> String {
        format!("table:{}", self.table_id)
    }
    /// EXPLAIN 算子信息：外键名与检查方向。
    pub fn operator_info(&self) -> String {
        format!(
            "foreign_key:{}, {}",
            self.name,
            if self.check_exist {
                "check_exist"
            } else {
                "check_not_exist"
            }
        )
    }
    /// 保持 Go 口径：浅层结构大小加上检查列的动态字符串占用。
    pub fn memory_usage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64
            + self
                .columns
                .iter()
                .map(|v| v.capacity() as i64)
                .sum::<i64>()
    }
}

/// ON DELETE/UPDATE 级联动作的规划期描述。
#[derive(Clone, Debug, PartialEq)]
pub struct FkCascade {
    pub name: String,
    pub child_table_id: i64,
    pub child_columns: Vec<String>,
    pub parent_columns: Vec<String>,
    pub cascade_type: CascadeType,
    pub action: ReferentialAction,
}
impl FkCascade {
    /// EXPLAIN 访问对象：级联作用的子表。
    pub fn access_object(&self) -> String {
        format!("table:{}", self.child_table_id)
    }
    /// EXPLAIN 算子信息：外键名、级联类型与动作。
    pub fn operator_info(&self) -> String {
        let operation = match self.cascade_type {
            CascadeType::OnDelete => "on_delete",
            CascadeType::OnUpdate => "on_update",
        };
        format!(
            "foreign_key:{}, {}:{}",
            self.name,
            operation,
            self.action.as_sql()
        )
    }
    /// 保持 Go 口径：当前只计算级联结构体的浅层大小。
    pub fn memory_usage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64
    }
}

/// 为 INSERT/REPLACE 构造子表存在性检查；REPLACE 时额外处理被引用外键。
pub fn build_on_insert_fk_triggers(
    table: &TableInfo,
    replacing: bool,
) -> Result<(Vec<FkCheck>, Vec<FkCascade>), String> {
    let mut checks = table
        .foreign_keys
        .iter()
        .filter(|fk| fk.enabled)
        .map(build_child_check)
        .collect::<Vec<_>>();
    let mut cascades = Vec::new();
    // REPLACE 会先删冲突行，按 ON DELETE 规则处理父表被引用外键。
    if replacing {
        for fk in table.referred_foreign_keys.iter().filter(|fk| fk.enabled) {
            match build_referred_trigger(fk, CascadeType::OnDelete)? {
                Trigger::Check(v) => checks.push(v),
                Trigger::Cascade(v) => cascades.push(v),
            }
        }
    }
    Ok((checks, cascades))
}
/// 为 UPDATE 构造：父表被引用列变更触发器 + 子表外键列变更的存在性检查。
pub fn build_on_update_fk_triggers(
    table: &TableInfo,
    updated: &BTreeSet<String>,
) -> Result<(Vec<FkCheck>, Vec<FkCascade>), String> {
    let mut checks = Vec::new();
    let mut cascades = Vec::new();
    for fk in table
        .referred_foreign_keys
        .iter()
        .filter(|fk| fk.enabled && contains_any(updated, &fk.parent_columns))
    {
        match build_referred_trigger(fk, CascadeType::OnUpdate)? {
            Trigger::Check(v) => checks.push(v),
            Trigger::Cascade(v) => cascades.push(v),
        }
    }
    checks.extend(
        table
            .foreign_keys
            .iter()
            .filter(|fk| fk.enabled && contains_any(updated, &fk.child_columns))
            .map(build_child_check),
    );
    Ok((checks, cascades))
}
/// 为 DELETE 构造所有被引用外键的限制检查或级联动作。
pub fn build_on_delete_fk_triggers(
    table: &TableInfo,
) -> Result<(Vec<FkCheck>, Vec<FkCascade>), String> {
    let mut checks = Vec::new();
    let mut cascades = Vec::new();
    for fk in table.referred_foreign_keys.iter().filter(|fk| fk.enabled) {
        match build_referred_trigger(fk, CascadeType::OnDelete)? {
            Trigger::Check(v) => checks.push(v),
            Trigger::Cascade(v) => cascades.push(v),
        }
    }
    Ok((checks, cascades))
}
/// 内部统一表示一次触发器产物。
enum Trigger {
    Check(FkCheck),
    Cascade(FkCascade),
}
/// 按引用动作把被引用外键展开为 Check 或 Cascade；其余动作沿用 Go 的限制检查路径。
fn build_referred_trigger(fk: &ForeignKeyInfo, kind: CascadeType) -> Result<Trigger, String> {
    let configured_action = if kind == CascadeType::OnDelete {
        fk.on_delete
    } else {
        fk.on_update
    };
    let action = if fk.public {
        configured_action
    } else {
        ReferentialAction::Restrict
    };
    match action {
        ReferentialAction::Restrict
        | ReferentialAction::NoAction
        | ReferentialAction::SetDefault => Ok(Trigger::Check(FkCheck {
            name: fk.name.clone(),
            table_id: fk.child_table_id,
            columns: fk.child_columns.clone(),
            check_exist: false,
            failed_error: format!("foreign key {} is referenced", fk.name),
        })),
        ReferentialAction::Cascade | ReferentialAction::SetNull => {
            Ok(Trigger::Cascade(FkCascade {
                name: fk.name.clone(),
                child_table_id: fk.child_table_id,
                child_columns: fk.child_columns.clone(),
                parent_columns: fk.parent_columns.clone(),
                cascade_type: kind,
                action,
            }))
        }
    }
}
/// 构造子表侧存在性检查：校验父表中是否存在对应引用行。
fn build_child_check(fk: &ForeignKeyInfo) -> FkCheck {
    FkCheck {
        name: fk.name.clone(),
        table_id: fk.parent_table_id,
        columns: fk.parent_columns.clone(),
        check_exist: true,
        failed_error: format!("foreign key {} fails", fk.name),
    }
}
/// 忽略大小写判断更新列集合是否与候选列有交集。
fn contains_any(columns: &BTreeSet<String>, candidates: &[String]) -> bool {
    candidates.iter().any(|candidate| {
        columns
            .iter()
            .any(|column| column.eq_ignore_ascii_case(candidate))
    })
}
/// 按表列位置描述，把各表列名填回联合 Schema 对应下标。
pub fn get_update_columns_info(
    tables: &BTreeMap<i64, Vec<String>>,
    positions: &[(i64, usize, usize)],
    size: usize,
) -> Vec<Option<String>> {
    let mut result = vec![None; size];
    for (table_id, start, end) in positions {
        if let Some(columns) = tables.get(table_id) {
            for (offset, column) in columns.iter().enumerate().take(end.saturating_sub(*start)) {
                if start + offset < size {
                    result[start + offset] = Some(column.clone());
                }
            }
        }
    }
    result
}
