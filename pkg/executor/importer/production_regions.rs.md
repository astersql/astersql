# `pkg/executor/importer/production_regions.rs`

## 文件定位

本文件属于 `astersql-executor-importer` crate；crate 根在 `pkg/executor/importer/lib.rs` 中声明并公开重导出 `production_regions`。它不是通用导入算法的主体，而是生产环境适配层：`HostTableImporterService` 包装宿主提供的 `TableImporterService`，只接管需要生产存储/元服务语义的 Region 划分和 allocator rebase，其余方法保持委托。直接装配入口是 `pkg/dxf/importinto/scheduler.rs` 的 `ImportSchedulerRuntime::FromEncodeRuntime`，其中 `ImporterService` 工厂把原服务包装成该类型。

`pkg/executor/importer/Cargo.toml` 将该 crate 映射到 Go 包 `pkg/executor/importer`，并直接依赖 `astersql-lightning-mydump`、`astersql-lightning-common`、`astersql-lightning-backend*` 与 `astersql-metaservice`；这些依赖分别承载文件切分、allocator 通用逻辑、导入后端接口和 keyspace-aware etcd 访问。

## 核心职责

1. `LocalRegionStorage` 将本机父目录实现为 `mydump::Storage`，让 mydump 的 CSV Region 划分以流式 `Read` 读取 server-disk 文件，而不是先把文件载入内存。
2. `HostTableImporterService::MakeTableRegions` 仅对绝对路径走本文件的本地文件切分；非绝对路径回退到宿主服务，保留云存储或宿主自定义路径的既有行为。
3. `HostTableImporterService::RebaseAllocatorBases` 从宿主取得 metadata store、PD 地址、etcd 拨号配置和 allocator bindings，再交给 `RebaseAllocatorsWithMetadata`，使导入后的 ID rebase 使用正确的元服务分组/命名空间。
4. `TableImporterService` 的其他方法逐项转发给 `Host`，因此包装器不会替换编码、后端、parser、配额、checksum 或统计刷新策略。

## 主要符号

- `LocalRegionStorage { parent: PathBuf }`：crate 内可见的本地目录句柄。其 `mydump::Storage::open(path, compression)` 拒绝任何非 `Compression::None` 输入，然后以 `parent.join(path)` 打开文件并返回 `Box<dyn Read + Send>`。
- `HostTableImporterService { Host: Arc<dyn TableImporterService> }`：公开生产适配器。`Arc` 使包装器与 DXF runtime 共享同一宿主服务；类型本身不保存可变任务状态。
- `HostTableImporterService::MakeTableRegions`：绝对 server-disk 路径的特化入口。它从 `LoadDataController` 提取表元信息、数据文件、CSV/字符集/并发配置，调用 `mydump::MakeTableRegions`，再把 mydump Region 投影为 importer 的 `TableRegion`。
- `HostTableImporterService::RebaseAllocatorBases`：生产 rebase 桥接点；缺少 `AllocatorMetadataStore` 时立即返回 `"TiKV store does not expose PD client"`。
- `RebaseAllocatorsWithMetadata(...) -> Result<(), String>`：公开、可单测的元服务感知 rebase 函数。它创建 namespaced etcd client、构造 Lightning `TableInfo` 和 allocator base 映射、调用 `common::RebaseTableAllocators`，最后关闭 client 并重置 allocator 连接。

## 执行流程

Region 划分主链如下。

1. `pkg/dxf/importinto/scheduler.rs` 创建 `HostTableImporterService`，并在准备导入计划时调用 `LoadDataController::PopulateChunks`。
2. `PopulateChunks`（`pkg/executor/importer/table_import.rs`）以调整后的每引擎大小调用 trait 方法 `MakeTableRegions`。
3. 本实现检查 `controller.Plan.Path`。若不是绝对路径，直接调用 `Host.MakeTableRegions`；若是绝对路径，则视作 server-disk 本地源。
4. 本地分支要求目标表至少有一列；随后将 `controller.DataFiles()` 转换成 `mydump::FileInfo`，保留路径、逻辑/实际大小、源类型、压缩方式、排序键和扩展列数据。
5. 它构造 `MDTableMeta` 与 `NewDataDivideConfig()`，设置列数、目标 engine 大小、线程数、严格切分、CSV 配置和字符集；字符集缺省为 `utf8mb4`。
6. `LocalRegionStorage` 以源路径的父目录为根，`mydump::MakeTableRegions` 逐段读取文件并生成 Region。结果仅复制 engine ID、文件元数据、offset 和 row-ID 边界到 `TableRegion`。
7. `PopulateChunks` 再按 engine ID 把 Region 展开为 `Chunk`，并补出索引 engine 槽位；这一步不在本文件内。

Allocator rebase 主链如下。

