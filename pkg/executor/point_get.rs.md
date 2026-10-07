# `pkg/executor/point_get.rs`

## 文件定位

该文件属于 `astersql-executor` crate；crate 根在 `pkg/executor/lib.rs`，通过 `pub mod point_get` 公开本模块，Cargo 包声明和 Go 包映射分别见 `pkg/executor/Cargo.toml` 的 `[package]`、`[lib]` 与 `[package.metadata.porting]`。它移植并抽象了 Go `pkg/executor/point_get.go` 的单行精确读取语义：由主键或唯一索引定位一条记录，处理分区、事务可见性、悲观锁、行解码和执行统计。

需要区分“已实现语义”和“当前主运行时接线”。本文件提供完整的 `buildPointGet`、`PointGetExecutor::{Open,Next,Close}` 和依赖边界，但仓库内 Rust 调用搜索显示 `buildPointGet` 没有生产调用者；当前会话路径 `pkg/session/runtime/scan_adapter_runtime.rs::BuildPointGetExecutor` 构建的是 `pkg/executor/builder.rs::BuildTypedPointGet` 和 `pkg/executor/typed_point_get.rs::TypedPointGet`。本文件当前直接服务于模块级测试、共享的快照统计类型及 Go 语义对照，不能把它描述为现役 SQL 请求主链的唯一执行器。

## 核心职责

- `buildPointGet` / `build_point_get_inner` 把 `PointGetPlan` 转成 `BuiltPointGet`：先校验表是否可读和分区裁剪，再取得快照、初始化执行器、包装缓存表快照，并向 builder 标记 TiKV 与锁语句状态。
- `PointGetExecutor::Next` 保证单次输出：可从公共主键、唯一索引或已有 handle 得到行键，按隔离级别决定是否锁不存在的索引键或仅锁已存在键，然后读取并解码一行。
- `get` 定义读优先级：事务内存缓冲区、悲观锁缓存、读锁表的 point-get 联合缓存，最后才是带超时的 `Snapshot::get`。
- `DecodeRowValToChunk`、`decodeOldRowValToChunk`、`tryDecodeFromHandle` 同时覆盖新旧行格式、整型主键、公共 handle、前缀主键、默认值和虚拟列占位。
- `fillRowChecksum` 只对新行格式计算行校验和，补齐 handle 列和缺失列默认值后按列 ID 排序，旧格式输出 `Null`。
- `Open` / `Close` 管理事务作用域校验、TopSQL 快照选项、索引用量和快照运行时统计的注册与汇总。

## 主要符号

- 常量与基础类型：`GLOBAL_TXN_SCOPE`、`EXTRA_HANDLE_ID`、`EXTRA_ROW_CHECKSUM_ID`、`PointGetResult`、`Key`；特殊列 ID 决定 handle 与校验和列的填充路径。
- 错误模型：`PointGetErrorKind::{General,NotFound,InvalidPlacementPolicy,LookupInconsistent}` 和 `PointGetError`。内部以 `is_not_found` 将“无行”从执行失败中分离。
- 计划与元数据：`PointGetPlan`、`TableInfo`、`IndexInfo`、`PartitionInfo`、`Schema`、`ColumnInfo`、`Handle`、`Datum`。它们是 Go 元数据/计划对象的局部、可注入表示，而不是直接使用完整 planner/model 类型。
- I/O 与注入接口：`Snapshot`、`Transaction`、`RowDecoder`、`DatumDecoder`、`IndexUsageReporter`、`PointGetDependencies`、`PointGetBuilder`。其中 `PointGetDependencies: Send + Sync` 汇集编码、锁、缓存、placement、解码和统计能力。
- 构建结果：`BuiltPointGet::{TableDual,Executor,Failed}`；裁剪为空返回 `TableDual`，依赖错误由 builder 记录后返回 `Failed`。
- 核心执行器：`PointGetExecutor` 保存计划元数据、当前 handle/索引键、事务与快照、锁配置、一次性 `done` 状态、解码器、虚拟列元数据和统计对象。
- 辅助函数：`GetPhysID`、`matchPartitionNames`、`shouldFillRowChecksum`、`fillRowChecksum`、`DecodeRowValToChunk`、`decodeOldRowValToChunk`、`tryDecodeFromHandle`、`notPKPrefixCol`、`getColInfoByID`。
- 统计类型：`SnapshotRuntimeStats` 与 `runtimeStatsWithSnapshot`；后者实现 `astersql_util_execdetails::execdetails::RuntimeStats`，支持深拷贝、合并、字符串展示和类型标识。

## 执行流程

