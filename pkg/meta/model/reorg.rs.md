# `pkg/meta/model/reorg.rs`

## 文件定位

`reorg.rs` 定义 DDL 重组（reorganization）和 backfill 持久化元数据，是 Go `pkg/meta/model/reorg.go` 的 Rust 对照实现。它不是独立 crate 入口：`pkg/meta/model/internal/group3/lib.rs` 在私有 `reorg` 模块中用 `include!("../../reorg.rs")` 编入源码，再以 `pub use reorg::*` 导出；`pkg/meta/model/lib.rs` 又通过 `group_3` 暴露该模型。顶层包还主要再导出 group1，因此使用方应留意实际导入路径，例如 DDL 代码通常使用 `astersql_meta_model::group_3::{DDLReorgMeta, ReorgType}`。

该文件位于在线 DDL 的“描述与持久化”边界，而不是执行引擎本身：它保存任务采用的回填方式、阶段、SQL 环境、范围进度和可动态调整的资源参数；真正扫描、写入、合并和调度发生在 `pkg/ddl/**`。源码没有条件编译项，也不创建线程、事务或存储连接。

## 核心职责

1. `BackfillState` 描述需要增量合并的索引回填状态，判别值 `0..=3` 是持久化协议的一部分。
2. `ReorgStage` 保存改列重组阶段，避免恢复或重试时重复执行已完成的更新列、重建索引步骤。
3. `DDLReorgMeta` 汇总一次 DDL 重组的 SQL 环境、告警、执行策略、版本和资源参数，并提供与 Go JSON 兼容的手写序列化。
4. `ReorgType` 区分事务回填、Lightning/SST ingest 和“事务回填 + 增量合并”，并集中判断是否需要 merge 阶段。
5. `BackfillMeta` 保存单个 backfill job 的键范围、当前位置、行数、错误和父 `JobMeta`，提供原子式的 JSON 编解码接口。
6. 私有 `go_bytes` 模块把 Rust `Vec<u8>` 编解码为 Go `encoding/json` 对 `[]byte` 使用的标准 Base64 字符串；`deserialize_null_default` 把 Go 的 `null` 容器兼容为 Rust 默认空值。

## 主要符号

- `deserialize_null_default<'de, D, T>`：私有 Serde 适配器。先解成 `Option<T>`，JSON `null` 返回 `T::default()`，非空值正常解码；用于两个告警 map。
- `go_bytes::{serialize, deserialize}`：私有字节 JSON 适配器，使用标准 Base64 字母表和 `=` 填充。反序列化拒绝非四字节倍数长度和非法字符；`null` 返回空向量。
- `BackfillState`：`#[repr(u8)]` 的公开枚举，包含 `Inapplicable`、`Running`、`ReadyToMerge`、`Merging`。`String` 给日志返回固定文本。
- `ReorgStage`：`#[repr(u8)]` 的公开枚举，依次表示未开始、更新列、重建索引、完成。
- `DDLReorgMeta`：公开运行/持久化模型。普通字段保存 SQL mode、告警、时区、策略、资源组、版本和阶段；`Concurrency`、`BatchSize`、`MaxWriteSpeed` 使用 `AtomicI64`。
- `DDLReorgMetaWire`：私有 Serde 中间结构，把三个原子量投影为 `i64`，并固定 Go 风格 JSON 字段名。`#[serde(default)]` 让缺失字段采用零值，`UseNewCollate` 为 `None` 时不输出。
- `DDLReorgMeta::{ShallowCopy, Get/SetConcurrency, Get/SetBatchSize, Get/SetMaxWriteSpeed, GetUseNewCollateOrDefault, setUseNewCollate}`：复制、兼容旧元数据及在线调整的 API。`setUseNewCollate` 只在同 crate 内可见。
- `ReorgMetaVersion0` 与 `CurrentReorgMetaVersion`：当前版本为 `1`，对应表范围终止 key 是否包含的语义修正。
- `AnalyzeStateNone` 至 `AnalyzeStateFailed`：以 `i8` 常量表达 analyze 子流程六种状态。
- `ReorgType`：`#[repr(i8)]` 的公开枚举；`NeedMergeProcess` 仅对 `Ingest`、`TxnMerge` 返回 `true`，`String` 返回稳定短名。
- `BackfillMeta::{Encode, Decode}`：`Encode` 返回 JSON 字节并经 `errors::Trace` 转换错误；`Decode` 先构造完整临时值，成功后才替换 `self`。

## 执行流程

