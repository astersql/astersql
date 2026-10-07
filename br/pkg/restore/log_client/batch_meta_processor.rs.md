# `br/pkg/restore/log_client/batch_meta_processor.rs`

## 文件定位

该文件属于 `astersql-br-pkg-restore-log-client` library crate；crate 根由 `br/pkg/restore/log_client/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，`lib.rs` 以 `pub mod batch_meta_processor` 挂载本模块并用 `pub use batch_meta_processor::*` 扁平导出其公开 API。它位于日志恢复（PiTR）客户端的 meta KV 处理层：上游把 DDL/meta 日志文件交给处理器，处理器复用 `client.rs` 的分 CF、排序和分批框架，再选择“恢复重写”或“只读收集映射信息”两种回调策略。

本文件不是文件读取或真实存储写入的实现位置。批次划分在 `client.rs::LoadAndProcessMetaKVFilesInBatch`，KV 读取/过滤和恢复委托给 `LogClient`，表映射类型当前从 `crate::stubs::stream` 引入。RustCodeGraph 的文件节点显示本文件被 `client.rs`、`client_test.rs`、`parity_test.rs` 使用；全仓 `rg` 进一步确认，当前 Rust 生产代码没有直接构造这两个处理器，生产级入口仍只见于 Go 的 `br/pkg/task/stream.go`。

## 核心职责

1. `BatchMetaKVProcessor` 定义统一批回调契约：输入某个 CF 的文件、上批遗留条目和时间上界 `filterTS`，返回未消费、需带到后续批次的条目。
2. `RestoreMetaKVProcessor` 编排恢复路径：启动 GC rows loader，按 default/write CF 分离排序和分批，委托 `LogClient::RestoreBatchMetaKVFiles`，最后根据是否存在显式过滤选择全量 schema reload 或逐表刷新。
3. `MetaKVInfoProcessor` 编排预扫描路径：使用同一批加载框架解析 meta KV，维护表 ID 映射及表历史，并在扫描完成后清理临时 KV 状态。
4. 保持与 `batch_meta_processor.go` 的类型划分和主控制流对齐，同时显式暴露 Rust 迁移现状：该层的编排已存在，但若干下游方法仍是桩，不能把成功返回等同于已经完成真实集群恢复。

## 主要符号

- `pub trait BatchMetaKVProcessor`：对象安全的同步回调 trait。`ProcessBatch(&mut self, ..., entries: Vec<KvEntryWithTS>, filterTS: u64, cf: &str) -> Result<Vec<KvEntryWithTS>>` 取得遗留条目所有权，允许实现更新内部状态，并用返回值跨批传递未到处理时间的条目。
- `pub struct RestoreMetaKVProcessor<'a>`：持有对 `LogClient` 的独占可变借用、按值保存的 `SchemasReplace`，以及两个 `FnMut + Send` 回调。`updateStats` 接收本批 KV 数和字节数；`progressInc` 表示文件级进度。
- `NewRestoreMetaKVProcessor`：构造恢复处理器。与 Go 保存 `*SchemasReplace` 不同，Rust 构造函数按值取得并持有一份 `SchemasReplace`；调用 `RestoreAndRewriteMetaKVFiles` 时又额外接收一个 `&SchemasReplace` 供最终逐表刷新。
- `RestoreMetaKVProcessor::RestoreAndRewriteMetaKVFiles`：恢复路径的公开编排入口。其 `hasExplicitFilter` 分支决定成功完成批处理后的 schema 同步策略。
- `RestoreMetaKVProcessor::ProcessBatch`：薄适配器，把 trait 参数、持有的替换规则及两个回调转交给 `LogClient::RestoreBatchMetaKVFiles`。
- `pub struct MetaKVInfoProcessor<'a>`：持有 `LogClient` 的独占可变借用，以及 `LogBackupTableHistoryManager`、`TableMappingManager` 两份会随扫描更新的状态。
- `NewMetaKVInfoProcessor`：通过 `NewTableHistoryManager` 和 `NewTableMappingManager` 初始化两个管理器。
- `MetaKVInfoProcessor::ReadMetaKVFilesAndBuildInfo`：只读扫描的公开编排入口；分批成功后调用 `CleanTempKV`。
- `MetaKVInfoProcessor::ProcessBatch`：先调用 `filterAndSortKvEntriesFromFiles`，再按排序结果依次调用 `ParseMetaKvAndUpdateIdMapping`，最后返回尚未达到 `filterTS` 的条目。
- `GetTableMappingManager` / `GetTableHistoryManager`：只读借用访问器，不转移也不克隆管理器状态。

文件没有模块级常量、宏或条件编译项；批大小等策略位于 `client.rs`。所有符号沿用 Go 风格大写命名，crate 根通过 `#![allow(non_snake_case, ...)]` 接受这种迁移期接口。

