# `pkg/table/mview_log.rs`

## 文件定位

[`mview_log.rs`](mview_log.rs) 属于 `astersql-table` crate；[`lib.rs`](lib.rs) 通过 `pub mod mview_log` 装配该模块，并用 `pub use mview_log::*` 再导出公共 API。它定义物化视图日志（MLog）的表级契约：校验基础表与日志表元数据、把基础行投影成日志行，并提供一个实现 `Table` 的语句级包装器，使基础表写入成功后在同一个 `Transaction` 中同步追加日志记录。

当前 Rust 应用主链需要区分两种接线。`WrapTableWithMaterializedViewLog`/`MLogTable` 是完整的 `Table` 包装实现，但仓库中的非测试 Rust 调用者没有直接构造它；实际关系型 DML 路径由 [`pkg/session/runtime/mlog.rs`](../session/runtime/mlog.rs) 的 `RuntimeMLog` 调用本文件的 `validate_meta` 和 `MLogDMLType`，再由 [`pkg/session/runtime/dml.rs`](../session/runtime/dml.rs) 将基础表与 MLog 的 KV mutation 一起提交。包装器行为由 [`mview_log_test.rs`](mview_log_test.rs) 和 [`go_merge_49_test.rs`](go_merge_49_test.rs) 独立验证。

## 核心职责

- `MLogSourceStmt` 保存发起写入的语句类别（`Insert`、`Update`、`Delete`、`Replace`、`LoadData`），与最终写入 `_MLOG$_DML_TYPE` 的逻辑变化类型分离。
- `MLogDMLType` 及 `as_str` 把逻辑变化编码为日志协议值 `I`、`U`、`D`。
- `validate_meta` 在包装或运行时写日志前一次性验证基础表与日志表的双向 ID、跟踪列顺序、两个尾部系统列及基础表列偏移，防止静默记录错误列值。
- `project_log_row` 按缓存的基础表 offset 提取跟踪列，并追加 DML 类型与 old/new 标记；旧行标记为 `-1`，新行标记为 `1`。
- `MLogTable` 代理只读表接口，并覆写 `AddRecord`、`UpdateRecord`、`RemoveRecord`，在基础表变更成功后同步写 MLog。
- `write_log_row` 对日志写入使用跳过重复键检查的选项，透传悲观事务 lazy duplicate-check 模式和可选 KV context，同时隔离并恢复基础表的预留 Row ID 分配区间。

## 主要符号

- `pub enum MLogSourceStmt`：语句来源枚举。它影响插入/冲突删除应被记作 `Insert`、`Update` 还是 `Delete`，不直接作为日志列值。
- `pub enum MLogDMLType` 与 `pub fn as_str(self) -> &'static str`：日志变化类型及稳定的一字符编码。
- `pub fn classify_add_record(source, is_update, removed_conflict) -> MLogDMLType`：`Update` 来源固定产生 `U`；`Insert`/`Replace`/`LoadData` 在 `IsUpdate` 或先前发生冲突删除时产生 `U`，否则产生 `I`。
- `pub fn validate_meta(base, mlog) -> TableResult<Vec<usize>>`：公共元数据校验器，成功时按 `MaterializedViewLog.Columns` 的声明顺序返回基础表列 offset。
- `pub struct MLogTable`：持有 `base`、`mlog` 两个 `Box<dyn Table>`、语句来源、已校验的 `tracked_offsets`，以及受 `Mutex<bool>` 保护的冲突删除标记。
- `pub fn WrapTableWithMaterializedViewLog(...) -> TableResult<Box<dyn Table>>`：调用 `validate_meta` 后创建包装器；返回 trait object，调用方仍按普通 `Table` 使用。
- `pub(crate) fn should_log_update(offsets, touched) -> bool`：仅在某个跟踪列被触碰时记录普通更新；`touched` 缺少某 offset 时保守返回 `true`。
- `pub(crate) fn project_log_row(...)`：构造一条独立的日志行；offset 越界时返回错误，而不是截断或补空值。
- `MLogTable::write_log_row`：内部日志写入边界，负责选项转换、Row ID allocator 暂存/清零/恢复和 `mlog.AddRecord`。
- `impl columnAPI for MLogTable`、`impl Table for MLogTable`：除三个 DML 方法外，列、索引、约束、前缀、allocator、元数据、collation、表类型和分区接口均委托给 `base`。