`DDLReorgMeta` 的持久化流程是：调用者持有模型并可能通过 setter 更新原子字段；Serde 调用手写 `Serialize`；实现以 `Ordering::SeqCst` 读取三个原子值、克隆 map/字符串/时区并构造 `DDLReorgMetaWire`；wire 结构输出固定 snake_case JSON。反向读取时，`DDLReorgMetaWire::deserialize` 先应用缺失字段默认值和 `null` map 兼容，再把三个整数重新包装为新的 `AtomicI64`，最后一次性返回 `DDLReorgMeta`。

读取资源参数时，`GetConcurrency` 和 `GetBatchSize` 先顺序一致地读取原子值。若值为零，分别回退到 `vardef::GetDDLReorgWorkerCounter()` 和 `vardef::GetDDLReorgBatchSize()`，用于兼容未保存这些字段的旧任务；`GetMaxWriteSpeed` 不回退，因为零本身定义为不限速。`Set*` 方法直接以 `SeqCst` 写入，允许 `admin alter ddl jobs` 一类控制路径动态调整。

DDL 选择 `ReorgType` 后，`NeedMergeProcess` 决定是否启用临时索引和增量合并。直接调用证据包括 `pkg/ddl/index.rs` 设置 `BackfillStateRunning`，以及 `pkg/ddl/persistent_modify_column.rs` 决定临时 GC ID、重建索引和 merging 分支。改列流程读取、推进 `DDLReorgMeta.Stage`，完成后写入 `ReorgStageModifyColumnCompleted`。

`BackfillMeta` 的进度持久化走 `Encode`/`Decode`：三个 key 字段经 `go_bytes` 变为 Base64；解码先完成整个新值，只有成功才覆盖接收者，因此格式错误不会留下“部分字段已更新”的半成品。`pkg/meta/model/job_test.rs::test_backfill_meta_codec` 验证了错误和父 `JobMeta` 的往返，`reorg_test.rs` 验证字段名、Base64 和 Go 零值兼容。

## 数据与状态

`BackfillState`、`ReorgStage`、`ReorgType` 的整数判别值和所有 `#[serde(rename = ...)]` 名称都是跨版本、跨 Go/Rust 的持久化契约，不应重排或改名。`BackfillState` 的状态转换由 DDL 层执行：`Running` 表示回填进行中，`ReadyToMerge` 表示等待所有实例感知复制状态，`Merging` 表示把临时增量合回原对象；本文件只提供状态值，不强制转换合法性。

`DDLReorgMeta` 同时含两类状态：普通字段是某一时刻的任务快照，三个原子字段可在共享引用下独立变化。`ShallowCopy` 在 Rust 中会克隆拥有所有权的 map、字符串、时区，并以三次 `SeqCst` load 创建互不共享的新原子量；因此得到的是逐字段观测的副本，并非把原子量或后续在线更新绑定到原对象。`pkg/meta/model/job.rs::JobWire::from_job` 使用该副本注入最新 warnings，`SubJob::to_proxy_job` 则复制父任务配置后覆盖子任务的 `ReorgTp`、`Stage`、`AnalyzeState`。

`UseNewCollate: Option<bool>` 明确区分“旧元数据未携带字段”和显式 `false`。`GetUseNewCollateOrDefault` 只在 `None` 时采用调用者默认值，防止在另一个 keyspace 执行时误用当前进程设置。

`BackfillMeta::{StartKey, EndKey, CurrKey}` 是原始字节键；`EndInclude` 决定结束键是否包含，`RowCount` 保存进度，`Error`、`Warnings`、`WarningsCount` 保存诊断信息，`JobMeta` 关联父任务。空和 JSON `null` 的 map/key 都被归一为空集合或空向量。

## 依赖与调用关系

crate 边界由 `pkg/meta/model/Cargo.toml` 和 `internal/group3/Cargo.toml` 决定：顶层 `astersql-meta-model` 依赖四个内部 group；实际编译本文件的 `astersql-meta-model-group3` 直接依赖 group1、`serde`、`serde_repr`、`serde_json`。`TimeZoneLocation` 来自 group1；`JobMeta` 与本文件同处 group3；`mysql`、`errors`、`terror`、`vardef` 由 group3 边界提供。当前这些边界中的 `errors::Error`/`terror::Error`、默认变量函数属于 Rust group3 的适配实现，不能假定具备 Go 包全部行为。

RustCodeGraph 的文件节点显示 `reorg.rs` 被 11 个文件使用，列出的上游包括 `pkg/ddl/backfilling_txn_executor.rs`、`pkg/ddl/index.rs`、`pkg/ddl/job_worker.rs`、`pkg/ddl/persistent_actions.rs` 及测试。精确源码搜索进一步确认：