## 执行流程

恢复重写路径如下：

1. 调用 `RestoreAndRewriteMetaKVFiles` 后，先执行 `LogClient::RunGCRowsLoader`。当前 Rust 实现仅把 `gcLoaderStarted` 设为 `true`，并未启动 Go 侧对应的真实异步加载。
2. `SeparateAndSortFilesByCF` 丢弃不可读或非 meta 文件，把空 CF 视作 default，只保留 default/write 两类，并分别按 `MinTs`、`MaxTs`、`ResolvedTs` 排序。
3. `LoadAndProcessMetaKVFilesInBatch` 按 default 文件时间范围和 `MetaKVBatchSize` 切批；每次 default 边界出现时，先回调 default，再处理 `MinTs` 小于该边界的 write 文件。两个 CF 最终都以 `filterTS = u64::MAX` 冲刷一次，即使文件列表为空也会调用。
4. trait 回调进入 `RestoreMetaKVProcessor::ProcessBatch`，再进入 `LogClient::RestoreBatchMetaKVFiles`。当前后者只对已有 `entries` 按 `Ts < filterTS` 分流和按 `(Ts, Key)` 排序，更新统计、按文件调用进度回调，并返回遗留条目；读取文件、重写 meta KV 和 raw KV 写入尚未实现。
5. 所有批次成功后，无显式过滤时调用 `UpdateSchemaVersionFullReload`；有显式过滤时调用 `RefreshMetaForTables`。当前 Rust 两者均只记录日志并返回成功。

信息预扫描路径如下：

1. `ReadMetaKVFilesAndBuildInfo` 同样先调用 `SeparateAndSortFilesByCF` 和 `LoadAndProcessMetaKVFilesInBatch`。
2. 每批由 `MetaKVInfoProcessor::ProcessBatch` 过滤、排序，逐项把 `Entry`、CF、时间戳和历史收集器传入 `TableMappingManager::ParseMetaKvAndUpdateIdMapping`。
3. 批框架返回的 `filteredEntries` 被带入后续批次，尾批以最大时间戳冲刷。
4. 全部批次成功后调用 `CleanTempKV`，再返回成功。当前本 crate 实际导入的是 `stubs.rs` 中的 `TableMappingManager`，其解析和清理方法均为空操作/直接成功，因此尚不能产生真实 ID 映射或重命名历史。

## 数据与状态

- `DataFileInfo` 是批处理输入的文件元数据；本文件不拥有文件持久状态。分 CF 和排序会克隆符合条件的文件描述。
- `KvEntryWithTS` 将一条 KV `Entry` 与时间戳绑定。`entries` 是跨批状态：`Ts < filterTS` 的条目进入当前批，其他条目原样结转。尾部 `u64::MAX` 冲刷意味着除时间戳恰等于最大值的条目外都应进入当前批；这种严格小于关系来自 `client.rs::filterAndSortKvEntriesFromFiles`。
- `RestoreMetaKVProcessor` 生命周期参数 `'a` 把处理器限制在其所借用 `LogClient` 的生命周期内；同时 `&mut LogClient` 保证该处理器存在时不能由别处并发可变访问 client。
- `schemasReplace` 是恢复处理器的长期状态；构造时保存的值供每批恢复使用，而 `RestoreAndRewriteMetaKVFiles` 的同名借用参数仅用于最终 `RefreshMetaForTables`。调用者必须保证两者表达同一恢复会话，否则批内重写规则和收尾刷新范围可能不一致；类型系统不会替调用者验证这一语义不变量。
- `MetaKVInfoProcessor` 的两个 manager 跨越所有 CF 和批次存活。访问器只返回共享引用，避免外部绕过处理器修改收集中状态。
- 两个回调为 `FnMut`，允许闭包内部累积统计/进度；`Send` 只表示闭包可在线程间转移，不表示本文件会创建线程或并行调用它们。

## 依赖与调用关系

上游与模块边界：

- `lib.rs` 公开挂载并再导出本模块。`Cargo.toml` 声明该 crate 对 stream、restore、checkpoint、split、utils 等本地 crate 的依赖，但本文件的实际类型路径主要经 `crate::client`、`crate::log_file_manager`、`crate::stubs` 到达，并没有直接引用外部 crate 名。
- RustCodeGraph 文件关系给出 `client.rs`、`client_test.rs`、`parity_test.rs` 三个使用者。`client.rs::LoadAndProcessMetaKVFilesInBatch` 以 `&mut dyn BatchMetaKVProcessor` 反向调用本文件的 trait 实现；测试文件提供自定义 processor 验证框架契约。
- Rust 全仓搜索只发现 `parity_test.rs` 构造具体处理器；`br/pkg/task/stream.rs` 虽依赖 `client.rs`，当前没有调用本文件公开构造器。Go 的 `br/pkg/task/stream.go` 才包含真实上游：预恢复阶段构造 `MetaKVInfoProcessor` 并读取 DDL files，恢复 meta files 阶段构造 `RestoreMetaKVProcessor` 并执行恢复。

