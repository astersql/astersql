# `br/pkg/utiltest/crr/lib.rs`

源文件：[`lib.rs`](lib.rs)

## 文件定位

`lib.rs` 是 Cargo 包 `astersql-br-pkg-utiltest-crr` 的 crate 根；`br/pkg/utiltest/crr/Cargo.toml` 以 `[lib] path = "lib.rs"` 明确指定它，并用 `package.metadata.porting.go-package = "br/pkg/utiltest/crr"` 标记 Go 对照包。该 crate 是 BR 跨区域复制（CRR）测试夹具，不是服务器生产请求链上的 CRR 服务实现：它用假 PD、对象存储替身、flush/复制模拟器和本地 harness，为 stream/checkpoint 逻辑提供可重复驱动的环境。

本文件自身没有业务函数、类型或运行时初始化；它负责建立编译期模块图、公开 API 和独立测试入口。Cargo 搜索未发现其他 Rust manifest 对该 crate 的直接依赖；Go/Bazel 侧的 `br/pkg/stream/crr/service/BUILD.bazel` 与 `br/pkg/stream/crr/internal/checkpoint/BUILD.bazel` 依赖同路径 Go target，不能据此推断 Rust crate 已接入这些包。

## 核心职责

- 以 `#[path = "..."] pub mod ...` 挂载 `stubs`、`types`、`builder`、`crr_sim`、`flush_sim`、`pd_sim`、`pd_sim_service`、`harness` 八个生产模块。
- 通过 `pub use builder::*`、`crr_sim::*`、`flush_sim::*`、`harness::*`、`pd_sim::*`、`types::*` 模拟 Go 同包符号的扁平可见性，使测试可从 crate 根取得布局、刷盘、复制、PD 和 harness API。
- 只从 `stubs` 根级再导出 `ArcMemStorage`、`CancelHandle`、`Context`、`Error`、`LocalStorage`、`MemStorage`、`Result`、`Storage`，避免把全部替身类型污染根命名空间。`pd_sim_service` 模块公开，但没有 glob 再导出；其作用主要是为 `PDSim` 实现 streamhelper trait。
- 在 `cfg(test)` 下挂载 `builder_test.rs`、`parity_test.rs`、`harness_test.rs`、`pd_sim_test.rs`、`pd_sim_service_test.rs`，保持生产源与 Rust 测试分文件。
- 用 crate 级 `#![allow(...)]` 接受 Go 风格的大写字段/函数名、迁移期死代码和未使用项；这只是编译期 lint 策略，不改变运行时行为。

## 主要符号

`lib.rs` 不定义常量、结构体、trait、函数或 `impl`。其公开面来自下列子模块：

- `types`：`DEFAULT_TASK_NAME`、`DEFAULT_TASK_START_PHYSICAL`、`REGION_ID_TAG`，以及 `TestContext`、`DeterministicRNG`、`RegionBoundary`、`RegionState`、`FlushRecord`。`deriveDeterministicSeed` 用 FNV-1a 从根 seed 和组件名派生可复现子流。
- `builder`：`RegionLayoutOption`、`BuildRegionLayout`、`StoreIDRange`、`AddRegion`、`AddRegionsBySplitKeys`、`AddRoundRobinRegions`，用于构造覆盖完整 key space 的连续 region 布局。
- `pd_sim` 与 `pd_sim_service`：`PDSim`、`NewPDSimWithTestContext` 维护假 region/store、TSO 和任务 checkpoint；`pd_sim_service.rs` 将其适配为 `ClusterMeta`、`LogBackupService`、`LogBackupFlushIntervalGetter`、`StreamMeta` 等 streamhelper 边界。
- `flush_sim`：`FlushSim`、`NewFlushSimWithTestContext` 将某个 store 的 region 状态写成 backup metadata/log 对象并记录 `FlushRecord`。
- `crr_sim`：`CRRUpstreamStorage`、`CRRWorker`、`NewVersionCreatedEvent`、`new_event_channel`。上游装饰器在写对象后发事件，worker 拉取并把对象复制到下游。
- `harness`：`TestHarness` 与 `NewLocalTestHarnessWithTestContext` 组合上述组件，提供 `Tick`、`PullMessages`、`Replicate`、`UploadGlobalCheckpoint`、`AssertDownstreamCanRestoreTo`、`Close`。
- `stubs`：`Storage` trait 及内存/本地文件实现、可取消 `Context` 和统一 `Error`/`Result`，是为了避免完整 kv/objstore/gRPC 依赖而建立的测试替身，不是真实 TiKV 或生产对象存储实现。