## 执行流程

1. 创建包装器时，`WrapTableWithMaterializedViewLog` 调用 `validate_meta(base.Meta(), mlog.Meta())`。校验成功后保存跟踪列 offset；任一元数据不变量不满足都不会产生包装器。
2. `AddRecord` 先解析 `NewAddRecordOpt`，再锁住 `removed_conflict`，读取并立即清除上一次冲突删除标记。随后先调用 `base.AddRecord`；只有基础写成功才通过 `classify_add_record` 决定 `I`/`U` 并写一条 marker=`1` 的日志行。这样失败的基础插入不会追加日志，且冲突标记不会泄漏给后一行。
3. `UpdateRecord` 先更新基础表。如果 `should_log_update` 判定所有跟踪列均未触碰，则直接成功；否则按同一组选项依次写旧行（`U,-1`）和新行（`U,1`）。handle 变化的更新通常走 `RemoveRecord` + 带 `IsUpdate` 的 `AddRecord`，因此不依赖 `touched`，采取保守记录。
4. `RemoveRecord` 先删除基础行。对 `Insert`、`Replace`、`LoadData` 来源，它把 `removed_conflict` 置为 `true`，让随后的 `AddRecord` 将新行分类成更新；`Delete` 来源写 `D,-1`，其余来源写 `U,-1`。
5. `write_log_row` 调用 `project_log_row`，临时保存并清零 `MutateContext::GetReservedRowIDAlloc` 暴露的区间，再以相同 `Transaction` 调用 `mlog.AddRecord`，最后无论日志写入成功与否都恢复已保存区间并传播结果。
6. 当前应用主链的另一实现位于 `RuntimeMLog`：`for_table` 用 `validate_meta` 得到 offset，`tracked_changed` 判断跟踪值是否变化，`append` 用 `MLogDMLType::as_str` 组装日志行；`dml.rs` 把这些日志 mutation 与基础表 mutation 放入同一个批次。

## 数据与状态

- `tracked_offsets: Vec<usize>` 是包装时从日志元数据列名映射到基础表 `ColumnInfo.Offset` 的快照；顺序必须与日志表公开跟踪列顺序一致。
- 一条日志行的布局为“所有跟踪列值 + `_MLOG$_DML_TYPE` + `_MLOG$_OLD_NEW`”。`project_log_row` 对 `Datum` 做 `clone`，形成新的 `Vec<Datum>`，避免日志表 `AddRecord` 改写调用方基础行的槽位。
- old/new 标记不是枚举：本文件在 DML 路径直接使用 `-1`（旧像）和 `1`（新像），与 Go 的 `mlogOldRowMarker`/`mlogNewRowMarker` 相同。
- `removed_conflict` 是语句级瞬时状态，只连接一次冲突删除及其后的新增。`AddRecord` 消费时先清零；普通 `Delete` 虽写旧行，但不会设置该标记。
- `base` 和 `mlog` 的生命周期由包装器独占；对外返回 `Box<dyn Table>`。列、索引与元数据查询仍呈现基础表，不暴露日志表为被操作目标。

## 依赖与调用关系

- crate 边界由 [`Cargo.toml`](Cargo.toml) 确认：模块直接依赖 `astersql-meta-autoid`（`Allocators`）、`astersql-errors`（`New`）、`astersql-kv`（`Handle`、`Key`、`Transaction`）、`astersql-meta-model`（表/MLog 元数据）、`astersql-table-tblctx`（allocator context）和 `astersql-types`（`Datum`）。其余表契约来自本 crate 的 `Table`、option、column/index/constraint 类型。
- RustCodeGraph 给出的内部调用边包括 `WrapTableWithMaterializedViewLog -> validate_meta`、`MLogTable::write_log_row -> project_log_row`、`MLogTable::AddRecord -> classify_add_record`；`write_log_row` 下游最终是 `mlog.AddRecord`。
- 非测试 Rust 上游是 [`pkg/session/runtime/mlog.rs`](../session/runtime/mlog.rs)：`RuntimeMLog::for_table -> validate_meta`，`RuntimeMLog::append -> MLogDMLType::as_str`。[`pkg/session/runtime/dml.rs`](../session/runtime/dml.rs) 在 INSERT/REPLACE/UPDATE/DELETE 及外键级联路径调用 `RuntimeMLog::for_table`/`append`。
- `WrapTableWithMaterializedViewLog` 的仓库内 Rust 调用当前只见于 [`mview_log_test.rs`](mview_log_test.rs)；因此不能把 Go 执行器直接使用包装表的事实等同为当前 Rust 生产接线。
- Go 对照实现是 [`tables/mview_log.go`](tables/mview_log.go)，其 `mlogTable.writeMLogRow`、三个 DML 覆写和 metadata 校验与本文件一一对应。