1. `pkg/executor/importer/table_import.rs::PostProcess` 先调用通用 `RebaseAllocatorBases`；只有表具备隐式 row ID、auto-increment 或 auto-random 时，才动态分派到 service。
2. 包装器取得 metadata store，并从运行配置的逗号分隔 `PDAddress` 形成 caller endpoints；随后调用 `RebaseAllocatorsWithMetadata`。
3. 后者要求 `plan.DesiredTableInfo` 存在，通过 `newEtcdClientForAllocatorRebase` 创建限定到存储 metadata group/keyspace 的 client。
4. 宿主闭包基于该 client 创建 `AllocatorRebaseBindings`。成功时，函数把三种 backend allocator 类型映射为 Lightning common 的 `RowID`、`AutoIncrement`、`AutoRandom`，并用计划中的 DB/table 信息执行 rebase。
5. 无论 bindings 或 rebase 成功与否，函数都会尝试关闭 client；若 bindings 已成功创建，还会调用 `ResetConnection`。最终返回 bindings/rebase 的原始结果，关闭失败只写入标准错误。

## 数据与状态

- Region 分支只创建局部值：`MDTableMeta`、`DataDivideConfig`、`LocalRegionStorage` 和结果向量；没有全局缓存或跨调用状态。
- `engine_data_size` 从 `i64` 转成 `f64` 传给 mydump；文件大小、offset 与 row-ID 边界保持 `i64`。扩展该处时应留意超大值的浮点精度，而不能假设转换完全无损。
- `LocalRegionStorage.parent` 是 `Plan.Path` 的父目录，数据文件的相对路径再通过 `join` 解析。代码当前不做 canonicalize 或目录逃逸检查，可信边界来自上游生成的 `DataFiles`。
- Rebase 分支中的 `maximum_ids` 是只读映射；它被转换成 common allocator 类型后消费。表能力位来自 `DesiredTableInfo`：`PKIsHandle`、`IsCommonHandle`、auto-increment、auto-random、separate-auto-increment 及 unsigned 属性。
- `NamespacedEtcdClient` 和 `AllocatorRebaseBindings` 均为调用期资源，不存入包装器。包装器长期持有的唯一字段是共享宿主 `Arc`。

## 依赖与调用关系

上游直接关系包括：

- `pkg/dxf/importinto/scheduler.rs::ImportSchedulerRuntime::FromEncodeRuntime` 构造包装器；同文件的计划准备路径调用 `PopulateChunks`。
- `pkg/executor/importer/table_import.rs::LoadDataController::PopulateChunks` 调用 `MakeTableRegions`；`PostProcess` 经通用 `RebaseAllocatorBases` 调用本包装器的 rebase 方法。
- `pkg/executor/importer/meta_service_group_test.rs` 直接调用 `RebaseAllocatorsWithMetadata` 验证 metadata group/keyspace 行为。
- `pkg/executor/importer/production_regions_test.rs` 直接使用 `LocalRegionStorage` 验证 CSV 分段读取。

下游直接关系包括：

- `astersql_lightning_mydump::{Storage, MakeTableRegions, NewDataDivideConfig}` 负责实际文件读取协议和 Region 切分算法。
- `crate::newEtcdClientForAllocatorRebase` 负责根据 metadata store、caller endpoints 与拨号配置选择 namespaced etcd 连接。
- `astersql_lightning_common::RebaseTableAllocators` 执行 allocator 类型选择及基址推进；本文件只适配表描述、ID 映射和连接生命周期。
- 所有未特化的 `TableImporterService` 方法均直接调用 `Host`，这是包装器保持行为透明的关键约束。

RustCodeGraph 将本文件列为被 `pkg/dxf/importinto/scheduler.rs`、`pkg/executor/importer/import.rs`、`pkg/executor/importer/meta_service_group_test.rs`、scheduler 测试及 Lightning stub 使用；对 trait 动态分派的精确 callers/callees 未生成边，因此上述运行链又由这些文件中的具体构造和调用点交叉核验。

## 错误处理与边界

- 压缩本地读取返回 `MydumpError::Io`，因为此 server-disk CSV 切分适配器只实现未压缩流；它不尝试透明解压。
- 本地切分在目标表无列、源路径无父目录、文件打开失败或 mydump 切分失败时返回错误。mydump 错误被字符串化，因而跨 trait 边界不保留结构化错误类型。
- 非绝对路径完全委托宿主；因此本文件不能作为云存储、相对路径或其他宿主存储失败语义的依据。
- Rebase 在 metadata store 缺失、目标表 metadata 缺失、etcd client 创建失败、bindings 创建失败或 common rebase 失败时返回 `Err(String)`。
- client `close()` 失败不会覆盖主要结果，只通过 `eprintln!` 报告；若 bindings 创建成功，`ResetConnection` 在主要结果为错误时也照常执行。若 bindings 创建失败则没有连接可重置。
- trait 外层的 `table_import.rs::RebaseAllocatorBases` 会对无 auto-ID 的表短路；直接调用本文件公开函数时不会获得该短路，调用者必须提供有意义的表 metadata 和 ID 集合。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁或通道。并发度只作为 `config.engine_concurrency = controller.Plan.ThreadCnt` 传递给 mydump；实际 worker 行为属于 `astersql-lightning-mydump`。

