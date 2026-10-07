# `br/pkg/restore/ingestrec/ingest_recorder.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-br-pkg-restore-ingestrec`（`br/pkg/restore/ingestrec/Cargo.toml`），由同目录 `lib.rs` 以 `pub mod ingest_recorder` 挂载，并通过 `pub use ingest_recorder::*` 对 crate 使用者扁平再导出。它移植自 `br/pkg/restore/ingestrec/ingest_recorder.go`，负责保存“日志恢复后必须重建”的 ingest 模式索引元数据；原因是日志备份不包含 ingest 路径写出的索引 KV。

当前接线状态必须区分语言：RustCodeGraph 显示该 Rust 文件包含 13 个符号，仅被 Rust `parity_test.rs` 文件引用，未发现 Rust 生产调用边；Cargo 也只依赖本地 `astersql-errors`，而 TiDB 元数据、InfoSchema 和 DDL job 由 `model_stub.rs` 提供本 crate 内的移植接口。因此它是可独立测试的 Rust 库实现，但仓库中可验证的完整 BR 运行主链仍在 Go：`stream.SchemasReplace.tryRecordIngestIndex` 录制 job，`task.rangeFilterFromIngestRecorder` 重写表 ID，`LogClient.generateRepairIngestIndexSQLs` 更新并遍历记录，最终由 `RepairIngestIndex` 消费。

## 核心职责

`IngestRecorder` 承担两个阶段之间的状态桥接：元数据日志回放阶段只从完成的 DDL job 记录 `(table_id, index_id)` 与主键标志；恢复接近完成、最新 InfoSchema 可用时，再补齐生成修复 SQL 所需的库名、表名、列表示、索引快照和相关外键。分阶段是必要的，因为 `TryAddJob` 看到的 job 信息可能已经被后续 DDL 改写，源码明确选择在 `UpdateIndexInfo` 中使用最新 schema。

它还负责三类约束：只接收 ingest 重组且会生成新索引 KV 的完成 job；通过 `RewriteTableID` 将备份侧表 ID 映射到恢复侧 ID；在重建索引前收集可能依赖这些索引的外键。`Iterate` 与 `IterateForeignKeys` 是只读消费边界，具体 DROP/ADD SQL 不在本文件生成。

## 主要符号

- `IngestIndexInfo`：单个待修复索引的快照。`SchemaName`、`TableName`、`ColumnList`、`ColumnArgs`、`IndexInfo` 在更新阶段填充；`IsPrimary` 在录制 job 时确定；`Updated` 是能否被 `Iterate` 暴露的门闩。普通列在 `ColumnList` 中用 `%n`，名称依序进入 `ColumnArgs`；隐藏生成列直接以内嵌 `(expression)` 表示。
- `IngestRecorder`：核心容器。私有 `items` 的形状为 `HashMap<table_id, HashMap<index_id, IngestIndexInfo>>`；`foreignKeyRecordManager` 在首次成功完成 `UpdateIndexInfo` 后变为 `Some`，其 `pub(crate)` 可见性仅服务同 crate 测试辅助。
- 包级 `New()` 与 `IngestRecorder::New()`：都创建空 `items`，并保持外键管理器为 `None`。包级函数用于对齐 Go 的 `ingestrec.New()` 调用形状。
- `notIngestJob`、`notReorgTypeJob`、`notSynced`：三个内部过滤器。分别排除无 `ReorgMeta`/非 `ReorgType::Ingest`、非 `AddIndex`/`AddPrimaryKey`/`ModifyColumn`、以及未到终态的 job；子 job 的合法终态是 `Done`，主 job 要求 `Synced`。
- `TryAddJob(job, isSubJob)`：公开录制入口。AddIndex/AddPrimaryKey 解析 `GetFinishedModifyIndexArgs` 的 `IndexArgs`；ModifyColumn 解析 `GetFinishedModifyColumnArgs` 的 `NewIndexIDs`。新记录均为 `Updated=false`，AddPrimaryKey 才设置 `IsPrimary=true`。
- `RewriteTableID(rewriteFunc)`：逐表调用映射函数，接收 `(new_id, skip)`；`skip=true` 丢弃整表记录。它先构造新 map，全部成功后才替换原 map。
- `UpdateIndexInfo(ctx, infoSchema)`：把粗粒度 ID 记录解析成可消费快照，并借助 `NewForeignKeyRecordManagerForTables`、`RemoveForeignKeys`、`Merge` 计算要临时移除的外键。
- `Iterate(f)`：只遍历 `Updated=true` 的索引，回调参数为 `(table_id, index_id, &IngestIndexInfo)`。
- `IterateForeignKeys(f)`：外键管理器尚未初始化时直接成功；初始化后遍历去重后的 `fkRecordMap`。

