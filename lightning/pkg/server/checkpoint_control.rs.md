# `lightning/pkg/server/checkpoint_control.rs`

## 文件定位

本文件属于 Cargo crate `astersql-lightning-pkg-server`，由同目录 `lib.rs` 以 `checkpoint_control` 模块装入并整体再导出。`lightning/pkg/server/Cargo.toml` 将该 crate 标记为 Go 包 `lightning/pkg/server` 的 library 移植，并直接依赖已经移植的 `checkpoints`、`importer` 与 `importinto` crates。

它位于 Lightning 运维控制面，而不是数据导入执行面：上游 `lightning/cmd/tidb-lightning-ctl/main.rs::dispatch` 根据 `cp_remove`、`cp_err_ignore`、`cp_err_destroy`、`cp_dump` 和 `local_storing_tables` 参数即时创建 `CheckpointControl`，本文件再把请求分派给传统 Lightning checkpoint 或 Import-Into checkpoint 后端。这里负责删除、修复、销毁、导出和诊断已有 checkpoint；不会创建或重新调度导入任务。

## 核心职责

- 用 `CheckpointControl` trait 固定五项后端无关操作：`Remove`、`IgnoreError`、`DestroyError`、`Dump`、`GetLocalStoringTables`。
- 用 `NewCheckpointControl` 检查 `cfg.TikvImporter.Backend`：仅 `BackendImportInto` 选择 `ImportIntoCheckpointControl`，其他值都进入 `LegacyCheckpointControl`。
- 在 legacy 路径中按操作打开并关闭 `astersql_lightning_pkg_checkpoints::DB`；在 import-into 路径中初始化并在有状态操作结束后关闭 `CheckpointManager`。
- 销毁失败 checkpoint 时，除删除 checkpoint 记录外，还删除目标 TiDB 表；legacy local backend 额外清理排序目录中的 engine 数据。
- 用 `CleanupMetas` 清理 importer 的 `table_meta`/`task_meta_v2` 元数据，并把特殊表名 `common::AllTables` 转换为 importer 约定的空字符串。
- 为两种后端维持相同的三文件诊断协议：`tables.csv`、`engines.csv`、`chunks.csv`。

## 主要符号

- `pub trait CheckpointControl: Send`：对象安全的同步控制接口。所有方法接收可变控制器，返回 crate 级 `Result`；本 trait 只要求 `Send`，不承诺 `Sync` 或并发共享。
- `pub fn NewCheckpointControl(...) -> Result<Box<dyn CheckpointControl>>`：统一工厂，也是 CLI 的直接入口。
- `LegacyCheckpointControl { cfg, tls }` 与 `NewLegacyCheckpointControl`：持有配置和 TLS 的传统实现。当前 Rust 实现的 `DestroyError` 通过 importer bridge 构造 TiDB manager 时传入 `None`，所以 `tls` 字段目前只保留接口/结构对齐，没有被后续逻辑读取。
- `LegacyCheckpointControl::withDB`：打开 checkpoint DB、执行一次闭包、最后尝试 `Close` 的模板。关闭错误只写 warning，不覆盖动作本身的结果。
- `ImportIntoCheckpointControl { cfg, mgr, tls }`：Import-Into 实现；`mgr` 用 `Option<Arc<dyn CheckpointManager>>` 表示可被 `closeManager` 取走的一次性资源。`tls` 当前同样未被实际 TiDB manager 构造使用。
- `ImportIntoCheckpointControl::closeManager`：通过 `Option::take` 保证 manager 最多关闭一次；关闭错误仅记录 warning。
- `ImportIntoCheckpointControl::with_manager_for_test`：仅在 `cfg(test)` 下提供 mock manager 注入，不属于生产 API。
- `NewImportIntoCheckpointControl`：创建 manager 后立即调用 `Initialize`，使 file/MySQL manager 在控制命令执行前载入状态。
- `CleanupMetas`：复用 importer crate 的真实数据库和元数据删除实现，而不是 server 本地替身。
- `TABLE_META_TABLE_NAME`、`TASK_META_TABLE_NAME`：分别固定为 `table_meta` 与 `task_meta_v2`。
- `DBFromConfigLocal`：返回内存 `crate::sql::DB` 的兼容入口；注释明确 checkpoint 元数据清理不走此函数。它不是 `CleanupMetas` 的下游。
- `importer_table_exists`：查询 `information_schema.tables`，对 schema/table 中的单引号做 SQL 字面量转义，并把 count 解析为布尔存在性。

