# `pkg/ddl/schema.rs`

## 文件定位

[`schema.rs`](schema.rs) 属于 `astersql-ddl` crate。crate 根在 [`lib.rs`](lib.rs) 中以 `pub mod schema` 公开该模块，并仅在 `cfg(test)` 下装配独立的 [`schema_test.rs`](schema_test.rs)。[`Cargo.toml`](Cargo.toml) 的 `[package.metadata.porting]` 将整个 crate 对应到 Go 的 `pkg/ddl`；本文件自身只依赖标准库 `std::collections::BTreeMap`。

该文件是数据库（schema）级 DDL 的内存目录与状态机模型：它在单个 `SchemaCatalog` 值内模拟建库、修改默认字符集/排序规则、修改默认 Placement Policy、分阶段删库、按 ID 恢复库和收集表物理 ID。它不是当前 Rust DDL 运行时主链的持久化实现：RustCodeGraph 的 callers 查询没有发现生产调用者，仓库引用搜索只发现独立单元测试直接使用这些 API。实际 Go 主链由 [`job_worker.go`](job_worker.go) 分派到 [`schema.go`](schema.go) 的 job handler，并包含元数据事务、schema version、通知和外部资源清理等机制。

按 DDL 执行框架的问题分类，本文件模拟的是 job handler 内的一小段元数据行为：删除具有 `Public -> WriteOnly -> DeleteOnly -> None` 状态迁移；恢复被压缩为从缓存直接回到 `Public`；没有 job 持久化、owner/failover、reorg/backfill、schema-version 同步、MDL、delete-range GC 或系统表。不能把 `SchemaCatalog` 的成功视为集群级 DDL 已完成。

## 核心职责

1. 用大小写不敏感的名称键维护仍可见的数据库，用数据库 ID 维护已删除、可恢复的数据库（`SchemaCatalog.schemas`、`SchemaCatalog.dropped`）。
2. 在创建和修改入口执行局部输入校验，并保持“先解析目标库、再校验新值”的 Go 错误优先级（`create_schema`、`modify_charset_and_collation`、`modify_placement`）。
3. 显式推进删库状态机，并在最终 `None` 阶段把完整 `DatabaseInfo` 从活动目录转入恢复缓存（`drop_schema_step`）。
4. 恢复时保留原 ID、名称、字符集、排序规则、Placement Policy 和表摘要，只把状态重置为 `Public`（`recover_schema`）。
5. 提供两个无状态辅助函数：校验有限字符集/排序规则组合，以及按稳定顺序展开表 ID 与分区物理 ID（`validate_charset_and_collation`、`schema_physical_ids`）。

职责边界非常窄：这里不解析 SQL，不构造或提交 DDL job，不读写 TiDB meta KV，不更新全局 schema version，也不验证真实 Placement Policy 是否存在。

## 主要符号

- `SchemaState { Public, WriteOnly, DeleteOnly, None }`：本地 schema 可见性状态。删除方向按枚举注释中的四阶段推进；本文件没有 Go 恢复路径的 `None -> WriteOnly -> Public` 两步状态机。
- `DatabaseInfo`：目录中的数据库快照，包含 `id`、保留原始大小写的 `name`、规范化后的 `charset`/`collation`、可选 `placement_policy`、`state` 与 `tables`。字段均公开，调用方可直接修改，因此不变量主要依靠方法约定而非类型封装。
- `SchemaTable`：只保存逻辑表 ID 与分区物理 ID，是清理路径所需的最小表摘要，不是完整表元数据。
- `SchemaError`：封闭错误集合，包括重复、缺失、字符集/排序规则非法、Placement Policy 文本非法和恢复同名冲突。`Display` 直接输出 `Debug` 名称，并实现 `std::error::Error`。
- `SchemaCatalog`：核心可变状态容器。`Default` 从 `next_id = 0`、两个空 `BTreeMap` 开始；无公开构造器或持久化接口。
- `SchemaCatalog::create_schema(...) -> Result<Option<i64>, SchemaError>`：创建成功返回 `Ok(Some(id))`；命中 `IF NOT EXISTS` 返回 `Ok(None)`；其他重复返回 `AlreadyExists`。
- `modify_charset_and_collation(...) -> Result<bool, SchemaError>` 与 `modify_placement(...) -> Result<bool, SchemaError>`：返回值表示新旧值是否不同，但即使未改变也会把字段赋为传入值/规范化值。
- `drop_schema_step(...) -> Result<SchemaState, SchemaError>`：一次调用只推进一个删除阶段；到 `None` 时转移所有权到 `dropped`。
- `recover_schema(schema_id) -> Result<(), SchemaError>`：按 ID 取出 dropped 记录；名称冲突时将记录放回，避免恢复材料丢失。
- `schema(name) -> Option<&DatabaseInfo>`：只读查询活动目录，不查询 dropped 缓存。
- `validate_charset_and_collation`、`validate_placement_policy`、`schema_physical_ids`：公开的局部校验/展开工具。