## 错误处理与边界

- `validate_meta` 拒绝：基础表没有非零 MLog ID、MLog ID 不匹配、日志表缺少 `MaterializedViewLog`、反向 `BaseTableID` 不匹配、公开列数量不等于“跟踪列数 + 2”、跟踪列顺序错误、尾部系统列错误、基础表缺少跟踪列或 offset 不能转为 `usize`。
- `public.len() == log_info.Columns.len() + 2` 保证随后访问 `public[n-2]`/`public[n-1]` 安全；即使没有跟踪列，仍要求两个系统列。
- `should_log_update` 对越界/过短的 `touched` 采用“必须记录”的保守策略，优先避免漏日志。
- `project_log_row` 对基础行过短返回包含 offset 与行长的错误。它不检查 marker 是否只能为 `-1/1`，调用者负责该协议。
- 三个 DML 方法都先执行基础表变更；基础变更错误会直接返回且不追加日志。日志写入错误也直接传播。由于两者共享同一个 `Transaction`，原子回滚由上层事务负责；方法本身不尝试撤销已经写入 transaction buffer 的基础 mutation。
- `removed_conflict` 的 poisoned mutex 被转换为 `TableResult` 错误。`write_log_row` 在 `mlog.AddRecord` 返回后再恢复 Row ID allocator，因此错误路径同样恢复；若 allocator 的实现自身 panic，则不属于返回值错误保证。
- Rust 参数是非空的 `Box<dyn Table>` 和 `&TableInfo`，因此不需要 Go 版本针对 nil table/meta 的显式检查；这是类型系统边界差异，不是遗漏的同类分支。

## 并发与资源生命周期

`MLogTable` 被设计为语句级对象：Go 注释明确禁止跨语句/会话复用，Rust 模块级说明同样称其为 statement scoped。Rust 使用 `Mutex<bool>` 而非 Go 的普通 `bool`，使通过共享 `&self` 的 `Table` DML 方法可以安全修改冲突标记；这只保护该标记，不把整个“Remove 后 Add”序列变成跨线程原子操作，因此调用方仍应遵守语句级、顺序 DML 生命周期。

基础写和日志写接收同一个 `&mut dyn Transaction`。[`mview_log_test.rs`](mview_log_test.rs) 记录 transaction 地址并验证所有事件相同，还验证 `Rollback` 同时清空基础表与日志表的 pending writes。日志 Row ID 分配期间，`write_log_row` 暂存基础表 allocator 的 `(base,maxv)`，把区间清零供日志表使用，调用结束后恢复，避免两个表共用单表预留区间。函数不启动线程、不持有异步任务或通道；`Arc` 仅用于代理 `Column`、`Index`、`Constraint` 返回值。

## 与 Go 版本的对应关系

Rust [`mview_log.rs`](mview_log.rs) 对照 Go [`tables/mview_log.go`](tables/mview_log.go)：`MLogSourceStmt`/`MLogDMLType`、metadata 校验、跟踪 offset、三个 DML 覆写、跳过未跟踪列更新、old/new 双行、冲突删除分类、duplicate-check option 以及 Row ID allocator 隔离均保持同一语义。

可见差异如下：