主要下游：

- `SeparateAndSortFilesByCF` 和 `LoadAndProcessMetaKVFilesInBatch`（`client.rs`）提供文件筛选、排序、批次交错与尾批冲刷。
- `LogClient::{RunGCRowsLoader, RestoreBatchMetaKVFiles, UpdateSchemaVersionFullReload, RefreshMetaForTables, filterAndSortKvEntriesFromFiles}` 承担恢复准备、批处理、收尾和条目过滤；其中多项当前有明确桩注释。
- `TableMappingManager::{ParseMetaKvAndUpdateIdMapping, CleanTempKV}` 与 `LogBackupTableHistoryManager` 当前来自 `stubs.rs`，不是 `br/pkg/stream/table_mapping.rs` 中更完整的 canonical Rust 实现。这是后续接线时需要优先核对的边界。

## 错误处理与边界

- 两条公开主流程均使用 `?` 原样传播 `Result` 错误；任一批处理、过滤/排序、映射解析或 schema 收尾失败都会立即停止，后续步骤不执行。
- `RestoreAndRewriteMetaKVFiles` 在批处理失败时不会执行 schema reload/refresh；`ReadMetaKVFilesAndBuildInfo` 在批处理失败时不会执行 `CleanTempKV`。因此后者当前没有 finally/RAII 清理保证；若未来 manager 真正持有临时 KV，错误路径是否需要清理必须单独设计并测试。
- 空输入不是错误。批框架仍依次回调 default 和 write 两个空尾批；Rust `client_test.rs::test_meta_kv_batch_flushes_both_empty_cfs` 和 Go `TestRestoreMetaKVFilesWithBatchMethod1` 都验证了该契约。
- `SeparateAndSortFilesByCF` 会静默忽略非 meta、不可读、未知 CF 文件；空 CF 按 default 处理。调用方不能用输入文件数推断实际处理文件数。
- `filterTS` 使用严格 `<`。等于边界时间戳的条目必须保留给后续批次，以维持 default/write 之间的时序关系。
- 当前恢复路径的成功有重大能力边界：`RestoreBatchMetaKVFiles` 不读取文件且不写 raw KV，schema 更新方法是日志桩；信息路径的映射解析也是桩。因此现阶段 `Ok(())` 只证明编排没有报错，不证明集群数据或映射已实际更新。
- 本文件没有 panic、显式重试或错误包装；相比 Go 使用 `errors.Trace`/`errors.Annotate`，Rust 错误上下文完全依赖下游返回的 `crate::stubs::Result`。

## 并发与资源生命周期

本文件自身不创建线程、异步任务、锁或 channel，两个主流程都是同步串行执行。default/write 批次的交错顺序由 `LoadAndProcessMetaKVFilesInBatch` 控制，而不是并行处理；这对同一 meta 事务在两个 CF 中的依赖关系至关重要。

`&mut self` 与内部 `&'a mut LogClient` 使同一处理器调用在 Rust 类型层面互斥。闭包标记 `Send` 是可转移约束，不引入共享并发；若未来把批次并行化，还需重新审查 `FnMut`、manager 可变状态、条目顺序以及 `SchemasReplace` 的线程安全性，不能仅凭 `Send` 判定可并行。

资源收尾方面，恢复路径只“启动” GC loader 而不在本文件中等待或关闭；当前 Rust 方法实际上仅设布尔标志。信息路径仅在成功完成所有批次后清理 manager 临时 KV。处理器析构时借用自然释放，回调和 manager 随结构体一同 drop，没有显式外部资源关闭逻辑。

## 与 Go 版本的对应关系

Rust 的 trait、两个 processor、构造器、两条主流程、两个 getter 以及批回调参数顺序都逐项对应 `batch_meta_processor.go`。恢复路径的控制顺序也一致：GC rows loader → CF 分组排序 → 分批处理 → 根据 `hasExplicitFilter` 选择全量 reload 或逐表 refresh；信息路径同样在成功扫描后 `CleanTempKV`。

需要注意的差异和迁移缺口：