## 执行流程

创建流程（`create_schema`）：

1. 将输入名称做 ASCII 小写，作为 `schemas` 唯一键；展示名称仍保存原字符串。
2. 若键已存在，根据 `if_not_exists` 返回 `Ok(None)` 或 `AlreadyExists`，此时不校验其余参数，也不分配 ID。
3. 依次校验字符集/排序规则与 Placement Policy 文本。
4. 用 `self.next_id.saturating_add(1).max(1)` 分配单调 ID；饱和到 `i64::MAX` 后会反复得到同一 ID，这是当前实现的显式边界。
5. 插入 `Public`、空表列表的 `DatabaseInfo`，并返回 ID。

修改流程（`modify_charset_and_collation`、`modify_placement`）：

1. 先用小写名称查找活动库；缺失立即返回 `NotFound`。
2. 再校验请求值，因此“目标不存在 + 请求值非法”稳定地报告 `NotFound`。这一顺序由源码注释及 `schema_modify_reports_missing_schema_before_invalid_new_values_like_go` 固定。
3. 比较旧值后更新字段，返回 `changed`。字符集与排序规则总是存为 ASCII 小写；Placement Policy 字符串不规范化。

删除流程（`drop_schema_step`）：

1. 查找活动库并读取当前状态。
2. 计算下一状态：`Public -> WriteOnly -> DeleteOnly -> None`；匹配分支也定义了 `None -> None`，但正常情况下 `None` 记录已经不在 `schemas`，外部无法通过公开方法走到该分支。
3. 非最终阶段原地更新状态；最终阶段从 `schemas` 移除，设置 `state = None`，再以 ID 插入 `dropped`。
4. 返回本次到达的状态。再次按名称删除已完成的库会得到 `NotFound`。

恢复流程（`recover_schema`）：

1. 按 ID 从 `dropped` 移除记录；ID 不存在返回 `NotFound`。
2. 若活动目录已有同名键，将刚移除的记录放回原 ID，并返回 `RecoveryConflict`。
3. 否则把状态直接设为 `Public`，按小写名称插回活动目录；原 ID 和其他元数据保持不变。

物理 ID 展开（`schema_physical_ids`）按输入表顺序输出每个逻辑表 ID，随后输出该表的全部 `partition_ids`；它不排序、不去重、也不检查 ID 合法性。

## 数据与状态

`SchemaCatalog` 的两个索引表达互斥生命周期：活动库只在 `schemas: BTreeMap<String, DatabaseInfo>`，完成删除的库只在 `dropped: BTreeMap<i64, DatabaseInfo>`。正常公开方法路径不会让同一个 `DatabaseInfo` 同时出现在两者。`BTreeMap` 使按键迭代具有确定顺序，但当前 API 没有暴露迭代器，主要价值是确定性的内部存储。

名称唯一性只使用 `to_ascii_lowercase`，不是 Unicode case folding，也没有 TiDB 标识符规则或系统库规则。`DatabaseInfo.name` 保留首次创建时的大小写，因此恢复键会重新由该展示名计算。字符集校验只接受：

- `utf8mb4` 搭配 `utf8mb4_*`；
- `utf8` 搭配 `utf8_*` 且排除 `utf8mb4_*`；
- `latin1` 搭配 `latin1_*`；
- `ascii` 搭配 `ascii_*`；
- `binary` 只搭配精确的 `binary`。

此处只做前缀/精确字符串判断，并不证明具体 collation 已注册。Placement Policy 校验同样只拒绝 `Some` 中的空白字符串；`None` 表示清除策略，非空名称是否存在不在本文件验证。

`next_id` 不因删除或恢复回退，正常情况下保证新建库不复用较小 ID；但它与 `dropped` 均只存在于进程内，重建 `SchemaCatalog` 会丢失全部状态。`tables` 在创建时为空，本文件也没有新增表摘要的方法，必须由同 crate 的其他调用方直接填充公开字段；当前生产调用缺失意味着 `schema_physical_ids` 在此模块内主要由测试构造输入验证。

## 依赖与调用关系

模块装配关系为 `pkg/ddl/lib.rs -> pub mod schema -> pkg/ddl/schema.rs`；测试装配关系为 `#[cfg(test)] mod schema_test -> crate::schema::{...}`。`pkg/ddl/Cargo.toml` 声明 crate 名 `astersql-ddl`，但本文件的直接代码依赖只有标准库，没有使用 Cargo 中列出的 meta、infoschema、schemaver 或 placement crates。

RustCodeGraph 核对到的内部调用边包括：