- Go 以嵌入 `table.Table` 自动提升非 DML 方法；Rust 显式实现 `columnAPI` 和 `Table` 并逐项委托。
- Go 的 `validateMLogMetaColumn` 与包装函数分开构建名字到 offset 的 map；Rust 的 `validate_meta` 合并这些步骤并直接返回 `Vec<usize>`，同时显式拒绝负 offset。
- Go 的 `removedConflict bool` 依赖语句级顺序使用；Rust 为满足共享引用接口使用 `Mutex<bool>`，并把锁中毒变为错误。
- Go 包装函数直接被传统 table executor 形态使用；当前 Rust session runtime 走 `RuntimeMLog` 的 KV mutation 路径，只复用本文件的 metadata/DML 公共契约。包装器存在且有测试，但尚未发现非测试 Rust 构造点。
- Go 用 `defer` 恢复 allocator；Rust 在 `mlog.AddRecord` 返回后显式恢复并再返回结果，对普通 `Err` 路径等价，但不提供 panic unwind guard。

## 扩展指南

- 新增语句来源或改变冲突分类时，先修改 `MLogSourceStmt` 与 `classify_add_record`，同步审查 `AddRecord`/`RemoveRecord` 的标记规则，并扩展 [`go_merge_49_test.rs`](go_merge_49_test.rs) 的分类矩阵及 [`mview_log_test.rs`](mview_log_test.rs) 的事件序列。
- 改变日志行布局、系统列或 marker 协议时，必须同步修改 `validate_meta`、`project_log_row`、`MLogTable::write_log_row` 和 [`pkg/session/runtime/mlog.rs`](../session/runtime/mlog.rs) 的 `RuntimeMLog::append`，并与 [`tables/mview_log.go`](tables/mview_log.go) 及 meta-model 常量核对兼容性。已有日志表 schema/消费端依赖列顺序，属于持久化兼容风险。
- 改变“哪些 UPDATE 需要记录”时，同时审查 `should_log_update` 与 `RuntimeMLog::tracked_changed`。过度跳过会破坏物化视图增量刷新正确性；过度记录主要带来写放大和存储成本。
- 增加 `Table` trait 方法时，要在 `MLogTable` 中明确决定委托还是覆写，不能让包装器丢失基础表能力；相关测试应继续放在独立的 [`mview_log_test.rs`](mview_log_test.rs)，不要内嵌到生产文件。
- 若将 `MLogTable` 接入 Rust 生产 DML，需先确认语句级实例边界、同事务回滚、分区表语义以及并发调用约束，并避免与现有 `RuntimeMLog` 重复写日志。
- 优化 metadata 查找或行投影时，保持包装时一次校验、写路径按 offset 线性投影的特性；当前每条日志的主要额外成本是 `Datum` 克隆、日志表 `AddRecord` 和 UPDATE 的两条日志写入。

## 验证依据

- 源码与模块：[`mview_log.rs`](mview_log.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)。`pkg/table` 中不存在 `doc.go`，因此最近的模块契约来自 Rust `lib.rs` 与 Go 对照实现。
- Go 对照：[`tables/mview_log.go`](tables/mview_log.go)，核对了 source/DML 枚举、metadata 不变量、DML 顺序、marker、option 透传和 allocator 恢复。
- 独立 Rust 测试：[`mview_log_test.rs`](mview_log_test.rs) 验证基础/日志错误传播、同一 transaction、日志事件序列、跳过重复键检查、Row ID 区间恢复及共同 rollback；[`go_merge_49_test.rs`](go_merge_49_test.rs) 验证分类、metadata/offset、投影越界和未触碰更新跳过。
- 生产接线：[`pkg/session/runtime/mlog.rs`](../session/runtime/mlog.rs) 与 [`pkg/session/runtime/dml.rs`](../session/runtime/dml.rs)，证明当前 Rust 主链调用 `validate_meta`/`MLogDMLType` 并以 mutation 批次记录日志，而非直接构造包装器。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`explore` 确认 `WrapTableWithMaterializedViewLog -> validate_meta`、`validate_meta <- RuntimeMLog::for_table`、`project_log_row <- write_log_row`、`classify_add_record <- AddRecord`。精确 path-qualified `callers/callees` 返回空数组，故调用者集合又用仓库 `rg` 做了直接交叉核验。
- 未运行 Cargo 或代码测试：本任务只新增说明文档，按任务计划以事实检索与固定十一章节结构校验代替运行时测试。