## 执行流程

1. `TryAddJob` 对 `None` 直接成功；对非 ingest、非三类目标 action、或状态不符合主/子 job 规则的输入静默跳过。
2. 通过过滤后，它解析 finished args，把索引 ID 写入 `items[TableID]`。相同表/索引键再次出现会覆盖旧的未完成快照；此时尚无库表名和列信息。
3. 恢复侧获得表 ID 重写规则后调用 `RewriteTableID`。映射函数可以保留并换 ID、跳过整表，或返回错误；只有整轮成功才提交新 map。
4. 最新 InfoSchema 可用后调用 `UpdateIndexInfo`。每张已录制表先按 ID 查表：表已删除则跳过；表仍在但 DB 不存在则返回硬错误。随后为该表建立外键候选集。
5. 对表的每个现存索引：若未录制，则调用 `RemoveForeignKeys` 从候选集中排除仍由该正常索引支撑的外键；若已录制，则按索引列顺序生成 `ColumnList`/`ColumnArgs`，复制 `IndexInfo` 与库表名，并最后设置 `Updated=true`。
6. 每表剩余外键合并到最终管理器；所有表处理结束后写入 `foreignKeyRecordManager`。
7. 消费方应先通过 `IterateForeignKeys` 处理外键，再通过 `Iterate` 处理索引。Go 主链的 `generateRepairIngestIndexSQLs` 正是先生成外键恢复 SQL，再按 `IsPrimary`、唯一性和普通索引分支生成索引 ADD SQL；实际删除、重建或输出 SQL 文件由 `RepairIngestIndex` 负责。

## 数据与状态

核心状态机是“已发现但未更新”到“可消费”：`TryAddJob` 创建 `Updated=false` 的项，只有 `UpdateIndexInfo` 找到同 ID 的现存索引并完成所有字段填充后才置为 `true`；`Iterate` 永远隐藏未更新项。这使已删除的表、已消失/改 ID 的索引不会生成不完整 SQL。

`ColumnList` 与 `ColumnArgs` 必须同步解释：每个普通列增加一个 `%n` 和一个名称参数；指定前缀长度时占位符后追加 `(length)`；隐藏列不增加参数，而是把 `GeneratedExprString` 包在括号中。`IndexInfo` 保存完整快照，供上层读取名称、唯一性、条件表达式、索引类型等本文件未展开的属性。

`items` 与外键 map 都是 `HashMap`，迭代顺序没有稳定保证，测试也只断言集合内容和次数。若多个旧表 ID 被映射到同一个新表 ID，后插入项会覆盖先插入项；当前函数没有冲突检测，而 Rust `HashMap` 遍历顺序不固定，因此调用方应保证映射保持表键唯一。

## 依赖与调用关系

下游直接依赖均来自当前 crate：`model_stub::{Job, ActionType, JobState, ReorgType}` 提供过滤输入，`GetFinishedModifyIndexArgs`/`GetFinishedModifyColumnArgs` 解析完成参数，`InfoSchema` 负责按 ID/名称读取最新元数据，`IndexInfo` 等模型形成输出快照；`foreign_key.rs` 提供外键收集、排除和合并逻辑；`astersql_errors::SharedError` 是统一错误类型，`trace`/`annotatef` 保留错误链和上下文。`std::time::Instant` 在 `UpdateIndexInfo` 中只创建 `_start`，当前 Rust 实现未记录或暴露耗时。

RustCodeGraph 对 `TryAddJob` 的 callee 边明确指向三个过滤器；对目标文件未检出 Rust 生产 caller。Rust 测试接线由 `lib.rs` 的 `#[cfg(test)]` 模块完成：`ingest_recorder_test.rs` 做细粒度行为测试，`parity_test.rs` 串联公开契约，`export_test.rs` 暴露仅测试使用的 FK map 观察口。

