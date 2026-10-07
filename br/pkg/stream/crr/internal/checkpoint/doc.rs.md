# `br/pkg/stream/crr/internal/checkpoint/doc.rs`

## 文件定位

`doc.rs` 是 `astersql-br-pkg-stream-crr-internal-checkpoint` crate 的包级算法说明模块，源码由 [`lib.rs`](lib.rs) 通过 `#[path = "doc.rs"] pub mod doc;` 显式挂载。它只包含 `//!` 文档属性，没有常量、类型、trait、函数、`impl`、条件编译项或可执行初始化逻辑；因此它是“算法契约说明”，不是计算器实现或兼容门面。

crate 边界由 [`Cargo.toml`](Cargo.toml) 确定：库入口是 `lib.rs`，Go 包映射是 `br/pkg/stream/crr/internal/checkpoint`，直接依赖 `backupmetas`、`streamhelper`、`serde` 与 `serde_json`。这些依赖均由相邻实现模块使用，`doc.rs` 自身没有 `use` 或依赖调用。实际计算入口在 [`calculator.rs`](calculator.rs) 的 `Calculator::ComputeNextCheckpoint`，扫描与解析在 [`storage.rs`](storage.rs)，轮次规划、等待和水位推进在 [`progress.rs`](progress.rs)。

## 核心职责

该文件为 CRR（持续复制恢复）安全检查点定义三组必须共同理解的语义：

1. 每轮先读取上游全局检查点 `c`，再扫描尚未纳入同步水位的 backup meta，等待其中引用的所有对象满足 `ObjectSyncChecker`，最后才把 `c` 作为下游安全检查点返回。对应实现主链是 `ComputeNextCheckpoint -> poll_upstream_checkpoint -> load_alive_stores -> plan_round -> wait_object_sync -> advance_synced_state`。
2. `flushTS <= c` 不能单独证明安全。一个以较大 `flushTS` 命名的批次可能包含小于该 `flushTS` 的 region checkpoint，因而仍是恢复到 `c` 所必需的对象。
3. `syncedTS` 是“复制已完成”的扫描水位，必须按 store 分别记录并取仍约束当前轮次的最小值；`lastCheckpoint` 则是最近一次成功提交给调用方的上游全局检查点。二者用途不同，不能互换。

因此，本文件存在的价值是固定跨模块、跨语言的安全不变量，防止维护者只看全局 checkpoint 或文件名字典序便错误跳过尚未复制的对象。

## 主要符号

`doc.rs` 没有 Rust item，RustCodeGraph 对该文件只索引到文件节点。文档中提到的关键符号均定义在相邻实现中：

- `Calculator::ComputeNextCheckpoint(&mut self, &Context) -> Result<u64, Error>`：一轮计算的公开入口；只在轮次成功后写入 `last_checkpoint`。
- `Calculator::SyncedTS`、`Calculator::LastCheckpoint`：分别暴露复制完成水位和最近安全 checkpoint。
- `calculatorState::{synced_ts, last_checkpoint, synced_by_store}`：跨轮状态；`synced_by_store` 是计算全局水位和识别缺失 alive store 的基础。
- `ObjectSyncChecker::FileSynced`：下游对象是否可安全消费的抽象。`ExistenceSyncChecker` 仅是“存在即同步”的适配实现，并非唯一允许的证明方式。
- `Calculator::plan_round`：过滤已由 store 水位覆盖的 meta，并受 `MetaReadConcurrency` 限制地加载新 meta。
- `Calculator::wait_object_sync`：轮询本轮 `pending_paths`，直到全部确认或上下文/检查器返回错误。
- `Calculator::advance_synced_state`：先合并本轮每个 store 的最大 `flush_ts`，以剪枝前的进度集合求最小候选，再剪除离线 store；任何 alive store 缺少进度时阻止全局 `synced_ts` 提升。
- `meta_scan_start_after` 与 `ParseName`：把 `synced_ts` 转成大写十六进制扫描边界，并从按 `(flushTS, storeID, extraTags)` 排序的 meta 名解析水位和 store。