## 执行流程

1. `tidb-lightning-ctl::dispatch` 仅在发现某个 checkpoint 参数时调用 `NewCheckpointControl`，避免无关命令提前触碰 checkpoint 存储。
2. 工厂依据 backend 返回 trait object。legacy 构造只克隆配置/TLS；import-into 构造还调用 `NewCheckpointManager` 和 `Initialize`。
3. legacy 的每个操作进入 `withDB`，由 `bridges::to_checkpoints_cfg` 转换配置并调用 `OpenCheckpointsDB`：
   - `Remove` 先读取 `TaskCheckpoint`。仅当存在非零 `TaskID` 时先执行 `CleanupMetas`，随后调用 `RemoveCheckpoint`。
   - `IgnoreError` 直接调用 `IgnoreErrorCheckpoint`，把指定表或全部表的错误状态交给 checkpoint 后端恢复。
   - `DestroyError` 先创建真实 importer TiDB manager，再让 checkpoint DB 返回应销毁的表；逐表 `DropTable`，若 backend 为 `BackendLocal`，还遍历闭区间 `MinEngineID..=MaxEngineID`，用 `backend::MakeUUID` 定位并 `Engine::Cleanup`。只有表和 engine 清理均无错误时才清理 meta。
   - `Dump` 创建目录和三份 CSV，按 tables、engines、chunks 的顺序调用 checkpoint DB 导出。
   - `GetLocalStoringTables` 把 checkpoint DB 返回的 `HashMap<table, engine_ids>` 包装成 `Some`。
4. import-into 的 `Remove`、`IgnoreError` 直接转发给 manager；`Dump` 创建相同的三份 CSV 后转发三个 dump 调用；这些操作无论成功或失败都会随后调用 `closeManager`。
5. import-into 的 `DestroyError` 先创建 TiDB manager，再调用 manager 的 `DestroyError` 获取待删表，逐表执行 `DropTable`；仅在没有 drop 错误时调用 `CleanupMetas`。TiDB manager 在内部动作完成后关闭，checkpoint manager 在最外层关闭。
6. import-into 的 `GetLocalStoringTables` 直接返回 `Ok(None)`，因为该后端没有 legacy local-engine 残留这一概念，也不会因此关闭 manager。
7. `CleanupMetas` 对 `all` 做空串转换，连接目标 TiDB，先检查并按表名删除 `table_meta`；若 `task_meta_v2` 不存在则成功返回，否则调用 `MaybeCleanupAllMetas` 处理任务级和 schema 级收尾。

## 数据与状态

控制器自身不缓存 checkpoint 内容。legacy 控制器仅保存 `config::Config` 和 `common::TLS` 的克隆，每次操作重新打开 DB；import-into 控制器保存一个已初始化 manager，并在首次有状态操作后把 `mgr` 从 `Some` 变为 `None`。因此同一 `ImportIntoCheckpointControl` 实例设计上是一次操作对象，不应在 `Remove`、`IgnoreError`、`DestroyError` 或 `Dump` 完成后复用；这些方法中的 `unwrap()` 依赖该不变量。

`tableName` 可以是具体的带限定表名，也可以是 `common::AllTables`。checkpoint 后端直接解释该选择；只有 meta 清理层把 `all` 转换为空字符串。`DestroyError` 使用 checkpoint 返回的 `TableName`、`MinEngineID`、`MaxEngineID` 决定外部副作用。错误集合用 `Vec<Error>` 聚合，保证多个表或 engine 的失败都可被报告。

`GetLocalStoringTables` 的两种“无数据”语义不同：legacy 返回 `Some(empty_map)` 表示该后端支持查询但当前无残留，import-into 返回 `None` 表示该概念不适用。CLI 最终会把两者都显示为无待报告表，但库调用方仍可区分。

## 依赖与调用关系

RustCodeGraph 的精确查询显示：`NewCheckpointControl` 的生产调用者是 `lightning/cmd/tidb-lightning-ctl/main.rs::dispatch`；它下调 `NewLegacyCheckpointControl` 或 `NewImportIntoCheckpointControl`。`CleanupMetas` 的本文件调用边来自 legacy `Remove`、legacy `DestroyError` 和 import-into `DestroyError`。

主要下游如下：