## 执行流程

crate 根不主动执行代码。典型的本地 CRR 测试链由公开 API 组装：

1. 调用 `BuildRegionLayout` 顺序应用布局选项，验证首段从空 key 开始、相邻段连续、store ID 非零、末段以空 end key 闭合；`NewTestContextWithSeed` 固定随机种子。
2. `NewLocalTestHarnessWithTestContext` 在系统临时目录下创建独立 upstream/downstream 子目录和 `LocalStorage`，再用布局创建 `PDSim`。
3. harness 建立事件通道，用同一个上游底层存储分别构造 `CRRWorker` 和 `CRRUpstreamStorage`；`FlushSim` 向装饰后的 upstream 写入，因此 metadata/log 写入会进入复制事件流。
4. `NewCommandCheckpointAdvancer` 连接实现 `StreamMeta` 的 `PDSim`。`start_task_listener` 同步读取初始任务事件并设置任务/暂停状态，然后启动订阅处理。
5. 测试调用 `FlushSim::FlushStore` 生成记录，调用 `PullMessages` 把版本事件收进 worker 缓冲，再调用 `Replicate` 将对应对象复制到 downstream。
6. `UploadGlobalCheckpoint` 推进任务 checkpoint；`AssertDownstreamCanRestoreTo` 先检查目标 TSO 不超过全局 checkpoint，再逐条解析截至该 TSO 的 backupmeta，确认元数据及其引用的日志对象都可从 downstream 读取。
7. `Close` 或 `Drop` 关闭存储并清理 harness 临时目录，结束本次夹具生命周期。

## 数据与状态

`lib.rs` 自身不保存状态。状态分散在子模块且通过根级再导出暴露：`TestContext` 保存 seed 和日志；`PDSim` 保存 fakecluster、任务名/起始 TS 与 checkpoint；`FlushSim` 保存 RNG、序列和 flush 记录；`CRRWorker` 保存事件接收端及待复制缓冲；`TestHarness` 持有上下游 `Arc<dyn Storage>`、临时根目录和所有组件。

布局和 flush 结果使用拥有所有权的 `Vec<u8>`、`Vec<u64>`、`String`，返回快照时做深拷贝，避免测试间共享可变缓冲。`ArcMemStorage`/`LocalStorage` 通过 `Arc<dyn Storage>` 被 flush、上游装饰器、worker 和断言侧共享；`Storage` 抽象只覆盖当前夹具所需的读写、遍历、流式 I/O、重命名、预签名占位和关闭行为。

可复现性来自 `TestContext::RNG(component)`：不同组件从相同根 seed 派生隔离的随机流。临时目录名还包含 seed、进程 ID 和 `AtomicU64` harness ID，因此同一进程中相同 seed 的多个 harness 仍相互隔离。

## 依赖与调用关系

`Cargo.toml` 将 crate 连接到 `astersql-br-pkg-stream`（metadata 编解码）、`stream/backupmetas`（backupmeta 名称解析）、`streamhelper` 及其 `config`（checkpoint advancer 与服务 trait）、`utiltest/fakecluster`（PD/store 仿真），并依赖 `rand`、`serde`、`serde_json`。manifest 没有 feature 开关；生产与测试模块只由 `cfg(test)` 区分。Cargo 注释明确该依赖集为 arm64 darwin 可用的精简版本，不包含 kv/domain/kvproto/grpcio/objstore。

