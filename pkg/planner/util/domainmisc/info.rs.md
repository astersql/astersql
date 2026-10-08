# `pkg/planner/util/domainmisc/info.rs`

## 文件定位

本文档对应源文件 [`info.rs`](info.rs)。该文件属于 `astersql-planner-util-domainmisc` crate，由同目录的 `lib.rs` 以公开子模块 `info` 导出。它把规划器在使用一份可能已经过期的表元数据时所需的“读取最新 Schema 版本并按表取得索引”能力，收敛为一个不依赖具体 Domain 实现的 Rust 接口。crate 的移植元数据在 `pkg/planner/util/domainmisc/Cargo.toml` 中指向 Go 包 `pkg/planner/util/domainmisc`。

当前 Rust 文件是一个自包含的语义移植核心：RustCodeGraph 对 `get_latest_index_info` 的调用方查询只找到 `pkg/planner/util/domainmisc/info_test.rs` 中的四个测试，仓库文本搜索也未发现生产 Rust 调用点。因此它还没有像 Go 版本那样接入规划器生产主链。Go 侧的实际调用者包括 `pkg/planner/core/planbuilder.go`、`pkg/planner/core/operator/logicalop/logical_datasource.go` 和 `pkg/planner/core/point_get_plan.go`，这些调用用于在读已提交或锁定读等场景中排除已不再公开或已消失的索引。

## 核心职责

- 用 `IndexInfo` 表示本函数判断和返回所需的简化索引元信息，字段包含 ID、名称、列名列表和公开状态。
- 用 `LatestSchema` trait 隔离 Schema 提供者，要求其暴露当前版本号，以及按表 ID 返回索引列表的能力。
- 用 `get_latest_index_info` 保留 Go `GetLatestIndexInfo` 的三态协议：Schema 未变化时不刷新；Schema 已变化且表存在时返回按索引 ID 建立的映射；Schema 已变化但表不存在时返回空映射。
- 在 Schema 提供者缺失时返回与 Go 版本一致的错误文本 `domain not found for ctx`。

该文件不负责决定何时需要检查最新 Schema，也不判断索引的 `public` 字段；调用方应根据事务隔离级别、连接状态或锁定读等条件决定是否调用，并根据返回的索引元信息执行可用性过滤。此职责边界可由 Go 调用点中对 `StatePublic` 的检查验证。

## 主要符号

- `pub struct IndexInfo`（`info.rs:20`）：可克隆、可调试、支持等值比较的拥有型数据结构。`id: i64` 是结果映射的键；`name: String`、`columns: Vec<String>` 和 `public: bool` 是简化的索引属性。当前函数体只读取 `id`，其余字段由调用者消费或用于测试。
- `pub trait LatestSchema`（`info.rs:28`）：Schema 访问抽象，没有关联错误类型，也没有异步接口。
  - `schema_version(&self) -> i64`（`info.rs:30`）返回当前元数据版本。
  - `table_indexes(&self, table_id: i64) -> Option<Vec<IndexInfo>>`（`info.rs:34`）按表 ID 返回拥有型索引列表；`None` 表示表不存在。
- `pub fn get_latest_index_info(...) -> Result<(Option<BTreeMap<i64, IndexInfo>>, bool), String>`（`info.rs:42`）：唯一业务入口。第一个元组元素区分“不需要刷新”和“刷新结果”，第二个布尔值直接表示 Schema 是否变化；唯一显式错误来自缺少 Schema。

文件没有模块级常量、宏、`impl`、异步函数或条件编译项。`IndexInfo` 与 `LatestSchema` 都是公开 API；映射构造过程是函数内部实现。

## 执行流程

1. `get_latest_index_info` 先把 `Option<&dyn LatestSchema>` 转换为有效 trait 对象。输入为 `None` 时立即返回错误，不查询版本或表。
2. 调用 `LatestSchema::schema_version`，与 `start_version` 比较。相等时返回 `Ok((None, false))`，并跳过 `table_indexes`；`info_test.rs::unchanged_schema_skips_the_table_lookup` 用调用计数验证了这个短路不变量。
3. 版本不相等时，把原始 `table_id` 传给 `LatestSchema::table_indexes`。测试同时验证查询恰好发生一次且表 ID 未被改写。
4. 表不存在时，`unwrap_or_default` 把 `None` 转为空向量；表存在时使用返回的索引向量。
5. 遍历向量并以 `index.id` 为键收集为 `BTreeMap`。重复 ID 遵循 `collect`/`insert` 的覆盖语义，列表中最后出现的值胜出。
6. 返回 `Ok((Some(indexes), true))`。即使表不存在，仍返回 `Some(空映射)` 和 `true`，表示已经观察到版本变化并完成刷新，而不是“无需刷新”。