1. `buildPointGet` 调用 `PointGetBuilder::validate_readable_table`；失败时写入 builder 错误。随后 `prune_partitions` 若判定无分区可读，直接返回 `TableDual`。
2. 对首次加锁 point get，函数临时设置 `in_select_lock_statement`，调用内部构建后恢复原值。`build_point_get_inner` 标记语句走 TiKV，取得 `Snapshot` 和依赖对象，构造 `PointGetExecutor` 并调用 `Init`。
3. `Init` 创建行解码器、复制计划字段、重置 `done`。临时表强制关闭悲观锁；普通表继承计划锁参数。随后构建并排序虚拟列索引，配置自适应副本读、读副本作用域、运行时统计和索引名。缓存表再由 builder 包装快照。
4. `Open` 获取事务，调用 `verifyTxnScope` 校验逻辑表或物理分区是否允许被当前事务作用域读取，然后向快照设置 TopSQL 选项。
5. `Next` 先清空结果并将 `done` 置真，因此同一次打开周期最多产出一行。若计划带唯一索引：公共 handle 读直接由索引值编码构造 handle；普通唯一索引先编码索引键，再按 `pessimistic_read_consistency` 和 `lock` 选择“先锁再读”“存在才锁”或普通读取。
6. 普通唯一索引命中后解码 handle，执行可重复读 failpoint/屏障。若为全局索引，还从索引值取得物理分区 ID，并应用显式分区名过滤及 `ids_in_ddl_to_ignore` 过滤。
7. 执行器编码行键并通过 `getAndLock` 取行。索引已命中但行缺失时，非公共-handle 且非弱一致读会调用 `report_lookup_inconsistent`；此后无结果返回，而不是输出空行。
8. 有行时，`DecodeRowValToChunk` 选择新格式 `RowDecoder` 或旧格式解码路径；随后 `fillRowChecksum` 填充校验和列，最后按定义顺序计算虚拟列。
9. `Close` 停止快照统计收集，按索引或 handle 上报请求数和实际行数，重置 `done`，注册运行时统计，并合并 scan detail 与 TiKV CPU 时间。

## 数据与状态

`PointGetExecutor` 是可复用但有状态的执行器。`Init`/`Recreated` 必须重置会随计划或语句变化的字段；`done` 是每次打开/关闭周期的单行闸门，`Close` 将其恢复为 `false`。`index_key` 与 `handle_value` 既保存两阶段“索引到 handle、handle 到行”的中间结果，也用于悲观锁缓存回填。

`partition_definition_index` 表示计划选择的本地分区；`GetPhysID` 在有有效分区元数据时返回物理 ID，否则回退到逻辑表 ID。全局索引则在 `Next` 中从索引值动态改写 `table_id`。`partition_names` 是用户显式分区限制，空列表代表不过滤，匹配使用 ASCII 大小写不敏感比较。

`Chunk` 按列保存 `Datum`。旧行格式先用 schema 列 ID 建立位置表；虚拟列表面先填 `Null`，整型 PK、额外 handle 列和可完整恢复的 common-handle 主键列优先由 handle 填充，其余从行值或列的原始默认值取得。前缀主键与 `needs_restored_data` 列不能只凭 handle 恢复。

`SnapshotRuntimeStats` 记录 Get RPC 数、处理时间、说明、read-pool 细节与 scan detail。`runtimeStatsWithSnapshot` 用 `Arc<Mutex<_>>` 共享统计；`Clone` 是锁内数据的深拷贝，`Merge` 使用各统计类型自身的累加语义。锁中毒时字符串/克隆路径降级为空或缺失，而 `Close` 的关键上报路径返回明确错误。

## 依赖与调用关系

构建侧的本地调用链为 `buildPointGet -> build_point_get_inner -> PointGetExecutor::Init`。执行侧为 `Open -> verifyTxnScope`，`Next -> {GetPhysID, lockKeyIfNeeded/lockKeyIfExists, get, DecodeRowValToChunk, fillRowChecksum, fill_virtual_columns}`，结束侧为 `Close -> {IndexUsageReporter, register_runtime_stats, merge_scan_detail, merge_tikv_cpu_time}`。RustCodeGraph 对 `buildPointGet` 的 callee 查询确认了表校验、分区裁剪、错误记录、锁状态切换和内部构建这些边；对 `DecodeRowValToChunk` 的查询确认了新格式解码与 `decodeOldRowValToChunk` 分支。