- `astersql-lightning-pkg-checkpoints`：`OpenCheckpointsDB`、`TaskCheckpoint`、`RemoveCheckpoint`、`IgnoreErrorCheckpoint`、`DestroyErrorCheckpoint`、三个 dump 方法和 `GetLocalStoringTables`。
- `astersql-lightning-pkg-importinto`：`NewCheckpointManager`、`Initialize` 以及对应控制/dump/close 方法。
- `astersql-lightning-pkg-importer`：`NewTiDBManager`、`DBFromConfig`、`RemoveTableMetaByTableName`、`MaybeCleanupAllMetas` 与 importer SQL DB。
- server 内部 bridge：`bridges::to_*_cfg` 把 server 配置转换到下游 crate，`bridges::map_err_*` 把三个 crate 的错误统一映射到 server `Error`。
- `backend::MakeUUID` 与 `ingestctrl::Engine::Cleanup`：只服务 legacy local backend 的物理 engine 清理。
- `std::fs`/`std::io::Write`：建立 dump 目录和文件；文件句柄在离开作用域时由 RAII 关闭。

## 错误处理与边界

- 打开 checkpoint DB、初始化 manager、查询状态以及 dump 调用失败时，错误经对应 bridge 映射，并多处用 `errors::Trace` 补充传播轨迹。
- `withDB` 即使闭包失败仍尝试关闭 DB；`closeManager` 即使业务动作失败仍尝试关闭 manager。关闭失败只记录告警，因此不会掩盖原业务错误，也不会单独使原本成功的动作失败。
- 两个 `DestroyError` 实现都会继续处理剩余表并用 `errors::Join` 汇总 drop/engine/meta 错误；一旦出现表或 engine 错误便跳过 `CleanupMetas`，避免资源仍存在时先抹掉恢复元数据。
- legacy `DestroyError` 若在获取待销毁表之前失败，TiDB manager 的显式 `Close` 不会执行；这是当前 Rust 控制流的可见边界。import-into 的外层则仍会关闭 checkpoint manager。
- `Dump` 可能已经创建部分文件后失败；没有事务式回滚，调用方必须把失败目录视作不完整输出。legacy file checkpoint 是否支持 dump 由后端决定，测试明确期待其返回错误。
- `importer_table_exists` 把 `not_found` 查询错误当作不存在；其他 SQL 错误继续传播。无法把返回值解析为 `u64` 时给出包含原始 count 的错误。
- `DBFromConfigLocal` 是内存兼容桩，不能替代真实 importer I/O。独立测试通过源码契约明确禁止 `CleanupMetas` 回退到本地替身。

## 并发与资源生命周期

本文件没有线程、异步任务、锁或通道。`CheckpointControl: Send` 允许对象跨线程转移，但方法需要 `&mut self`，且没有 `Sync` 约束；调用方应串行执行控制动作。

legacy checkpoint DB 的生命周期严格包在单次 `withDB` 调用内；Import-Into manager 在构造时初始化，在控制动作结束后由 `closeManager` 取出并关闭。TiDB manager 在 `DestroyError` 尾部显式关闭。CSV 文件由 Rust 作用域自动释放；该实现没有显式检查文件关闭/flush 错误。

资源回收是 best effort：DB/manager 的关闭错误仅告警。特别要注意，`GetLocalStoringTables` 在 import-into 路径不使用也不关闭 manager；若只执行该查询，manager 的最终回收依赖其 `Arc`/对象析构，而不是 `Close` 调用。测试注入的 `Arc<dyn CheckpointManager>` 还用于记录严格调用顺序。

## 与 Go 版本的对应关系

主体接口、backend 分流、legacy 的五种操作、DestroyError 的“先删资源、无错误再清 meta”、Import-Into 返回 nil/`None` 的本地残留语义，以及三份固定 CSV 文件均与 `checkpoint_control.go` 对齐。Rust 独立测试的表驱动 case 与 Go 的 `TestImportIntoCheckpointControl`、`TestLegacyCheckpointControl`、`TestNewCheckpointControl_LegacyBackend` 对应。

已确认的实现差异如下：

