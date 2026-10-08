# `pkg/util/replayer/replayer.rs`

## 文件定位

该文件是 `astersql-util-replayer` crate 的核心实现，crate 入口 `pkg/util/replayer/lib.rs` 通过 `pub mod replayer` 声明模块，并用 `pub use replayer::*` 再导出这里的公开 API。它不负责收集 SQL、执行计划或统计信息，也不负责 ZIP 内容编码；它只提供 Plan Replayer 产物的命名规则、固定对象目录，以及一个将上下文绑定到对象写入器的薄适配层。

当前 Rust 生产链的实际接线范围需要特别区分：`pkg/session/runtime/dispatch.rs::execute_plan_replayer_dump` 调用 `GeneratePlanReplayerFileName` 后自行编码归档并写入全局扩展存储；同文件以及 `pkg/server/http_status.rs::plan_replayer_download_response` 调用 `GetPlanReplayerDirName` 形成写入/下载路径。`GeneratePlanReplayerFile` 和本文件自定义的 `Storage`、`ObjectWriter`、`Context` 目前没有活跃的生产 Rust 调用者，只由 `pkg/util/replayer/migration_aster_unit_test.rs` 直接验证；它们是尚未连接共享 Rust 对象存储抽象的局部移植边界。

crate 边界由 `pkg/util/replayer/Cargo.toml` 定义：运行时直接依赖只有 `base64 = "0.22"` 和 `getrandom = "0.3"`，没有 feature 开关；`[package.metadata.porting]` 将其对应到 Go 包 `pkg/util/replayer`。

## 核心职责

1. `GeneratePlanReplayerFileName` / `generatePlanReplayerFileName` 生成 `{模式前缀}_{随机键}_{纳秒时间戳}.zip`，其中模式前缀必须与下载和清理逻辑所理解的命名约定一致。
2. `GetPlanReplayerDirName` 固定返回相对对象目录 `replayer`，让生产写入端和 HTTP 下载端使用同一目录。
3. `GeneratePlanReplayerFile` 将上述名称和目录拼成 `replayer/{file_name}`，请求 `Storage::create`，再把返回的 `ObjectWriter` 包装成不要求调用者重复传入上下文的 `WriteCloser`。
4. `FileWriter` 保证 `write` 和 `close` 都把创建时绑定的同一个 `Context` 引用传给底层写入器。
5. `PlanReplayerTaskKey`、`PlanReplayerPath` 和 `PlanReplayerPathOnce` 保留 Go 包中的任务键与一次性路径初始化形状；在当前活跃 Rust 代码中尚无调用者，不能据此推断任务调度或本地路径初始化已经接线。

## 主要符号

- `pub type Error = Box<dyn std::error::Error + Send + Sync>`：本 crate 的统一动态错误边界，使随机源、系统时间和存储实现的错误可以通过 `?` 向上传播，并允许错误跨线程边界传递。
- `pub struct Context`：零字段、`Default` 的不透明占位类型。它只用于证明同一上下文会经过 create/write/close；它不是 `std::task::Context`，也尚不是共享 extstore 的真实上下文。
- `pub trait Storage::create(&self, ctx, path, options)`：按字符串相对路径创建 `ObjectWriter`。当前 options 固定传 `None`，类型暂为 `Option<()>`。
- `pub trait ObjectWriter::{write, close}`：底层对象写入器接口；两个操作都显式接收 `&Context`。
- `pub trait WriteCloser::{write, close}`：提供给本 crate 调用方的简化接口；上下文已由适配器绑定。
- `const planReplayerDirName: &str = "replayer"`：外部存储相对目录的唯一内部常量。
- `pub struct PlanReplayerTaskKey { SQLDigest, PlanDigest }`：由 SQL digest 与 plan digest 组成，可克隆、比较并哈希的任务标识。字段保持 Go 风格名称是因为文件级 `#![allow(non_snake_case, non_upper_case_globals)]`；当前活跃 Rust 调度代码 `pkg/domain/plan_replayer.rs` 定义并使用的是它自己的任务键类型，而不是此类型。
- `pub fn GeneratePlanReplayerFile(...) -> Result<(Box<dyn WriteCloser>, String), Error>`：完成命名、路径拼接、对象创建和适配器构造，返回写入器与不含目录的文件名。
- `pub fn NewFileWriter(ctx, writer) -> Box<dyn WriteCloser>`：构造私有 `FileWriter`，隐藏底层对象写入接口和上下文参数。
- `struct FileWriter` 及其 `impl WriteCloser`：仅保存一个 `Context` 和一个 `Box<dyn ObjectWriter>`，逐次原样转发写入长度、错误和关闭结果。
- `pub fn GeneratePlanReplayerFileName(...)`：公开命名入口，仅委托私有 `generatePlanReplayerFileName`。
- `fn generatePlanReplayerFileName(...)`：模式选择、时间戳、随机键和最终字符串拼装的唯一实现。
- `pub static PlanReplayerPath: Mutex<String>` / `pub static PlanReplayerPathOnce: Once`：分别对应 Go 的全局路径和 `sync.Once`。两者在本文件内只声明不读取，且 Rust 类型额外要求持锁访问字符串。
- `pub fn GetPlanReplayerDirName() -> &'static str`：返回固定目录常量，无分配、无 I/O。