- `create_schema -> validate_charset_and_collation`；
- `create_schema -> validate_placement_policy`；
- `modify_charset_and_collation -> validate_charset_and_collation`；
- `modify_placement -> validate_placement_policy`；
- 各目录操作调用 `BTreeMap` 的 `get`、`get_mut`、`insert`、`remove`。

RustCodeGraph callers 对这些入口未返回生产调用边；仓库文本引用确认直接调用集中在 [`schema_test.rs`](schema_test.rs)。因此不要把同名的 Rust [`executor.rs`](executor.rs) `create_schema`/`recover_schema` 或 [`persistent_actions.rs`](persistent_actions.rs) 函数自动视为本类型的上游调用者：它们是独立实现，名称相同但没有调用边。

Go 运行时关系则是 [`job_worker.go`](job_worker.go) 的 action 分派调用 [`schema.go`](schema.go) 中 `onCreateSchema`、`onModifySchemaCharsetAndCollate`、`onModifySchemaDefaultPlacement`、`worker.onDropSchema` 和 `worker.onRecoverSchema`。这条 Go 链仅用于语义对照，不是本 Rust 文件当前已接线的调用链。

## 错误处理与边界

所有目录修改都用 `Result` 返回业务错误，且在返回错误前尽量不改变可观察状态：创建先完成校验再递增 ID；修改先完成查找与校验再赋值；恢复冲突会把记录放回 `dropped`。例外是 Rust 进程级故障或内存分配失败，这些不在错误枚举中。

错误优先级和重要边界如下：

- 重复创建先于字符集和 Placement 校验；带 `if_not_exists` 时重复被视为无操作。
- 两种修改都先报 `NotFound`，再检查请求值。
- 删除或恢复未知对象统一报 `NotFound`，没有携带名称/ID 上下文。
- `SchemaError::Display` 只打印枚举名，不包含参数、调用位置或错误链。
- `validate_charset_and_collation` 的前缀规则可能接受实际不存在的排序规则；`validate_placement_policy` 可能接受不存在的策略。
- `schema_physical_ids` 保留重复和负数；调用方必须自行保证元数据有效。
- `next_id` 饱和后不再唯一；实现没有显式 `IdExhausted` 错误。

与 Go 相比，这里没有 job cancel/rollback 状态、事务回滚、外部调用失败、GC safe point、历史 job 错误或 best-effort 清理日志。扩展错误类型时应保持失败前的状态原子性，并为错误优先级补充独立 Rust 测试。

## 并发与资源生命周期

`SchemaCatalog` 的修改 API 都要求 `&mut self`，单个值在安全 Rust 中不能被多个调用者同时无锁修改；本文件本身不创建线程、任务、锁、通道或异步 future。若上层需要跨线程共享，必须自行使用互斥/消息传递，并明确一次 DDL step 的锁粒度。

资源生命周期完全由值所有权决定：创建把新 `DatabaseInfo` 移入 `schemas`；最终删除用 `remove` 取得所有权并移入 `dropped`；恢复再从 `dropped` 移回 `schemas`。恢复冲突的“取出后放回”路径避免记录提前析构。销毁 `SchemaCatalog` 会一次性释放活动库、dropped 缓存、表/分区向量及所有字符串，不存在持久化或崩溃恢复。

这里没有真实 Go DDL 的 owner-only 执行、job 重试、schema lease/version barrier、GC 开关和快照生命周期。因此，把它接入运行时前必须先决定持久化点和跨节点同步语义，不能只在外层加一把锁就认为获得了集群一致性。

## 与 Go 版本的对应关系

直接对照文件是 [`schema.go`](schema.go)，相关 Go 回归测试入口是 [`schema_test.go`](schema_test.go)。对应关系不是逐函数等价，而是抽取了部分可测试语义：

- Rust `create_schema` 对照 Go `onCreateSchema` 的重复检查和最终 `Public` 状态；Rust 自行分配内存 ID，而 Go 使用 job 已分配的 `SchemaID`、检查名称和 ID 冲突、更新 schema version，并通过 meta mutator 持久化。
- Rust `modify_charset_and_collation` 保留 Go `onModifySchemaCharsetAndCollate` 的“先确认库存在”和相同值无实际变更语义，但 Rust 额外做简化字符串校验；Go 的请求值在更上游解析，并会更新 meta/schema version、完成 job。
- Rust `modify_placement` 对照 Go `onModifySchemaDefaultPlacement` 的查库、无变化判断、设置/清除默认策略；Rust 只拒绝空白名称，不会二次确认 policy 引用存在，也不表达 Go 的完整 placement 引用结构。
- Rust `drop_schema_step` 对照 Go `worker.onDropSchema` 的 `Public -> WriteOnly -> DeleteOnly -> None` 主状态机。Go 每步更新 schema version/meta，并处理外键、TTL 外部负载、label rules、affinity、masking policy、物化视图信息、notifier 分批及 finished args；Rust 均未实现，只在最终阶段保存一份内存快照。
- Rust `recover_schema` 只对照“恢复后保持数据库身份并回到 Public”的结果。Go `worker.onRecoverSchema` 实际执行 `None -> WriteOnly -> Public`，检查/暂时关闭 GC、验证 safe point、从快照加载表与 auto IDs、禁用恢复表 TTL 调度并逐表恢复；Rust 没有这些安全条件。
- Rust `schema_physical_ids` 表达 drop 清理需要同时覆盖表 ID 和分区 ID 的意图；Go drop handler 使用完整 `TableInfo` 和其他 helper 生成清理/通知数据，而非调用该 Rust helper。