这些符号不是由 `doc.rs` 导出；它们经 `lib.rs` 的 `pub use calculator::*` 和公开模块组成 crate API。

## 执行流程

运行时上游是 [`service.rs`](../../service/service.rs) 的 `Service::run_once`：服务恢复持久状态后锁住 `Calculator`，读取旧 `LastCheckpoint`，调用 `ComputeNextCheckpoint`，随后把新状态排队并持久化；若 checkpoint 未前进，服务转而等待 PD watcher，避免空转。

一次已推进的计算轮次按以下顺序执行：

1. `poll_upstream_checkpoint` 用任务名读取 PD 全局 checkpoint。只有严格大于 `last_checkpoint` 才继续；相等或回退时返回旧值。
2. `load_alive_stores` 读取当前存活 store，并过滤 ID 为 0 的占位项。
3. `storage.rs::collect_meta_files` 从 `v1/backupmeta` 开始增量扫描；`synced_ts == 0` 时全量扫描，否则使用 `meta_scan_start_after(synced_ts)`。文件名必须能解析，且 `flush_ts > synced_ts` 才进入候选。
4. `plan_round` 再按 `synced_by_store[store_id]` 跳过该 store 已覆盖的旧 meta，并行加载 meta 内容，提取需要确认的 data file 路径和每个 store 的最大 `flush_ts`。
5. `wait_object_sync` 反复调用 `ObjectSyncChecker::FileSynced`。只有 `pending_paths` 清空才允许轮次提交；未清空时按 `PollInterval` 等待，并响应取消或 deadline。
6. `advance_synced_state` 合并本轮 store 水位。额外的未观察 alive store 只会阻塞提升，不能抬高最小值；已观察但离线的 store 在本轮对象验证完成后被剪枝，但剪枝前进度仍参与本轮最小值。
7. 最后更新 `last_checkpoint`，发送成功观察事件并返回上游 checkpoint。任一步失败都不执行成功提交，并发送失败事件。

## 数据与状态

- `last_checkpoint`：最近成功完成一轮并返回的上游全局 checkpoint。它判断 PD 是否出现新进展，也由服务层决定是否进入 watcher 等待。
- `synced_ts`：全局复制完成水位，用于构造下一轮 meta 列举的 `StartAfter`；只允许单调增加。
- `synced_by_store: HashMap<u64, u64>`：每个 store 已确认同步的最大 `flush_ts`。这是避免把“全局文件名字典序”误当成“跨 store 时间单调”的关键状态。
- meta 文件名：前 16 个大写十六进制字符表示 `flushTS`，随后 16 个字符表示 `storeID`，之后可有额外标签。全局排序键是 `(flushTS, storeID, extraTags)`，但 `flushTS` 的单调保证只成立于单个 store 的序列。
- `PersistentState`：把上述三个状态复制为可持久化快照。`RestorePersistentState` 只能在计算开始前调用，避免运行中覆盖进度。
- `pending_paths`：只属于当前轮次；它清空之前不得提交该轮 checkpoint。计算器检查同步状态，不读取下游对象内容。

必须保持的安全关系是：checkpoint 返回值可以前进而 `synced_ts` 因缺失 alive store 暂停，但 `synced_ts` 绝不能因为 alive store 集合变化而越过本轮已同步 store 进度的最小值。

## 依赖与调用关系

上游调用链为 `crr/service::Service::run_once -> Calculator::ComputeNextCheckpoint`。`service` crate 通过路径依赖 `../internal/checkpoint` 引用本 crate，并通过 `Observer` 把轮次状态提供给 status/metrics。

计算器的直接下游依赖通过 `CalculatorDeps` 注入：

- `PDMetaReader` 提供任务全局 checkpoint 和 alive store 集合；
- `UpstreamStorageReader` 列举并读取源端 backup meta；
- `ObjectSyncChecker` 判定被 meta 引用的对象是否已在恢复侧安全可用；
- `backupmetas::ParseName` 解析 meta 文件名；
- `serde`/`serde_json` 解析 meta 内容，`streamhelper::Store` 表示 store 信息。