- `pkg/meta/model/job.rs` 调用 `ShallowCopy` 并持有 `DDLReorgMeta`/`ReorgStage`/`ReorgType`。
- `pkg/ddl/backfilling_txn_executor.rs::new_reorg_dist_sql_context_with_reorg_meta` 把 `ResourceGroupName` 送入 DistSQL 请求上下文。
- `pkg/ddl/job_worker.rs` 用 `GetBatchSize` 构造批处理请求，并显式拒绝不可转换或为零的批大小。
- `pkg/ddl/index.rs` 用 `NeedMergeProcess` 选择临时索引状态和 telemetry 路径。
- `pkg/ddl/persistent_modify_column.rs` 使用 `NeedMergeProcess`、`ReorgStage`、`GetBatchSize` 和 `BackfillState` 驱动持久化改列流程。

下游方面，本文件只调用 Serde/JSON、标准集合与原子 API、group3 的默认配置函数以及错误转换函数，没有 I/O、事务或网络调用。

## 错误处理与边界

`BackfillMeta::Encode` 和 `Decode` 把 `serde_json` 错误映射为 `errors::Error`。`Decode` 的“先解码、后替换”保证失败时原对象不变；`DDLReorgMeta` 反序列化也只有 wire 全部成功后才构造公开对象。

Base64 解码明确检查长度和字符，但实现没有完整验证填充位置/尾部规范位：它允许第三、第四位置的 `=` 映射为零，并根据原始填充决定输出长度。因此扩展时不应宣称它是严格 Base64 验证器；兼容目标是 Go JSON `[]byte` 的正常输出。非法枚举判别值由 `serde_repr` 拒绝，未知 `BackfillState` 无法像 Go 的任意 `byte` 值那样进入 `String` 的 `unknown` 分支，这是 Rust 封闭枚举与 Go 命名整数的差异。

`GetConcurrency`/`GetBatchSize` 把 `i64` 直接转换为 `i32`，setter 也没有在模型层校验正数或上限；实际使用方必须验证业务范围。`job_worker.rs` 至少会拒绝转换失败和零批大小，但不能据此推断所有调用方都已验证负值或过大值。`MaxWriteSpeed == 0` 是有效的“不限速”，不能沿用另外两个 getter 的旧版本回退逻辑。

## 并发与资源生命周期

文件中的并发机制仅限 `DDLReorgMeta` 的三个 `AtomicI64`，所有 load/store 均使用 `Ordering::SeqCst`。setter 接受 `&self`，因此共享引用持有者可以在线更新参数；getter 每次重新读取，不缓存值。三个参数彼此不是一个原子事务：序列化或 `ShallowCopy` 依次读取它们，可能得到来自不同更新时刻的组合，但每个字段自身不存在撕裂读取。

`Warnings` 等普通字段不受锁或原子保护，不能仅凭 `DDLReorgMeta` 本身跨线程并发修改。调用方负责外层所有权或同步；例如测试/执行框架可把元数据包在 `Arc<Mutex<_>>` 中。本文件不启动后台任务、不持有 channel、不打开文件，也没有显式清理阶段。`BackfillMeta` 的 `Vec`、map、错误和 `JobMeta` 都随结构体按 Rust 所有权自动释放。

## 与 Go 版本的对应关系

Rust 的字段、JSON 名、状态值和核心方法逐项对应 `pkg/meta/model/reorg.go`：两个版本都以零值兼容旧任务，`UseNewCollate` 都保留三态语义，`ReorgTypeIngest`/`TxnMerge` 都要求 merge，`BackfillMeta` 都使用 JSON。`pkg/meta/model/Cargo.toml` 的 `package.metadata.porting.go-package = "pkg/meta/model"` 也明确记录此移植来源。

已核对的差异包括：

- Go `atomic.Int64` 在结构体浅拷贝中按值复制；Rust 因 `AtomicI64` 不实现 `Clone`，`ShallowCopy` 显式 load 后创建新原子量，并同时深克隆拥有所有权的容器。
- Go `int` 的宽度依平台，Rust 将 concurrency/batch 固定为 API `i32`、wire `i64`；Rust `MaxWriteSpeed` API 保留 `i64`，`reorg_test.rs::reorg_meta_max_write_speed_preserves_go_int64_range` 覆盖大于 32 位的数值。
- Go `[]byte` 由标准库 JSON 自动编码为 Base64；Rust 用私有 `go_bytes` 手写同格式。Go nil slice/map 与 Rust 空集合的内存语义不同，但当前 Rust 解码将 `null` 归一为空值。
- Go `BackfillState.String` 对未知整数返回 `backfill state unknown`；Rust `serde_repr` 的封闭枚举不能构造未知合法实例，因此只有四个已知分支。
- Go `errors.Trace` 可保留 PingCAP 错误语义；当前 group3 的 Rust `errors::Trace` 把可显示错误转换成字符串，文档不能把它描述成等价的 Go 堆栈实现。