外部能力主要通过 `PointGetDependencies` 注入，因此本文件不直接绑定真实 KV 客户端。Cargo 中与本文件直接可见类型对应的依赖包括路径依赖 `astersql-kv`、`astersql-util-execdetails`，以及启用 `failpoints` feature 的 crates.io `fail`。`pkg/executor/lib.rs` 公开 `point_get`，并用独立模块 `point_get_test.rs` 装配 Rust 测试，符合源文件与测试分离约束。

上游生产接线存在迁移边界：RustCodeGraph/文本调用搜索没有找到 `buildPointGet` 的生产调用；`pkg/session/runtime/scan_adapter_runtime.rs::BuildPointGetExecutor` 当前调用 `astersql_executor::builder::BuildTypedPointGet`，缓存的也是 `typed_point_get::TypedPointGet`。本文件的 `runtimeStatsWithSnapshot` 则仍被该运行时用于注册 point-read 统计。因此新增真实 SQL 行为前，必须先判断应修改本文件的 Go 语义模型，还是修改现役 typed point-get 链路，不能假设二者自动同步。

## 错误处理与边界

- “键不存在”使用 `PointGetErrorKind::NotFound` 表示，多数查找点将它转成正常的零行结果；编码、解码、锁、元数据或快照错误则通过 `?` 向上传播。
- 分区索引越界在 `GetPhysID` 中使用 `expect`，意味着调用方必须保证计划索引与当前分区定义一致；但表已不再分区时会回退逻辑表 ID，这一点由 Rust 测试覆盖。
- 全局索引没有分区元数据、旧行缺少列元数据、common handle/整型 handle 形态不符、校验和范围越界、解码器未初始化等都返回带上下文的 `PointGetError`。
- `verifyTxnScope` 对空作用域和 `global` 快速放行；其他作用域按物理 ID 校验，并分别生成表或表分区的 `InvalidPlacementPolicy` 错误。
- 索引命中但行缺失是潜在一致性故障。仅在普通唯一索引且非弱一致读时报告，公共 handle 读或弱一致读不触发该报告。
- 旧行格式不计算行校验和而写入 `Null`；校验和输入按列 ID 排序，避免表列存储顺序造成结果不稳定。
- 空 key 的 `get` 返回 `NotFound`，`lockKeyBase` 则直接返回无值；最大执行时间在加锁前检查，并作为快照 Get 的可选超时。

## 并发与资源生命周期

`PointGetDependencies` 要求 `Send + Sync`，`Snapshot`、`Transaction` 和各解码/上报 trait 要求 `Send`；执行器本身持有可变状态，API 依赖调用方串行执行 `Init/Open/Next/Close`，文件内没有为同一执行器提供并发 `Next` 的同步保护。

悲观锁生命周期集中在 `lockKeyBase`：加锁前检查执行超时，按单键构造 `LockContext`，调用 `do_lock_keys`，再把 `values_not_locked` 以及已取得的索引值写回悲观锁缓存。`lock_only_if_exists` 为真时从锁上下文直接取值；若标为 `already_locked`，再走统一 `get` 路径。锁的提交、回滚和释放不由本文件负责，而由注入的事务/会话实现管理。

快照由 builder 创建并由执行器拥有。`Init` 可给它挂载共享统计，`Open` 配置 TopSQL，`Close` 先停止收集再汇总指标。统计通过 `Arc<Mutex<_>>` 可共享，但所有锁持有时间仅覆盖读取、合并或克隆；代码不在锁内执行 KV I/O。缓存表快照包装发生在构建期，普通读取时只有满足读/只读表锁且 point-get cache 开启才走 `table_cache_union_get`。

可重复读 failpoint 位于唯一索引已解析 handle、第二次行 Get 之前，用来复现并发更新窗口；真实语义屏障由 `repeatable_read_point_get_barrier` 注入。Rust failpoint 测试只确认注入点可启用，完整并发场景来自 Go 测试/Go 实现语义。

## 与 Go 版本的对应关系

Rust 的 `buildPointGet` 对应 Go `executorBuilder.buildPointGet`；`PointGetExecutor` 字段及 `Recreated`、`Init`、`Open`、`Close`、`Next`、锁/读取辅助函数、事务作用域校验和行解码辅助函数均在 `pkg/executor/point_get.go` 有同名或直接对应实现。核心顺序保持一致：表/分区校验，快照初始化，索引到 handle，failpoint，全局索引分区过滤，行 Get，解码、校验和、虚拟列，最终统计汇总。