## 执行流程

公开命名流程从 `GeneratePlanReplayerFileName` 进入：

1. `generatePlanReplayerFileName` 用 `SystemTime::now().duration_since(UNIX_EPOCH)` 取得当前时刻；若系统时间早于 Unix epoch，立即返回错误。
2. 将纳秒值转为 `i64`，然后调用 `getrandom::fill` 填充 16 字节随机数组；随机源失败会包装为 `std::io::Error`，消息包含底层原因。
3. 使用 `base64::engine::general_purpose::URL_SAFE` 编码随机字节。16 字节产生 24 字符、带 `==` 填充的 URL-safe key。
4. 严格按分支顺序选前缀：`isContinuesCapture` 为真，或 `isCapture && enableHistoricalStatsForCapture` 为真时选择 `capture_replayer`；否则当 `isCapture && !enableHistoricalStatsForCapture` 时选择 `capture_normal_replayer`；其余组合选择 `replayer`。
5. 返回 `{prefix}_{key}_{timestamp}.zip`。连续捕获优先级最高，因此即使 `isCapture` 为假仍得到 `capture_replayer`。

完整写入器创建流程从 `GeneratePlanReplayerFile` 进入：先生成文件名；失败时不会调用存储。成功后用 `format!("{}/{file_name}", GetPlanReplayerDirName())` 形成相对路径，调用 `storage.create(&ctx, object_path, None)`；创建失败时直接返回错误。创建成功才调用 `NewFileWriter`，将传入的 `Context` 移入 `FileWriter`，并同时把裸文件名返回给调用者。后续 `WriteCloser::write` 与 `close` 分别转发到相同底层 writer 的 `ObjectWriter::write` 与 `close`。

当前生产 SQL 路径没有走这条完整创建流程：`pkg/session/runtime/dispatch.rs::execute_plan_replayer_dump` 只复用名称和目录约定，通过 `astersql-planner-extstore` 的 `WriteFile` 一次性写入编码后的归档。下载路径 `pkg/server/http_status.rs::plan_replayer_download_response` 使用相同目录查询并读取文件。

## 数据与状态

生成文件名由三类数据组成：模式布尔值决定稳定前缀，16 字节操作系统随机数降低同纳秒内碰撞概率，`SystemTime` 的 Unix 纳秒值支持按文件名解析时间。文件名本身不携带 SQL digest、plan digest 或归档内容信息。

`PlanReplayerTaskKey` 是纯值对象，`Eq + Hash` 允许作为集合或映射键，但本 crate 不维护任务集合。当前活跃 domain 实现使用 `pkg/domain/plan_replayer.rs::PlanReplayerTaskKey` 和 `BTreeSet`，两者不可互换。

`FileWriter` 独占 `Box<dyn ObjectWriter>` 与一个 `Context`。它不缓存字节、不累计写入位置、不记录是否已经关闭，所有这些语义由具体 `ObjectWriter` 决定。`PlanReplayerPath` 是进程级可变字符串，初始为空；`Mutex` 防止并发数据竞争。`PlanReplayerPathOnce` 是独立的进程级一次性门闩，但本文件没有把两个静态量组合成初始化函数，因此调用方若未来使用它们，必须自行在 `call_once` 闭包内取得路径锁并写入。

## 依赖与调用关系

RustCodeGraph 对 `GeneratePlanReplayerFile` 给出的直接下游边为 `generatePlanReplayerFileName`、`Storage::create`、`NewFileWriter` 和 `GetPlanReplayerDirName`。`FileWriter::write/close` 的语义下游是所持 `ObjectWriter` 的同名方法；动态派发的具体实现由调用方提供。

活跃 Rust 上游经 `rg` 核对如下：

- `pkg/session/runtime/dispatch.rs::execute_plan_replayer_dump` 调用 `GeneratePlanReplayerFileName(false, false, false)`，并用 `GetPlanReplayerDirName` 构造上传路径。
- `pkg/server/http_status.rs::plan_replayer_download_response` 调用 `GetPlanReplayerDirName` 构造下载查询路径。
- `pkg/server/handler/optimizor/plan_replayer_test.rs::plan_replayer_file_names_preserve_capture_modes` 从消费者侧验证三个代表性前缀和目录名。
- `pkg/util/replayer/migration_aster_unit_test.rs` 是本 crate 的独立测试模块，覆盖所有八种布尔组合、写入/关闭转发以及完整创建流程的相对路径。