RustCodeGraph 的文件查询确认目录内 `calculator.rs`、`progress.rs`、`storage.rs` 及独立测试均被索引；对 `ComputeNextCheckpoint` 的精确调用图查询因 Go/Rust 同名符号存在歧义未返回有效边，因此生产调用者又由 `rg` 核实为 Rust `service/service.rs`，测试调用者分布在 `checkpoint_calculator_test.rs`、`integration_test.rs`、`parity_test.rs` 和 `randomized_integration_test.rs`。这项限制不影响源码引用结论，但不把无效图边当作证据。

## 错误处理与边界

`doc.rs` 不产生错误；它描述的实现采用 `Result<_, Error>` 逐层传播，并为关键边界增加上下文：PD 读取、alive store 加载、meta 遍历/解析/读取、store ID 一致性、对象同步检查和上下文取消均可终止本轮。

关键安全边界包括：

- 非法或不支持增量 `StartAfter` 的上游存储在构造期拒绝；`TaskName` 为空也拒绝构造。
- 坏 meta 文件名不能静默跳过，否则扫描水位可能越过未知对象；存储遍历错误同样向上返回。
- 任一 meta 加载任务失败会取消同轮兄弟任务，并保留首个错误。
- `ObjectSyncChecker` 返回 `false` 表示继续等待，返回错误表示整轮失败；两者不能混为“已同步”。
- 上下文取消和 deadline 会打断等待；失败轮次不会更新 `last_checkpoint` 或提交新的安全水位。
- alive store 尚无 `synced_by_store` 项时，checkpoint 仍可能依据已验证对象返回，但全局 `synced_ts` 必须停留在旧值。
- 已移除 store 的未同步文件仍必须在当前轮次等待，不能因 store 已离线而直接忽略。

## 并发与资源生命周期

`doc.rs` 无资源和线程生命周期。实际并发位于 `plan_round`：它先物化待加载 meta，再使用 `thread::scope` 启动受 `ConcurrencyLimiter` 控制的任务；许可由 `ConcurrencyGuard::drop` 自动归还，scope 保证返回前所有借用上游存储的线程已经结束。共享轮次计划和首错槽由 `Mutex` 保护，首错触发子上下文取消。

下游同步等待是当前线程中的轮询循环，不为每个对象创建常驻任务；每轮检查后通过可取消的睡眠遵守 `PollInterval`。`Context` 用原子取消标记和祖先链传播父级取消，子级取消不反向影响父级。服务层以 `Mutex<Calculator>` 串行化状态修改，并在每轮后安排持久状态保存，因此 `synced_ts`、`last_checkpoint` 与 `synced_by_store` 的生命周期跨越多个计算轮次和进程恢复。

扩展并发代码时必须保持三点：并发上限约束的是正在执行的读取；首错会停止同轮后续工作；所有读取线程退出且全部必要对象确认后才可提交状态。

## 与 Go 版本的对应关系

Rust 文件逐段对齐同目录 [`doc.go`](doc.go)。两者都声明相同的轮次模型、`flushTS`/region checkpoint 反例、每 store 最小水位、alive store 的“只额外阻塞”规则、离线 store 的当轮约束，以及 `ObjectSyncChecker` 不读取下游内容的边界。Rust 版本额外提供中文说明和 Rust 标识符拼写，但没有改变算法契约。

实现语义对应关系为：Go `Calculator.ComputeNextCheckpoint` 对应 Rust `Calculator::ComputeNextCheckpoint`；Go `lastCheckpoint`、`syncedTS`、`syncedByStore` 对应 Rust `calculatorState` 的蛇形字段；Go `ObjectSyncChecker.FileSynced` 对应 Rust trait 方法。Rust 用 `Box<dyn ... + Send + Sync>` 表达非空依赖，部分 Go 的 `nil` 校验因此变成编译期约束；Rust 独立测试保留 Go 的错误文本与正向构造契约用于对照。