## 数据与状态

函数本身无持久状态，所有结果由三个输入和调用期间观察到的 `LatestSchema` 决定。`IndexInfo` 使用拥有型 `String` 与 `Vec<String>`，`table_indexes` 也转移整个 `Vec<IndexInfo>` 的所有权，因此构造结果时不需要借用 Schema 内部数据或延长其生命周期。

返回值的状态组合具有明确语义：

| 返回状态 | 含义 |
| --- | --- |
| `Err("domain not found for ctx")` | 没有可查询的 Domain/Schema 提供者 |
| `Ok((None, false))` | 当前版本等于 `start_version`，无需刷新 |
| `Ok((Some(非空映射), true))` | 版本变化且表存在并含索引 |
| `Ok((Some(空映射), true))` | 版本变化，但表不存在或索引列表为空 |

使用 `BTreeMap` 使迭代顺序按索引 ID 稳定；Go 版本使用无序 `map[int64]*model.IndexInfo`。映射键唯一，重复索引 ID 会覆盖旧值，这一点由 `changed_schema_returns_indexes_keyed_by_id_with_last_value_winning` 覆盖。

## 依赖与调用关系

直接代码依赖只有标准库 `std::collections::BTreeMap`，以及文件内定义的 `LatestSchema`、`IndexInfo`。`lib.rs` 公开 `info` 模块，并仅在 `cfg(test)` 下装入独立测试文件 `info_test.rs`。

`pkg/planner/util/domainmisc/Cargo.toml` 定义 crate 名 `astersql-planner-util-domainmisc`、库入口 `lib.rs` 和 Go 包映射。在 `cfg(windows)` 目标依赖区声明了 `astersql-domain`、`astersql-meta-model`、`astersql-planner-core-base` 与 `astersql-table-temptable`，对应 Go 实现的 Domain、模型、规划上下文和临时表依赖；但当前 `info.rs` 没有直接引用这些 crate。根 `Cargo.toml` 还以 `facade_planner_util_domainmisc` 名称登记该 crate。

RustCodeGraph 的实际上游边仅有四个测试函数：`missing_domain_returns_the_go_error`、`unchanged_schema_skips_the_table_lookup`、`changed_schema_returns_indexes_keyed_by_id_with_last_value_winning` 和 `changed_schema_with_missing_table_returns_an_empty_map`。图中的直接下游语义调用是 `LatestSchema::schema_version` 与 `LatestSchema::table_indexes`。当前没有实现 `LatestSchema` 的生产类型，只有测试桩 `MockSchema`。

Go 主链中，`domainmisc.GetLatestIndexInfo` 从规划器构建路径进入，向下调用 `domain.GetDomain`、`Domain.InfoSchema`、`temptable.AttachLocalTemporaryTableInfoSchema`、`SchemaMetaVersion`、`TableByID` 和 `Table.Meta`。这些具体接线尚未出现在 Rust 函数中，不能把 Go 的生产接线视为 Rust 已支持能力。

## 错误处理与边界

唯一的 `Err` 分支是 `schema == None`，错误类型为裸 `String`，文本刻意与 Go 保持一致。`LatestSchema::schema_version` 和 `table_indexes` 的签名没有错误通道，因此底层版本读取、表查询或元数据转换失败无法由本 API 表达；实现者只能在 trait 实现内部消化失败或改变接口。这是扩展时需要优先评估的兼容边界。

`table_indexes == None` 不视为错误，而是“最新 Schema 中表已不存在”。空向量与表不存在最终都归并为空映射，调用者无法通过结果区分两者。版本判断只比较相等性，并不要求新版本大于起始版本。函数也不验证负表 ID、重复索引 ID、空名称/列列表或 `public` 状态；这些输入均按提供者数据原样处理。

Go 版本先把本地临时表信息附着到 Domain 的 InfoSchema，再执行版本与表查询；Rust 抽象要求传入的 `LatestSchema` 已经具备调用方需要的视图。若生产实现忽略本地临时表，行为会与 Go 不一致。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务、网络连接或文件句柄。函数同步执行，只持有调用期间有效的共享借用 `&dyn LatestSchema`，返回前即结束借用。索引列表被按值取得并移动进 `BTreeMap`，返回值不引用 Schema，因此可独立存活。

trait 只要求 `&self`，但没有 `Send`、`Sync` 或快照一致性约束。提供者可以像测试中的 `Cell` 一样使用内部可变性；因而本 API 本身不保证跨线程可用，也不保证 `schema_version` 与随后 `table_indexes` 来自同一原子快照。生产接线若允许并发 DDL，需要由 `LatestSchema` 实现维护一致视图，或在接口层增加可验证的一致性约束。

## 与 Go 版本的对应关系