`pkg/util/replayer/Cargo.toml` 的 `base64` 提供 URL-safe 编码，`getrandom` 提供操作系统随机字节；标准库提供系统时间、`Mutex` 与 `Once`。反向 Cargo 声明存在于 `pkg/session`、`pkg/server`、`pkg/domain`、`pkg/executor` 及相关 plan-replayer 测试 crate，但 Cargo 依赖本身不等于某个 API 已被调用。

## 错误处理与边界

- 系统时间早于 Unix epoch 时，`duration_since` 的错误通过 `Error` 原样向上传播，且随机源尚未读取。
- `getrandom::fill` 失败时转换为 `std::io::Error::other("failed to read secure random bytes: ...")`，保留可诊断文本；此时不生成文件名。
- `Storage::create` 失败时，`GeneratePlanReplayerFile` 不构造 `FileWriter`，调用者得到存储错误；文件创建是否留下部分对象由存储实现决定。
- `FileWriter::write` 返回底层报告的实际写入字节数，不保证一次写完全部输入；本层不重试、不循环补写，也不吞掉错误。
- `FileWriter::close` 仅转发一次调用，不提供幂等保护，也没有 `Drop` 自动关闭实现。调用者必须显式关闭并处理错误。
- 布尔组合并非三个独立标签：连续捕获会覆盖其他模式；只有“捕获且未启用历史统计”得到 `capture_normal_replayer`。这条分支顺序是 HTTP 处理逻辑依赖的兼容约定。
- 纳秒值先由 `u128` 强制转换为 `i64`；当前日期范围可用，但代码没有对极远未来的截断做显式检查。
- Rust 版本没有 Go 的 `InjectPlanReplayerFileNameTimeField` failpoint，因此无法在测试中固定时间戳；依赖精确时间的 Go GC 测试语义尚未完整迁移到本文件。

## 并发与资源生命周期

文件名生成不共享可变状态：每次调用独立读取系统时间与随机源，适合并发调用。随机键加纳秒时间降低重名风险，但函数没有全局去重表，也不承诺数学上的绝对唯一性。

`FileWriter` 持有 `Box<dyn ObjectWriter>`，接口使用 `&mut self`，因此同一适配器的 write/close 在安全 Rust 中需要独占可变访问。本文件没有为 `Storage`、`ObjectWriter`、`WriteCloser` 声明 `Send` 或 `Sync` 上界，不能仅凭 `Error: Send + Sync` 推断 writer 可以在线程间移动或共享。创建时的 `Context` 与 writer 同寿命存在于适配器中，直到 `FileWriter` 被丢弃；丢弃不会自动调用 `close`。