[`schema_test.rs`](schema_test.rs) 验证了大小写不敏感重复创建、`IF NOT EXISTS`、修改、三步删除、按 ID 恢复、元数据 ID 保持、非法局部输入、表/分区 ID 顺序，以及缺失库优先于非法新值。Go [`schema_test.go`](schema_test.go) 还验证 job 历史、实际 meta 状态、含表/空库删除和 owner 等集成行为；这些不能由当前 Rust 单元测试替代。

## 扩展指南

如果只扩展这个内存模型，应从最小拥有符号切入：

- 新增数据库字段：修改 `DatabaseInfo`、`create_schema` 的初始化、删除/恢复保持性断言，并同步独立 [`schema_test.rs`](schema_test.rs)，不要把测试嵌入生产文件。
- 新增字符集或 collation：修改 `validate_charset_and_collation`，补充合法、非法、大小写和相似前缀用例；若要与 TiDB 完全一致，应复用 parser charset 注册表，而不是继续扩展手写前缀。
- 增强 Placement Policy：修改 `validate_placement_policy` 或引入可查询依赖，并覆盖不存在、删除中的 policy、清除策略和不变更路径；同时对照 Go `checkPlacementPolicyRefValidAndCanNonValidJob`。
- 改变删除状态：修改 `SchemaState` 与 `drop_schema_step`，同步逐步状态断言，并核对 Go `worker.onDropSchema`、schema diff 和在线可见性契约。
- 增强恢复：优先补齐明确的中间状态、冲突/失败后的回滚不变量和持久化边界；若目标是生产接线，必须与 job/owner/meta/schema-version/GC safe point 体系集成，不能以 `dropped` 内存缓存代替。
- 扩展物理 ID 收集：修改 `SchemaTable`/`schema_physical_ids` 时明确是否排序、去重以及是否包含其他物理对象，并同步 drop/delete-range 的 Go 语义。

兼容性风险集中在错误顺序、名称折叠、ID 稳定性与状态可见性；正确性风险集中在恢复冲突时的数据保留和最终删除时的所有权转移；性能上当前操作为 `BTreeMap` 的对数查找，`schema_physical_ids` 为表及分区总数的线性扫描。若未来存入大量 dropped 元数据，需要定义淘汰/持久化策略，避免无界内存增长。

## 验证依据

本说明以以下直接证据为准：

- 生产源码：[`schema.rs`](schema.rs) 全部 257 行，核对所有枚举、结构体、方法、辅助函数及注释。
- crate 边界：[`Cargo.toml`](Cargo.toml) 的 package、lib path 与 Go porting metadata；[`lib.rs`](lib.rs) 的 `pub mod schema` 和 `#[cfg(test)] mod schema_test`。
- Rust 独立测试：[`schema_test.rs`](schema_test.rs) 的三个测试，覆盖创建/修改/删除/恢复、校验/物理 ID、错误优先级。
- Go 对照：[`schema.go`](schema.go) 的五个 schema job handler及其持久化/副作用；[`job_worker.go`](job_worker.go) 的 action 分派；[`schema_test.go`](schema_test.go) 的 schema 状态与 job 集成检查。
- 包级契约：[`doc.go`](doc.go) 声明 DDL 的在线算法与全局 schema-version 同步不变量；该契约用于识别本地内存模型尚未覆盖的运行时职责，而不是推断本文件已实现它们。
- RustCodeGraph：`status` 显示索引覆盖本仓库；`node --file pkg/ddl/schema.rs` 读取完整源码；`query` 定位 `SchemaCatalog`、`SchemaState`、`SchemaError` 及全部主要函数；`callees` 验证校验函数和容器操作的内部边；`callers` 未发现生产调用，随后以仓库引用搜索确认只有独立 Rust 测试直接使用本模块。

结构验收使用任务指定命令，要求文档存在并且恰好包含这里的十一个固定二级标题。由于任务是纯文档分析，按计划不运行 Cargo；运行时接线、集群行为和性能基准均未在本任务中验证。