- Go `withDB` 使用调用者 `context.Context`，Rust 在下游 checkpoints/importer/importinto crates 之间使用各自的 `Background()`，传入的 server `ctx` 主要只继续传给 `CleanupMetas`；取消/超时传播目前并不等价。
- Rust `NewImportIntoCheckpointControl` 显式调用 `Initialize`，以满足已移植 file/MySQL manager 的加载时机；Go 构造器返回的 manager 可直接使用。
- Go 将 TLS 传给 `importer.NewTiDBManager`；当前 Rust bridge 调用传 `None`，两个控制器保存的 `tls` 字段未参与 I/O。
- Go `os.MkdirAll` 明确使用 `0750`，Rust `create_dir_all` 依赖平台默认权限与 umask；两者的文件名/调用顺序一致，但权限合同未完全对齐。
- Go 用 `defer` 关闭 DB、manager、TiDB manager 和文件；Rust 用显式关闭加 RAII。多数错误路径等价，但 legacy Rust 在部分早退路径上可能绕过 TiDB manager 的显式 `Close`。
- Go 的 `CleanupMetas` 位于 `lightning.go` 并调用 `common.TableExists`；Rust 将它放在本文件，通过 importer SQL DB 的 `QueryRowString` 实现等价的存在性检查。

## 扩展指南

- 新增控制动作时，先扩展 `CheckpointControl`，并同时实现 legacy 与 import-into 两条路径；CLI 若需要暴露该动作，应在 `tidb-lightning-ctl::dispatch` 增加独立参数分支。
- 新增 backend 时不要默认复用 legacy 分支；应明确其 checkpoint 存储、初始化、关闭、meta 清理以及本地 engine 概念，再修改 `NewCheckpointControl`。
- 修改销毁顺序时必须保持“外部表/engine 清理失败则保留 meta”的恢复不变量，并继续聚合多个资源错误。涉及 local engine 标识时同步检查 `backend::MakeUUID` 和 engine ID 闭区间。
- 修改 dump 协议时应同步维护两种实现的三个文件名、调用顺序和内容兼容性，并补充部分文件创建或下游 dump 失败的测试。
- 修改 Import-Into manager 生命周期时，消除当前 `Option::unwrap` 的一次性前提或明确拒绝复用，并覆盖关闭失败、重复调用以及仅调用 `GetLocalStoringTables` 的情形。
- 修改 meta 清理时应复用 importer crate 的真实操作，不能改接 `DBFromConfigLocal`；同时验证 `all` 到空串的协议、meta 表不存在、count 解析失败及未完成表阻止全量清理等边界。
- Rust 单元测试继续放在独立的 `checkpoint_control_test.rs`；与 Go 对齐时同步检查 `checkpoint_control_test.go`，不要把测试嵌入生产文件。

## 验证依据

- 源文件：`lightning/pkg/server/checkpoint_control.rs`，核对了 trait、两个控制器、工厂、五项操作、`CleanupMetas`、常量和存在性查询辅助函数。
- Crate 装配：`lightning/pkg/server/Cargo.toml`、`lightning/pkg/server/lib.rs`，核对 crate 归属、四个直接 Lightning 依赖和模块再导出关系；目录内不存在 `doc.go`。
- 上游入口：`lightning/cmd/tidb-lightning-ctl/main.rs::dispatch`，核对五个 CLI 操作到 trait 方法的生产调用边。
- RustCodeGraph：`status` 显示索引含 11,467 个文件并覆盖目标文件；`files --filter lightning/pkg/server` 确认源/Go/测试集合；`node --file ...` 读取 580 行源码；精确 `query/explore` 确认 `NewCheckpointControl -> NewLegacyCheckpointControl/NewImportIntoCheckpointControl`、CLI `dispatch -> NewCheckpointControl`，以及三个 `CleanupMetas` 调用位置。
- Rust 测试：`lightning/pkg/server/checkpoint_control_test.rs`，核对真实 importer 清理路径、Import-Into 调用/关闭顺序和错误分支、三份非空 CSV、legacy 单删/全删/错误恢复/local engine 查询、工厂的 legacy 分流。
- 补充 Rust 合同测试：`lightning/pkg/server/parity_test.rs`，核对 Import-Into file manager 的初始化后 dump/DestroyError、legacy 查询和 `None` 语义。
- Go 对照：`lightning/pkg/server/checkpoint_control.go`、`lightning/pkg/server/lightning.go::CleanupMetas`、`lightning/pkg/server/checkpoint_control_test.go`，用于确认接口、操作顺序、资源回收和测试意图，并记录上述 Rust 差异。
- 本任务是纯文档分析，按计划不运行 Cargo；结构检查用于确认目标文档存在且恰有规定的十一个二级章节。