文件句柄由 `Storage::open` 返回的 boxed reader 所有，使用结束后按 Rust RAII 关闭。`Host` 通过 `Arc` 共享，要求 trait object 满足其 trait 定义的线程安全约束。Rebase 的 etcd client 则采用显式生命周期：创建 client，创建并使用 bindings，尝试 `close`，最后调用 `ResetConnection`；测试还验证 reset 闭包执行时 client 已关闭。当前关闭失败仅记录，属于有意的 best-effort 清理语义。

## 与 Go 版本的对应关系

最接近的 Go 实现位于 `pkg/executor/importer/table_import.go`。

- Go 的 `LoadDataController.PopulateChunks` 同样构造 `MDTableMeta`/`DataDivideConfig`、调用 `mydump.MakeTableRegions`，再按 engine ID 转成 chunks；Rust 把“生产 server-disk 的 Region 生成”拆到本包装器，通用的 Region-to-Chunk 转换仍在 `table_import.rs`。
- 两版都传递列数、调整后 engine 大小、线程并发、严格格式、字符集和 CSV 配置。Go 配置还显式包含 `MaxChunkSize`、I/O workers、invalid-character replacement、read block size、parquet row-count 策略和通用 data store；当前 Rust 本地分支使用 `NewDataDivideConfig` 的这些字段默认值，且 `TableRegion` 投影未携带 Go chunk 的 `ParquetMeta`。因此不能宣称本分支覆盖 Go 的全部格式/存储能力。
- Go 使用 controller 的抽象 `dataStore`；Rust 仅在绝对路径时用 `LocalRegionStorage`，其他路径交回宿主。这是适配边界差异，不是 mydump 算法差异。
- Go `RebaseAllocatorBases` 同样先判断 `TableHasAutoID`，创建元服务感知 etcd client，构造 auto-ID requirement，调用 `common.RebaseTableAllocators`，关闭 client 后 reset 连接。Rust 将 auto-ID 短路放在 `table_import.rs`，将 metadata group client 和 bindings 注入留在本文件。
- Rust 明确逐项构造 common `TableInfo` 和 allocator 类型映射；Go 直接传递 TiDB table info 与 `map[autoid.AllocatorType]int64`。两者都以 DB ID、目标表 metadata 和导入观测到的最大 ID 为 rebase 输入。

## 扩展指南

- 新增本地压缩格式支持应从 `LocalRegionStorage::open` 接入，并在独立的 `production_regions_test.rs` 增加解压、损坏流与大文件测试；不要把测试嵌入生产源文件。
- 新增 Region 划分参数时，应同步核对 Go `LoadDataController.PopulateChunks` 的 `DataDivideConfig` 字段，并修改 `HostTableImporterService::MakeTableRegions`。尤其要评估 CSV/Parquet 差异、默认值兼容性、超大 engine size 精度及内存/并发影响。
- 扩展路径类型时应保留“非本地来源委托宿主”的不变量，避免仅凭绝对/相对字符串误判对象存储 URI；相应测试应覆盖委托是否发生及本地父目录解析。
- 新增 allocator 类型时，需要同时更新 backend-to-common 的 `match`、Lightning common 实现及独立测试；当前穷尽匹配会在编译期提示新增 enum variant。
- 调整 rebase 资源清理时，应维持主要错误优先级，并在 `meta_service_group_test.rs` 验证 client 关闭先于 reset、keyspace namespace 正确、bindings/rebase 失败路径也清理资源。
- 若增加新的生产特化方法，优先只覆盖必要方法，其余继续透明委托 `Host`，并在 scheduler 构造路径及 `TableImporterService` trait 处同步检查。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；目标 `production_regions.rs` 被识别为 28 个符号，并显示 5 个使用文件。
- RustCodeGraph 读取/查询：`production_regions.rs`、`production_regions_test.rs`、`lib.rs`、`table_import.rs`、`pkg/dxf/importinto/scheduler.rs`、`pkg/dxf/importinto/subtask_executor.rs`、`meta_service_group_test.rs`；精确查询确认 `HostTableImporterService`、`RebaseAllocatorsWithMetadata` 及 trait 方法定义。trait 动态分派的 callers/callees 查询无输出，已用精确引用搜索补足。
- crate/装配依据：`pkg/executor/importer/Cargo.toml`、`pkg/executor/importer/lib.rs`。
- Go 对照：`pkg/executor/importer/table_import.go` 的 `PopulateChunks`、`PostProcess`、`RebaseAllocatorBases` 和 `newEtcdClientForAllocatorRebase`。
- 独立 Rust 测试：`production_regions_test.rs::local_region_storage_streams_large_csv_split` 验证未压缩 CSV 可流式切成多个连续 Region，首 offset 为 0、末 end offset 等于文件长度；`meta_service_group_test.rs::allocator_rebase_consumes_scoped_discovery_and_resets_after_close` 验证 scoped namespace、rebase 值和 close-before-reset（该用例需真实 etcd endpoint，源码标记为 ignored）。
- 人工复核结论：本文件存在的原因是把 DXF 宿主服务适配到 server-disk Region 切分和 metadata-group-aware allocator rebase；安全扩展点分别是 `Storage::open`、`MakeTableRegions` 配置/投影、allocator 类型映射和显式清理顺序。