已验证的 Go 上游为 `br/pkg/stream/rewrite_meta_rawkv.go::tryRecordIngestIndex`：普通 job 以 `isSubJob=false` 录制，多 schema job 转换为 proxy job 后以 `true` 录制。Go 中游 `br/pkg/task/stream.go::rangeFilterFromIngestRecorder` 调用表 ID 重写；Go 下游 `br/pkg/restore/log_client/client.go::generateRepairIngestIndexSQLs` 调用更新及两类迭代，`RepairIngestIndex` 在 schema reload 之后进入实际修复阶段。这些 Go 边说明设计位置，不等价于 Rust 已接入同一生产链。

## 错误处理与边界

- `TryAddJob` 对空 job 和被过滤 job 返回 `Ok(())`；目标 job 缺少 finished args 时，解析错误经 `trace` 返回，不能静默丢失待修复索引。
- `RewriteTableID` 为回调错误附加旧 `table_id`；因为替换发生在循环结束后，失败保留原始 `items`。测试 `rewrite_table_id_error_preserves_original_items` 明确验证这一原子性。
- `UpdateIndexInfo` 对已删除表跳过而不报错，因为无需再修复；表存在但其 `DBID` 无法查到 schema 时硬失败，错误包含 table ID 和 DB ID。构造外键管理器时的子表查询错误也经 `trace` 传播。
- 源码以 `tblInfo.Columns[column.Offset as usize]` 直接索引列数组；它依赖 InfoSchema 保证索引列 offset 合法，损坏或不一致的元数据会 panic，而不是返回结构化错误。
- `Iterate` 与 `IterateForeignKeys` 遇到首个回调错误立即经 `trace` 返回；先前已执行的回调不会回滚。
- Rust 的 `IterateForeignKeys` 对 `None` 做空成功，较 Go 生产方法直接解引用 manager 更稳健；但 Rust `export_test.rs::GetFKRecordMap` 刻意在未初始化时 panic，以对齐 Go 测试观察行为。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、事务或外部句柄。所有修改方法都要求 `&mut self`，Rust 借用规则保证同一实例更新期间不能并发读取；`Iterate`/`IterateForeignKeys` 只借用 `&self`，但未声明额外的业务级并发协议。测试使用 `AtomicI32` 只是闭包计数夹具，不表示生产录制器内部并发。

状态生命周期应遵循：创建 → 录制一个或多个 job →（可选）重写表 ID → 用最新 InfoSchema 更新 → 遍历消费。再次调用 `UpdateIndexInfo` 会重建并整体替换外键管理器，也会刷新能够匹配到的索引快照；对本次无法匹配的旧项并不会主动把既有 `Updated=true` 重置为 false，因此若 InfoSchema 在两次更新之间发生破坏性变化，调用方不应假定第二次调用会撤销所有旧快照。

`RewriteTableID` 为保证错误原子性会克隆每张表的索引 map，时间和额外内存与记录总量线性相关。`UpdateIndexInfo` 的主要成本为已录制表的 schema/FK 查询，以及遍历这些表的全部现存索引和索引列；外键最终按 key 合并去重。

## 与 Go 版本的对应关系

字段布局、三重过滤、AddIndex/AddPrimaryKey/ModifyColumn 参数来源、子 job `Done` 特例、表 ID 重写、最新 schema 补全、隐藏列表达式、前缀长度和 `Updated` 门闩均与 `ingest_recorder.go` 对齐。`ingest_recorder_test.rs` 对应 Go `ingest_recorder_test.go`，覆盖 Txn/DropIndex/RollbackDone 跳过、主索引和子 job 录制、列表示、表 ID 重写及 FK 组合；`parity_test.rs::go_rust_public_contract_matches` 额外把公开路径串成端到端契约。

可见差异有三点。第一，Go `New()` 返回指针，Rust 返回拥有所有权的值。第二，Go 直接使用 TiDB 的 `model`、`infoschema`、`context.Context` 和丰富的 `IndexInfo`；Rust 通过 `model_stub.rs` 的本地类型与 trait 隔离这些依赖，所以不能仅凭 API 同名推断已经和 Go 生产对象互通。第三，Go `UpdateIndexInfo` 写开始/结束日志并记录耗时，Rust 只保留未使用的 `Instant`；Go `IterateForeignKeys` 假定 manager 已初始化并会在 nil 时 panic，Rust 生产方法把未初始化视为空集合。