Rust 用局部数据结构和 `PointGetDependencies`/`PointGetBuilder` trait 代替 Go 的 `sessionctx.Context`、`kv.Transaction`、`kv.Snapshot`、planner/model 和表编解码包，错误也压缩为四类 `PointGetErrorKind`。这使语义可单测，但不等价于已经连接全部真实基础设施。Go 的执行器直接实现现役 `exec.Executor`；Rust 当前生产会话路径改走 typed point-get，这是最重要的接线差异。

测试意图也不完全等量。`pkg/executor/point_get_test.rs` 目前覆盖物理分区 ID/分区名匹配，以及 read-pool/scan 统计的克隆合并；`pkg/executor/point_get_test.go` 还覆盖 GC 可见性、悲观锁缓存返回值、表缓存与分区缓存、RC/非 RC 下已存在和不存在键的锁行为、历史快照等端到端行为。因此不能用现有 Rust 小型测试声称 Go 的全部 point-get 行为已回归。

## 扩展指南

- 修改定位/锁顺序时，优先审查 `PointGetExecutor::Next`、`getAndLock`、`lockKeyBase` 和 `get`，保持 Go 中隔离级别、空键、索引键与行键的先后关系；同时检查现役 `typed_point_get.rs` 是否需要等价变更。
- 增加索引或分区能力时，更新 `PointGetPlan`、`IndexInfo`/`PartitionInfo`、`PointGetDependencies` 的编码解码接口，并覆盖全局索引、显式分区名、DDL 忽略分区和逻辑/物理 ID 的组合。
- 扩展行类型时，分别核对新格式 `RowDecoder`、旧格式 `decodeOldRowValToChunk`、`tryDecodeFromHandle` 和 `fillRowChecksum`；不得让前缀主键或需要 restored data 的列错误地仅从 handle 还原。
- 扩展统计时，保持 `SnapshotRuntimeStats::{clone,merge}`、`runtimeStatsWithSnapshot` 的 trait 实现和 `Close` 汇总一致；scan detail 不应被当作 cop-task 次数重复计算。
- 新增回归测试应放在独立测试文件，而不是内嵌进 `point_get.rs`。局部纯逻辑优先扩展 `pkg/executor/point_get_test.rs`；真实 SQL、事务与并发行为应同步参考 `pkg/executor/point_get_test.go`，并在现役 typed 路径的独立测试中补证据。
- 若要把 `buildPointGet` 接回生产主链，需要先实现真实的 `PointGetBuilder`/`PointGetDependencies` 适配并证明与 `BuildTypedPointGet` 的职责边界；在此之前不要删除 typed 路径或把本文件的接口存在当作接线完成。

## 验证依据

- 源码事实：`pkg/executor/point_get.rs`，重点符号为 `buildPointGet`、`PointGetExecutor::{Init,Open,Next,Close,getAndLock,lockKeyBase,get,verifyTxnScope}`、`fillRowChecksum`、`DecodeRowValToChunk` 和 `decodeOldRowValToChunk`。
- 模块与依赖：`pkg/executor/lib.rs` 的 `pub mod point_get`/独立 `point_get_test` 声明；`pkg/executor/Cargo.toml` 的 `astersql-executor` crate、Go 包映射、`astersql-kv`、`astersql-util-execdetails` 与 `fail` 依赖。
- Go 对照：`pkg/executor/point_get.go` 的构建、生命周期、索引/锁/读取、解码与统计实现；`pkg/executor/point_get_test.go` 的可见性、锁缓存、表缓存、分区、隔离级别和历史快照场景。
- Rust 测试：`pkg/executor/point_get_test.rs` 的分区 ID 回退、分区名大小写不敏感匹配、read-pool 统计与 scan detail 合并；额外直接引用见 `pkg/executor/temporary_table_test.rs` 和 `pkg/executor/executor_failpoint_test.rs`。
- RustCodeGraph：索引状态为 11,467 文件、307,296 节点、1,848,419 边；文件节点列出 `point_get_test.rs` 与会话统计消费者。`query PointGetExecutor/buildPointGet/DecodeRowValToChunk/fillRowChecksum/verifyTxnScope` 消除了 Go/Rust 同名歧义；`callees` 验证 `buildPointGet -> build_point_get_inner` 及 builder 方法边，和 `DecodeRowValToChunk -> {RowDecoder::decode_to_chunk, decodeOldRowValToChunk}` 边。调用搜索同时确认生产会话当前由 `scan_adapter_runtime.rs::BuildPointGetExecutor -> builder::BuildTypedPointGet` 接线。
- 验证边界：按任务要求未运行 Cargo；本文对本文件的结构和静态调用关系负责，不宣称现役 typed point-get 已由这里的 Rust 单测覆盖，也不宣称 Go 端到端测试在本次任务中执行过。