- Go `RestoreMetaKVProcessor` 保存 `*SchemasReplace`，Rust 按值保存；两者都另外向主流程传递 schema replacement 用于最终刷新，但 Rust 更容易出现“持有值”和“参数引用”不是同一实例的调用错误。
- Go 的生产调用已在 `br/pkg/task/stream.go` 接线：构建 id mapping、保存 manager、恢复 meta files 并生成 rewrite rules。Rust 生产调用尚未出现，只有 `parity_test.rs` 的空输入接线冒烟。
- Go `LogClient::RestoreBatchMetaKVFiles` 对接真实读取、重写和恢复能力；Rust `client.rs` 明确写明 rawkv put 是 stub。Rust 的全量 schema reload、逐表刷新、GC loader 也未展开。
- Go `MetaKVInfoProcessor` 使用 `br/pkg/stream` 的真实 `TableMappingManager`；本文件当前使用 `crate::stubs::stream`，其 `ParseMetaKvAndUpdateIdMapping` 与 `CleanTempKV` 不产生实际效果。仓库虽有 `br/pkg/stream/table_mapping.rs` 及独立测试，但尚未从这里接入。
- Go 文档明确列出重命名历史和 upstream→downstream ID mapping 两项业务目的；Rust 类型结构保留了这两个 manager，却由于上述桩尚未实现同等效果。

## 扩展指南

- 若补齐真实恢复，最可能修改的是 `LogClient::RestoreBatchMetaKVFiles` 及其文件读取/重写/写入依赖，而不是把逻辑堆入 `RestoreMetaKVProcessor::ProcessBatch`。保持 processor 作为策略适配层，避免复制批框架。
- 若补齐 ID 映射，应优先把本文件从 `crate::stubs::stream` 接到 canonical stream crate/实现，并验证方法签名差异；必须同步 `br/pkg/stream/table_mapping_test.rs`、`rewrite_meta_rawkv_test.rs`，以及本目录针对 processor 的独立测试，不能只依赖空输入冒烟。
- 若改变切批或 CF 时序，应修改 `client.rs::LoadAndProcessMetaKVFilesInBatch`，并同步 Rust `client_test.rs` 中空 CF、尾批、遗留 entries 和批次数测试，以及 Go `client_test.go` 的 `TestRestoreMetaKVFilesWithBatchMethod1` 至 `6` 语义。
- 若改变 `hasExplicitFilter` 行为，需要分别覆盖 `UpdateSchemaVersionFullReload` 和 `RefreshMetaForTables` 的成功/失败分支，并验证失败不会被吞掉。当前测试没有直接覆盖这两个 processor 收尾分支，应新增同目录独立 `*_test.rs` 或扩展已有 `parity_test.rs`/`client_test.rs`，不要把测试内嵌回生产源文件。
- 若消除构造时和方法参数中的双份 `SchemasReplace`，需同时调整 Go 对照或明确记录 Rust API 偏差，并核查 `br/pkg/task` 的未来 Rust 接线；这是兼容性变更而非局部重命名。
- 性能敏感点在跨批 `Vec<KvEntryWithTS>` 所有权传递、文件元数据克隆、排序和 manager 逐条解析。优化时必须保留严格时间边界、确定性 `(Ts, Key)` 顺序及 default/write 依赖，不能用无序并行替代。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter br/pkg/restore/log_client` 确认目标、Go 对照及测试文件；`node --file br/pkg/restore/log_client/batch_meta_processor.rs` 读取完整 194 行并报告其被 `client.rs`、`client_test.rs`、`parity_test.rs` 使用；`node` 还核对了 `client.rs` 的批框架、下游桩，以及 `stubs.rs` 的 manager 桩。按符号执行的 `query RestoreAndRewriteMetaKVFiles`、`query ReadMetaKVFilesAndBuildInfo` 返回空结果，因此调用点用文件关系和精确 `rg` 补证。
- 源码：`br/pkg/restore/log_client/batch_meta_processor.rs`、`client.rs`、`lib.rs`、`stubs.rs`。
- crate 配置：`br/pkg/restore/log_client/Cargo.toml`，确认 library crate、Go package 元数据及本地依赖边界。
- Go 对照与生产入口：`br/pkg/restore/log_client/batch_meta_processor.go`、`client.go`、`br/pkg/task/stream.go`。
- Rust 测试：`br/pkg/restore/log_client/client_test.rs` 验证空输入双 CF 冲刷、批回调统计/进度及基本批次数；`parity_test.rs` 验证两个具体 processor 的空输入接线和 getter。目标文件没有内嵌测试，也没有同名独立 `batch_meta_processor_test.rs`。
- Go 测试：`br/pkg/restore/log_client/client_test.go` 的 `TestRestoreMetaKVFilesWithBatchMethod1` 至 `6` 覆盖空 CF、时间范围、大小阈值、两 CF 交错及跨批 entries；`TestRestoreBatchMetaKVFiles` 覆盖空批。
- 相关 canonical manager 测试：`br/pkg/stream/table_mapping_test.rs`、`rewrite_meta_rawkv_test.rs`（以及对应 Go 测试）证明完整映射逻辑的预期测试面，但不能证明本文件当前已接入该实现。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证仅执行任务指定的 11 标题结构检查，并人工复核唯一产物、源码链接、当前桩边界和无“已完整支持”臆断。