此外，Rust 枚举对 action 的覆盖只显式包含当前 stub 中的 `Other`/`DropIndex` 等变体；Go 的真实 `ActionType` 范围更大，但均由前置过滤表达式排除。扩展模型枚举时必须继续保持“只有三类 action 能到参数解析分支”的不变量。

## 扩展指南

新增可录制 DDL 类型时，应同时修改 `notReorgTypeJob` 与 `TryAddJob` 的参数解析/`IsPrimary` 规则，不能仅放宽过滤；还要在 `model_stub.rs` 表达真实 finished args，并在独立 `ingest_recorder_test.rs` 增加过滤、成功和缺参用例，同时对照 Go 同路径行为。若扩展 SQL 所需索引属性，优先把稳定数据放入 `IngestIndexInfo`，在 `UpdateIndexInfo` 一次性从最新 schema 填充，并保持 `Updated` 最后写入。

改变列串生成时必须维护 `%n` 与 `ColumnArgs` 一一对应、隐藏列不进入参数、前缀长度紧跟对应普通列这三个约束，并同步 `test_indexes_kind`。改变表 ID 重写时要保留失败不修改原 map的语义；若允许多对一映射，应先定义确定性的合并或显式报错，避免当前覆盖结果依赖 HashMap 顺序。

外键逻辑应在 `foreign_key.rs` 扩展，本文件只负责按“未录制索引排除候选、已录制索引保留候选”的协议调用它。消费 API 的顺序或错误语义变化需要同步 `parity_test.rs`，并检查 Go `generateRepairIngestIndexSQLs` 的先外键后索引约束。若要接入 Rust 生产恢复主链，还需新增真实上游 caller 和真实 InfoSchema/model 适配；当前 Cargo 边界与 RustCodeGraph 不足以证明该接线存在。

性能方面应关注全表索引扫描、map 克隆和 `IndexInfo` 克隆；兼容性方面应关注 Go/Rust finished args、job 状态和隐藏列 SQL 转义的一致性。测试必须继续放在独立 `*_test.rs`/`parity_test.rs`，不要内嵌回生产源文件。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、7,032 个 Rust 文件；`files --filter br/pkg/restore/ingestrec` 找到该目录 14 个 Go/Rust 文件。
- RustCodeGraph `node --file br/pkg/restore/ingestrec/ingest_recorder.rs`：核对完整 272 行源码、13 个符号、数据结构与所有分支；文件级关系仅显示被 `parity_test.rs` 使用。
- RustCodeGraph `query IngestRecorder`、`query ... --kind function` 与 `node TryAddJob`：核对 Rust/Go 同名定义，并确认 `TryAddJob` 到 `notIngestJob`、`notReorgTypeJob`、`notSynced` 的调用边；针对目标 Rust 方法的 caller 查询未返回生产边。
- RustCodeGraph `node NewForeignKeyRecordManagerForTables`、`node RemoveForeignKeys`、`node Merge`：核对外键候选的构造、按正常索引排除以及最终合并语义。
- 读取 `br/pkg/restore/ingestrec/Cargo.toml` 与 `lib.rs`：核对 crate 名、唯一外部 crate 依赖、模块挂载、再导出和独立测试模块。
- RustCodeGraph 读取 `ingest_recorder.go`，并读取 Go 直接入口 `br/pkg/stream/rewrite_meta_rawkv.go`、`br/pkg/task/stream.go`、`br/pkg/restore/log_client/client.go`：核对 Go 生产主链与实际 SQL 消费边界。
- RustCodeGraph 读取 `ingest_recorder_test.rs` 和 `parity_test.rs`，并以 `rg` 定位 `ingest_recorder_test.go` 对应调用：核对过滤、成功录制、列表示、重写、回调/缺参错误、外键、子 job 与缺库边界。测试未在本纯文档任务中执行。
- 交付结构检查使用任务规定命令，要求文件存在且固定二级标题恰好为 11 个；人工复核重点为“Rust 尚无生产 caller”的限定、Go/Rust 差异、不稳定 HashMap 顺序及所有错误边界均未被描述为无依据的已支持能力。