Go 侧相关测试不在 `reorg_test.go`（该文件不存在），而在 `pkg/meta/model/job_test.go`：`TestDDLReorgMetaUseNewCollate` 和 `TestBackfillMetaCodec`。Rust 对照覆盖位于独立的 `reorg_test.rs`、`job_test.rs` 和 `job_3_aster_unit_test.rs`，符合测试不内嵌生产文件的仓库规则。

## 扩展指南

新增持久化字段时，应同时修改 `DDLReorgMeta` 与 `DDLReorgMetaWire` 的字段、`Serialize`、`Deserialize`、`ShallowCopy`，选择与 Go 完全一致的 JSON 名和缺失值语义，并同步 `pkg/meta/model/reorg.go` 对照事实。若字段需要区分“缺失”和显式零值/false，应使用 `Option<T>`，不要仅依赖 `#[serde(default)]`。涉及协议版本语义时还需评估 `CurrentReorgMetaVersion` 及旧任务恢复路径。

新增或改变回填类型/阶段/状态时，不得重排已有判别值；应检查 `pkg/ddl/index.rs`、`pkg/ddl/persistent_modify_column.rs`、`pkg/meta/model/job.rs` 的所有分支和持久化恢复逻辑。改变 `NeedMergeProcess` 会直接影响临时索引、GC ID、增量合并及 telemetry，兼容性和数据正确性风险高。

改变在线参数时，应继续通过 getter/setter 访问原子量，明确零值含义、数值范围和多字段一致性需求。若要求多个参数形成一致快照，当前三个独立 `AtomicI64` 不足，需要在调用层或模型层设计共同同步机制；不能只调整内存顺序就获得多字段事务性。

测试应放在独立文件，优先扩展 `pkg/meta/model/reorg_test.rs`；涉及 Job JSON 和父子任务复制时同步 `job_test.rs`，涉及 DDL 状态机时扩展对应 `pkg/ddl/*_test.rs`。至少覆盖旧 JSON 缺字段/`null`、Go/Rust 往返、非法枚举和 Base64、零值回退、`i64` 边界、失败解码不修改原对象。性能上需留意序列化和 `ShallowCopy` 会克隆告警 map、字符串和时区，超大告警集合可能增加内存与延迟。

## 验证依据

- RustCodeGraph：`status` 确认索引含 11,467 个文件、`reorg.rs` 含 52 个符号；`files --filter pkg/meta/model` 确认源、Go 对照和独立测试均已索引；`node --file pkg/meta/model/reorg.rs --offset 1 --limit 500` 读取完整 499 行并报告 11 个使用文件。
- RustCodeGraph 精确查询：`query ReorgType`、`query DDLReorgMeta`、`query BackfillMeta` 确认 Rust/Go 对照符号及 `pkg/ddl/index.rs::pick_backfill_type` 等相关入口。方法级 `callers` 对同名符号优先解析到 Go 实现，故 Rust 方法调用边由精确源码搜索补足，没有把 Go 调用边误记成 Rust 调用边。
- 源与装配：`pkg/meta/model/reorg.rs`、`pkg/meta/model/internal/group3/lib.rs`、`pkg/meta/model/lib.rs`、`pkg/meta/model/Cargo.toml`、`pkg/meta/model/internal/group3/Cargo.toml`。
- Go 对照：`pkg/meta/model/reorg.go`；Go 测试：`pkg/meta/model/job_test.go`。
- Rust 测试：`pkg/meta/model/reorg_test.rs`、`pkg/meta/model/job_test.rs`、`pkg/meta/model/job_3_aster_unit_test.rs`。
- Rust 调用证据：`pkg/meta/model/job.rs`、`pkg/ddl/backfilling_txn_executor.rs`、`pkg/ddl/job_worker.rs`、`pkg/ddl/index.rs`、`pkg/ddl/persistent_modify_column.rs`。
- 人工复核结论：该文件存在是为了给 DDL 重组与单个 backfill job 提供可恢复、可跨 Go/Rust JSON 交换且部分参数可在线调整的模型；运行路径由 Serde 编解码、原子 getter/setter 和 DDL 层状态消费组成；安全扩展必须同时维护 wire、复制、版本、Go 协议与独立测试。