内部方向是：`builder -> types`；`pd_sim -> builder/types/fakecluster`；`pd_sim_service -> pd_sim + streamhelper`；`flush_sim -> pd_sim + Storage + stream metadata`；`crr_sim -> Storage + event channel`；`harness -> pd_sim + pd_sim_service 提供的 trait 实现 + flush_sim + crr_sim + stubs`。根文件只决定这些模块和符号是否可见，不替代任何子模块调用。

RustCodeGraph 的 `node --file br/pkg/utiltest/crr/lib.rs` 显示该文件被 `tools/tazel/parity_test.rs` 使用；子模块节点则显示 `types.rs` 被本目录 10 个文件使用、`builder.rs` 被 33 个已索引文件使用、`harness.rs` 被 3 个文件使用。精确限定到 Rust 文件的 `callers/callees` 查询在 30 秒内未返回，因此本文不把图查询缺失解释成“没有调用者”，而以 crate 内直接引用、测试调用和 manifest 搜索作为调用关系补证。

## 错误处理与边界

crate 根没有自己的错误分支；根级 `Result<T>` 固定使用 `stubs::Error`，部分 builder/streamhelper 适配 API 使用 `String` 错误。关键边界由子模块和独立测试锁定：空布局、首段非空、边界不连续、末段未闭合、零 store ID、空 store 列表或无效 region 数量都会拒绝；checkpoint 不允许回退，未知任务名也返回错误。

复制侧空路径事件会被跳过；缺少事件通道时 `PullMessages` 返回零，而需要随机选择器却传入 `None` 时返回 `nil intN` 错误。上游存储先完成写入再发送版本事件，所以发送失败可能伴随“对象已写入”的可观察状态，调用方不能假设整步原子回滚。

`AssertDownstreamCanRestoreTo` 会拒绝超前于全局 checkpoint 的目标，并验证 backupmeta 文件名中的 flush TS、元数据可解析性及所有引用日志的可读性。`LocalStorage` 对缺失删除默认返回带 `is_not_exist` 的错误，只有显式开启 `IgnoreEnoentForDelete` 才忽略。上述行为在 `parity_test.rs` 中有直接断言。

## 并发与资源生命周期

`new_event_channel` 连接上游写入方和 `CRRWorker`；worker 先拉取事件到本地缓冲，再按最新优先或注入的随机顺序复制，因而“拉取”和“落下游”是两个可分别驱动的阶段。`pd_sim_service.rs::SubscribeFlushEvents` 为 fakecluster stream 启动线程，将事件解码后转发到标准通道；接收端断开或流结束时线程退出并取消上下文。

`stubs::Context` 使用共享 `AtomicBool`、`Mutex<Option<String>>` 和 `Condvar` 表示取消；首次取消写入错误并以 `SeqCst` 发布。`pd_sim_test.rs::flush_store_observes_cancellation_after_the_call_starts` 验证进行中的 flush 最终观察到取消。

`TestHarness` 用 `Arc` 共享存储和 `PDSim`，用全局 `AtomicU64` 防止临时目录冲突。构造下游或 PD 失败时显式关闭已创建存储；正常路径由 `Close`/`Drop` 收口并删除临时目录。`harness_test.rs` 验证关闭一个同 seed harness 不会删除另一个 harness 的存储。

## 与 Go 版本的对应关系

Rust 子模块与 Go 同目录文件逐一对应：`types.rs` ↔ `types.go`、`builder.rs` ↔ `builder.go`、`crr_sim.rs` ↔ `crr_sim.go`、`flush_sim.rs` ↔ `flush_sim.go`、`pd_sim.rs` ↔ `pd_sim.go`、`pd_sim_service.rs` ↔ `pd_sim_service.go`、`harness.rs` ↔ `harness.go`。Go 没有 `lib.go`；Rust 通过 crate 根模块声明和再导出模拟 Go 包内/包级可见性。

已验证的语义对应包括：确定性 seed 派生、region/store 布局约束、PD checkpoint 单调性、flush 生成 metadata/log、对象写入触发复制事件、最新优先复制、harness 的 flush→复制→checkpoint→可恢复断言链和关闭清理。`builder_test.rs` 还固定了 Go `uint64` 加法溢出与二进制 key 字节保留语义。