`PlanReplayerPath` 的每次访问必须取得互斥锁；锁中毒如何处理由未来调用方决定。`PlanReplayerPathOnce` 能跨线程保证初始化闭包最多成功执行一次，但当前没有所有者或调用点。生产写入和下载所使用的 extstore 生命周期位于 `pkg/session/runtime/dispatch.rs` 与 `pkg/server/http_status.rs`，不由本文件管理。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/replayer/replayer.go`。Rust 保留了 Go 的目录常量、任务键字段、三个命名布尔参数、前缀选择顺序、16 字节随机 key、URL-safe padded Base64、Unix 纳秒后缀、ZIP 扩展名，以及把同一上下文传给 create/write/close 的适配行为。

已确认的差异如下：

- Go 使用真实的 `context.Context`、`storeapi.Storage`、`objectio.Writer` 与 `io.WriteCloser`；Rust 当前定义本地占位 `Context` 和最小 traits，尚未接到 `astersql-planner-extstore` 或 `pkg/objstore` 的共享接口。
- Go 用 `filepath.Join` 拼接路径；Rust用 `format!("{}/...")` 固定 `/`。目标是对象存储相对 key，这与当前测试预期一致，但若未来改成本地文件系统路径应重新审视平台语义。
- Go 用 `errors.AddStack` 给命名和创建错误附加栈；Rust 只通过动态错误传播，除随机源转换外不增加上下文。
- Go 的命名函数含 failpoint，可固定纳秒值支持 GC 边界测试；Rust 没有等价注入点。
- Go 的全局 `PlanReplayerPath string` 可以无锁读取，约定由 `sync.Once` 初始化；Rust 将字符串放入 `Mutex`，但 `Once` 与路径没有封装为不可误用的 API。
- Rust 将系统纳秒 `as_nanos()` 的 `u128` 转为 `i64`，Go 的 `UnixNano()` 直接返回 `int64`。
- 当前 Rust 生产 dump 流程绕过 `GeneratePlanReplayerFile`，而 Go domain 流程直接调用它。Rust 的完整创建适配仅有 crate 测试证据，不能宣称已经达到 Go 的生产接线覆盖。

Go 测试 `pkg/domain/plan_replayer_test.go::TestPlanReplayerDifferentGC` 通过 failpoint 构造不同时间的 capture/normal 文件并验证垃圾回收，`TestDumpGCFileParseTime` 验证八种模式生成的名称都能被时间解析器接受。Rust 独立测试覆盖命名分支、URL-safe key 形状、相对目录以及转发行为，但未复刻可控时间与 GC 集成场景。

## 扩展指南

- 新增或修改命名模式时，应只在 `generatePlanReplayerFileName` 集中改变分支，并同步检查 `pkg/server` 的下载/识别逻辑、domain 的时间解析/GC 逻辑及 Go 对照。至少扩展 `pkg/util/replayer/migration_aster_unit_test.rs::test_generate_plan_replayer_file_name_matches_go_branches`，必要时也更新消费者测试 `pkg/server/handler/optimizor/plan_replayer_test.rs::plan_replayer_file_names_preserve_capture_modes`。前缀变更具有文件兼容与清理遗漏风险。
- 若把 `GeneratePlanReplayerFile` 接入生产，应优先用共享 extstore 的 Context/Storage/Writer 接口替换或适配本地占位 traits，而不是创建第二套长期并行抽象；同时增加独立测试文件验证 create 失败、部分写入、close 失败与同一上下文转发。需评估动态派发和逐块写入的性能，但不得把测试写入 `replayer.rs`。
- 若需要确定性时间测试，可抽象时钟或提供受测试配置约束的注入点，并复刻 Go 的 GC 边界意图；不要为了测试改变公开文件格式。
- 若启用 `PlanReplayerPath`，应提供封装函数把 `Once` 与 `Mutex` 的操作绑定起来，并明确锁中毒、重复初始化和空路径语义；相应测试继续放在独立的 `migration_aster_unit_test.rs` 或新的同目录测试文件。
- 若要统一任务键，应先核对 `pkg/domain/plan_replayer.rs::PlanReplayerTaskKey` 的排序需求和字段语义；本类型只有 `Eq + Hash`、没有 `Ord`，不能直接替换 domain 的 `BTreeSet` 键。
- 修改 API 时必须保持 `lib.rs` 再导出和各消费者 Cargo 依赖可用，并注意 Go 风格公开名当前依赖 crate 的 lint allowance。任何功能修改都应先做失败回归测试，再修复生产代码；本说明任务本身不修改运行时行为。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/util/replayer` 列出 `lib.rs`、`replayer.rs`、独立测试和 Go 对照；`node --file pkg/util/replayer/replayer.rs --offset 1 --limit 500` 读取目标文件全部 172 行；`query GeneratePlanReplayerFile --kind function` 定位 Rust/Go 同名实现；`callees GeneratePlanReplayerFile` 确认 Rust 的四条核心下游边。对同名方法和动态派发无法可靠消歧之处没有强行采用图结果。
- Rust 源与入口：`pkg/util/replayer/replayer.rs`、`pkg/util/replayer/lib.rs`。
- crate 与反向依赖：`pkg/util/replayer/Cargo.toml`，以及通过 Cargo 文本搜索确认的 `pkg/session/Cargo.toml`、`pkg/server/Cargo.toml`、`pkg/domain/Cargo.toml`、`pkg/executor/Cargo.toml` 等。
- 活跃 Rust 调用点：`pkg/session/runtime/dispatch.rs::execute_plan_replayer_dump`、`pkg/server/http_status.rs::plan_replayer_download_response`。
- 独立 Rust 测试：`pkg/util/replayer/migration_aster_unit_test.rs`；消费者测试：`pkg/server/handler/optimizor/plan_replayer_test.rs::plan_replayer_file_names_preserve_capture_modes`。
- Go 对照与测试：`pkg/util/replayer/replayer.go`、`pkg/domain/plan_replayer_test.go::TestPlanReplayerDifferentGC`、`pkg/domain/plan_replayer_test.go::TestDumpGCFileParseTime`；`pkg/domain/extract.go` 另证实 Go 的 `NewFileWriter` 被生产代码复用。
- 人工边界复核：文档将已接线的名称/目录 API、仅测试覆盖的本地存储适配层和未使用的兼容静态量分别陈述；未把 Cargo 依赖、注释或 Go 行为冒充 Rust 活跃调用事实。