Rust `get_latest_index_info` 对应 `pkg/planner/util/domainmisc/info.go::GetLatestIndexInfo`。已对齐的语义包括：Domain/Schema 缺失时的错误文本；版本相同时返回“无映射、未变化”；版本变化时按索引 ID 建图；表不存在时返回空映射但标记已变化；重复 ID 由后出现值覆盖。

两者仍有重要结构差异：

- Go 接收 `base.PlanContext` 并自行通过 `domain.GetDomain` 获取 Domain；Rust 直接接收可选 `LatestSchema` trait 对象。
- Go 调用 `temptable.AttachLocalTemporaryTableInfoSchema` 组合会话本地临时表；Rust 没有对应步骤，这一责任隐含在提供者中。
- Go 返回完整的 `*model.IndexInfo`，包括真实索引状态等字段；Rust 返回本地简化 `IndexInfo`，字段集合和模型身份并不等价。
- Go 用 `context.Background()` 调用 `TableByID`；Rust trait 不接收上下文，不支持取消、截止时间或查询错误。
- Go 映射无序且保存指针；Rust 使用有序映射和拥有值。
- Go 已有多个生产规划器调用点；当前 Rust 仅由单元测试调用。因此应描述为逻辑语义已经移植并受测试覆盖，生产集成尚未验证。

未发现同目录 Go 单元测试文件；本次 Go 语义证据来自 `info.go` 本身及上述规划器生产调用点。Rust 边界由独立文件 `info_test.rs` 验证。

## 扩展指南

- 接入生产规划器时，应先为真实 Schema/Domain 视图实现 `LatestSchema`，并明确在哪里完成 Go 的本地临时表附着；随后把调用接到与 Go 相同的访问路径、点查和索引查找决策位置。
- 若规划器需要完整索引状态，不应仅凭当前 `public: bool` 假定等价于 Go `model.IndexInfo.State`；应扩充或复用统一模型，并为状态迁移、不可见索引、全局索引等实际消费字段增加独立测试。
- 若底层查询可能失败，应审慎把 trait 方法改为 `Result`，并同步调整 `get_latest_index_info` 的错误类型和所有实现者，避免在实现层吞掉错误。
- 若要保持同一次调用内的快照一致性，应让版本读取和表查询来自同一快照对象，或把二者合并为单个原子接口；并发 DDL 场景必须增加回归测试。
- 修改三态返回协议时，必须同步检查所有调用者对 `Option` 与 `changed` 的组合判断，尤其不能把“表消失后的空映射”混同为“版本未变化”。
- 测试继续放在同目录独立文件 `pkg/planner/util/domainmisc/info_test.rs`，不要内嵌回生产源文件。至少保留现有四类分支；新增生产实现后再增加本地临时表、底层错误、并发快照一致性和真实调用接线测试。

主要兼容风险是简化 `IndexInfo` 与 Go 模型不等价以及错误通道变更；正确性风险是临时表视图或并发 Schema 快照缺失；性能风险主要来自每次版本变化时克隆/搬移完整索引列表并构造 `BTreeMap`，而 `BTreeMap` 的插入成本高于哈希映射但提供稳定顺序。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标目录中识别到 `info.rs` 的 7 个符号、`info_test.rs` 的 11 个符号；查询时索引可用。
- RustCodeGraph `explore "pkg/planner/util/domainmisc/info.rs ..."`：读取目标文件完整源码，并核对 Go `info.go` 的对应实现。
- RustCodeGraph `query get_latest_index_info`、`callers get_latest_index_info --json`：定位入口 `info.rs:42`，确认四条调用边全部来自 `info_test.rs`；符号查询和入口源码核对 `schema_version`、`table_indexes` 两个下游 trait 方法。仓库 `rg` 复核未发现生产 Rust 调用者。
- 已读 Rust 源与模块边界：`pkg/planner/util/domainmisc/info.rs`、`pkg/planner/util/domainmisc/lib.rs`、`pkg/planner/util/domainmisc/Cargo.toml`、根 `Cargo.toml`。
- 已读独立 Rust 测试：`pkg/planner/util/domainmisc/info_test.rs`，覆盖缺少 Schema、版本未变短路、版本变化/重复 ID 和表不存在四类行为。
- 已读 Go 对照与直接调用证据：`pkg/planner/util/domainmisc/info.go`、`pkg/planner/core/planbuilder.go`、`pkg/planner/core/operator/logicalop/logical_datasource.go`、`pkg/planner/core/point_get_plan.go`；仓库搜索未找到直接命名 `GetLatestIndexInfo` 的 Go 测试。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证本文档存在且恰好包含 11 个固定二级章节，并人工复核没有把 Go 生产接线误写成 Rust 当前能力。