同目录 Go 测试提供原始行为依据：`checkpoint_calculator_test.go` 覆盖新 alive store 阻塞、同步检查错误、离线 store 等待/剪枝和 stale meta 跳过；`integration_test.go` 覆盖持久状态恢复、并发读取、上游无变化和未来 flush meta；`randomized_integration_test.go` 覆盖随机交错。Rust 对应测试分别位于独立的 `*_test.rs` 文件，没有把测试嵌入生产源文件。

## 扩展指南

若只修改算法说明，应同时核对 `doc.rs` 与 `doc.go`，并确认描述能在 `calculator.rs`、`progress.rs`、`storage.rs` 中找到实现证据。不要在 `doc.rs` 增加业务状态或执行逻辑；真正的 API 放在 `calculator.rs`，扫描格式放在 `storage.rs`，轮次推进和等待策略放在 `progress.rs`。

常见扩展接入点及验证要求：

- 新的同步证明后端：实现 `ObjectSyncChecker`，在服务装配处注入；不得让核心读取下游对象内容。同步补充 `checkpoint_calculator_test.rs` 的自定义 checker、错误传播和等待场景，以及 service 层接线测试。
- 改变 meta 命名或排序：同步修改 `ParseName` 上游契约、`meta_scan_start_after` 和 storage 测试；重点验证字典序边界、大小写、相同 `flushTS` 多 store、额外标签和坏名称。错误会直接影响漏扫风险。
- 改变水位算法：修改 `advance_synced_state`/`check_missing_store`，同步补充 alive store 缺失、离线剪枝、跨 store 不同水位和重启恢复测试。必须证明 `synced_ts` 单调且不超过安全最小值。
- 改变并发或轮询：修改 `plan_round`/`wait_object_sync`，验证最大并发、首错取消、deadline、部分同步和 observer 统计；关注对象存储读放大及锁持有时间。
- 改变持久状态：同时更新 `PersistentState`、快照/恢复和 service 保存流程，提供向后兼容策略，避免旧状态被误解释为更高安全水位。

任何行为修复都应在同目录独立 `*_test.rs` 中增加回归测试，并与对应 Go 测试意图保持一致；本文件是纯说明，不应承载测试代码。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter br/pkg/stream/crr/internal/checkpoint` 列出目标实现、Go 对照和独立测试。
- RustCodeGraph `node --file .../doc.rs`：确认目标共 106 行且仅有 crate 文档属性；`node` 读取 `doc.go`、`lib.rs`、`calculator.rs` 和 `progress.rs`，核实挂载方式、主流程、状态推进与并发等待。
- RustCodeGraph `query CheckpointCalculator`、`query ObjectSyncChecker`、`query ComputeNextCheckpoint`：确认 Go/Rust 对应定义及测试引用；精确 `callers/callees` 因同名歧义未产出有效边，未据此推断调用关系。
- 源码与配置：`Cargo.toml`、`service/Cargo.toml`、`service/service.rs`、`storage.rs`，核实 crate 依赖、生产调用入口、增量扫描和 meta 解析边界。
- 测试：`checkpoint_calculator_test.rs`、`integration_test.rs`、`parity_test.rs`、`progress_test.rs`、`storage_internal_test.rs`、`randomized_integration_test.rs`，以及对应 Go `checkpoint_calculator_test.go`、`integration_test.go`、`randomized_integration_test.go`、`storage_internal_test.go`。
- 人工复核结论：该文件为何存在——固定 CRR 安全 checkpoint 的跨语言契约；如何运行——自身不运行，由 service 驱动 calculator 主链；如何安全扩展——在职责所属实现模块接线并保持每 store 最小水位、全对象确认、失败不提交三项不变量。

本任务为纯文档分析，按计划不运行 Cargo。交付验证以固定十一章节结构、真实路径/符号引用及上述源码证据为准。