两侧依赖边界并不相同：Go `BUILD.bazel` 直接依赖 kv、objstore、kvproto、tikv client 和 failpoint；Rust Cargo 使用本地 `stubs` 与 slim BR crates。因此可声称的是测试公开契约与已覆盖行为对齐，不能把 Rust 替身描述为生产 Go 存储/网络栈的完整实现。

## 扩展指南

- 新增生产子模块时在 `lib.rs` 添加明确的 `#[path] pub mod`；只有要维持 Go 包级调用体验的 API 才根级再导出，并检查多个 glob re-export 的同名冲突。
- 修改布局契约时聚焦 `builder.rs` 和 `types.rs`，同步独立 `builder_test.rs`、`parity_test.rs` 与 Go `builder_test.go`；必须覆盖空/闭合/连续性、store 轮转、二进制 key 和 `uint64` 溢出。
- 修改 PD 或 streamhelper 适配时聚焦 `pd_sim.rs`、`pd_sim_service.rs`，同步 `pd_sim_test.rs`、`pd_sim_service_test.rs` 和 parity 测试，重验 checkpoint 回退、任务名、取消、region 错误类型、flush 订阅和 key 解码。
- 修改 flush/复制顺序或对象格式时聚焦 `flush_sim.rs`、`crr_sim.rs`，同步 `parity_test.rs` 及对应 Go 文件；特别保留 metadata 的 `StoreId`、`FileGroup.Path` wire shape、空路径过滤、nil 边界和“先写后发事件”的非原子语义。
- 修改 harness 生命周期时聚焦 `harness.rs`，同步 `harness_test.rs`、`parity_test.rs` 与 Go `harness.go`，覆盖部分构造失败清理、同 seed 隔离、重复/显式关闭和恢复目标边界。
- 若要让其他 Rust crate 使用本夹具，应在调用方 Cargo manifest 添加真实路径依赖并验证平台依赖；只增加根级再导出不能建立跨 crate 接线。测试逻辑继续放在独立 `*_test.rs`，不要内嵌到生产源文件。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、其中 7,032 个 Rust 文件；`node --file br/pkg/utiltest/crr/lib.rs` 核对了 8 个公开生产模块、根级再导出和 5 个 `cfg(test)` 模块。
- RustCodeGraph 源码节点：读取了 `stubs.rs`、`types.rs`、`builder.rs`、`crr_sim.rs`、`flush_sim.rs`、`pd_sim.rs`、`pd_sim_service.rs`、`harness.rs` 的模块职责和关键符号；对 `NewLocalTestHarnessWithTestContext`、`BuildRegionLayout`、`NewCRRWorker`、`NewPDSimWithTestContext` 发起精确 `query/callers/callees`，其中 query 区分出 Go/Rust 定义，限定文件的调用边查询超时，故未据其作负面结论。
- crate/构建边界：`br/pkg/utiltest/crr/Cargo.toml`、`br/pkg/utiltest/crr/BUILD.bazel`；Rust manifest 全库引用搜索只找到该 crate 自身，Bazel 文件证明的是同路径 Go library 的使用面。
- Go 对照：`builder.go`、`types.go`、`harness.go`，以及同目录 `crr_sim.go`、`flush_sim.go`、`pd_sim.go`、`pd_sim_service.go`。Rust 独立测试：`builder_test.rs`、`parity_test.rs`、`harness_test.rs`、`pd_sim_test.rs`、`pd_sim_service_test.rs`；Go 对照测试：`builder_test.go`。
- 行为事实由 `parity_test.rs::go_rust_public_contract_matches` 和 `local_storage_matches_go_uri_presign_and_delete_contracts` 覆盖布局、seed、PD 错误、flush wire shape、复制顺序、nil/空路径、恢复检查与本地存储边界；其他独立测试补充同 seed harness 隔离、运行中取消、region 错误种类和 flush 订阅。
- 本任务是纯文档分析，依计划未运行 Cargo。交付验证使用任务规定的 11 章结构命令、`git diff --check` 和目标范围 diff 自审，不把结构验证等同于运行时测试。
