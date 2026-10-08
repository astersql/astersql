# [`pkg/util/cpuprofile/pprof_api.rs`](./pprof_api.rs)

## 文件定位

本文件属于 `astersql-util-cpuprofile` crate，是全局 CPU 采样器与调用方之间的“按请求收集”层：下游从同 crate 的 [`cpuprofile.rs`](./cpuprofile.rs) 接收周期性 `ProfileData`，上游则可通过 `Collector` 获取合并后的 pprof protobuf，或通过传输无关的 `ProfileHTTPHandler` 使用精简的 `HttpRequest`/`HttpResponse` 完成一次 HTTP 风格采集。[`lib.rs`](./lib.rs) 以 `pub use pprof_api::*` 再导出这里的公开符号。

crate 边界由 [`Cargo.toml`](./Cargo.toml) 定义；本文件直接依赖 `crossbeam-channel`、启用 `prost-codec` 的 `pprof` 和 `prost`。当前 Rust 生产代码中，[`pkg/util/profile/profile.rs`](../profile/profile.rs) 的 `Collector::cpu_profile_graph` 会创建本文件的 `Collector`，采集后把 protobuf 转成火焰图行。`ProfileHTTPHandler` 已公开并有端到端测试，但代码搜索未发现 Rust 服务路由调用它；相对地，Go 版本由 [`pkg/server/http_status.go`](../../server/http_status.go) 挂载到 `/debug/pprof/profile`。因此不能把 Rust HTTP 适配器描述为已经接入 Rust 服务主链。

## 核心职责

- 用 `Collector` 把一个调用者注册为进程级并行 CPU profiler 的消费者，并在独立线程中持续接收 `ProfileData`。
- 解码多个 pprof protobuf，校验采样类型兼容性，统一字符串表及 mapping/function/location ID 后合并样本和元数据。
- 输出前仅保留 key 为 `sql` 的 sample label，避免向普通 pprof 使用者暴露 TopSQL 的其他标签。
- 将合并结果编码后写入线程安全的 `ProfileWriter`；`shared_buffer_writer` 提供内存缓冲适配器。
- 实现 Go `net/http/pprof` 风格的 seconds 默认值、WriteTimeout 拒绝、下载响应头及明文错误格式。

本文件不负责启动底层进程级采样循环；那由 `cpuprofile.rs::{StartCPUProfiler, Register, Unregister}` 及其全局 profiler 完成。`Collector::StartCPUProfile` 的含义是开始消费该全局采样器，而不是直接独占原生采样设施。

## 主要符号

- `ProfileWriter = Arc<Mutex<Box<dyn Write + Send>>>`：跨线程共享的输出端。外层 `Arc` 管理所有权，`Mutex` 串行化写入。
- `SharedBufferWriter` 与 `shared_buffer_writer`：把 `Write` 转发到 `Arc<Mutex<Vec<u8>>>`，供内存采集和测试使用；内部缓冲锁中毒会转成 `io::Error`。
- `HttpRequest { seconds, write_timeout }`：只保留本处理器需要的查询参数和服务端写超时；它不是具体 HTTP 框架的 request 类型。
- `HttpResponse { status, headers, body }`：传输无关的响应值，默认状态为 200。
- `ProfileHTTPHandler(&mut HttpResponse, &HttpRequest)`：解析时长、检查写超时、同步等待采集结束并填充 protobuf 响应；开始或停止失败均返回 500。
- `Collector`：一次收集会话的状态机。`data_ch/data_recv` 是容量 1 的 profile 通道，`first_read/first_read_recv` 是首包通知，`cancelled` 控制工作线程退出，`state` 保存合并结果或错误，`wg` 保存 join handle，`started` 防止重复启动。
- `NewCollector()`：创建尚未启动的 collector 和两组 bounded channel。
- `Collector::{StartCPUProfile, StopCPUProfile}`：生命周期入口。公开 API 保持 Go 风格名称，且源码明确允许非 snake case。
- `Collector::{handleProfileData, buildProfileData, removeLabel}`：同步注入/合并、提取结果和标签过滤路径；前两者也用于独立测试以及 `pkg/util/profile/profile.rs` 的测试夹具注入。
- `handle_profile_data`：后台线程与同步入口共用的内部解码/合并函数。
- `merge_profiles`：本地 pprof 合并实现；调用 `validate_compatible_sample_types` 与 `remap_profile_strings`。
- `parse_profile_seconds`、`durationExceedsWriteTimeout`、`serveError`：HTTP 行为辅助函数。
- `labelSQL = "sql"`：唯一保留的标签 key。

## 执行流程

1. 调用者通过 `NewCollector` 创建单次收集器，再调用 `StartCPUProfile(writer)`。若同一实例已经启动，立即返回 `Collector already started`。
2. `StartCPUProfile` 保存 writer、清空共享结果、复位取消标志和遗留首包信号，然后创建工作线程。工作线程先调用 `Register(Some(data_ch))` 向全局 profiler 注册容量 1 的消费者。
3. 工作线程每 10ms 轮询 `data_recv`。收到首个数据后，无论该包最终成功还是报错，都会尽力向 `first_read` 发通知；有效包经 protobuf 解码后写入或合并进 `CollectorState::result`，带 `ProfileData::Error` 的包则记录错误并终止循环。
4. 调用者在所需采样窗口后调用 `StopCPUProfile`。未启动的 collector 直接成功返回；已启动的 collector 最多等待 `profile_duration() * 2` 获取首包，然后设置取消标志、join 工作线程。线程退出前执行 `Unregister(Some(data_ch))`。
5. `buildProfileData` 先传播已记录错误；无结果时返回 `Ok(None)`；有结果时克隆 profile 并删除非 `sql` 标签。
6. `StopCPUProfile` 将结果编码为 protobuf，拒绝编码后的空缓冲，再持有 writer 锁执行 `write_all`。没有 profile 数据是成功的空操作。
7. HTTP 路径先写 `X-Content-Type-Options: nosniff`，将缺失、非法或非正的 seconds 归一到 30。若 seconds 大于等于非零 WriteTimeout，`serveError` 返回 400；否则设置下载响应头、启动 collector、睡眠指定秒数、停止 collector，并把共享缓冲复制到响应 body。
8. `merge_profiles` 先按 type/unit 字符串校验两边的 `sample_type`，再合并字符串表并重写 source 中所有字符串下标；随后以 destination 当前最大 ID 为偏移重写 source 的 mapping/function/location 引用。最后取两个时间窗口的并集、补齐或取大值的标量字段，并追加 comment/sample/mapping/location/function。

## 数据与状态

`CollectorState` 在 `Arc<Mutex<_>>` 中保存互斥的 `err` 与 `result`。新启动时二者都会清空；处理错误包时 `err` 被设置，处理正常包时 `result` 保存首份 profile 或累计合并结果。`buildProfileData` 克隆结果后再过滤标签，因此不会就地破坏后台线程保存的 profile。

两个 channel 都是容量 1。profile channel 的容量与 `cpuprofile.rs` 的非阻塞消费者分发配合：慢 collector 不会让全局采样器无限堆积数据，但可能丢掉全通道期间的新 profile。首包 channel 只表达“至少处理过一次接收”，不携带数据；发送使用 `try_send`，避免工作线程阻塞。

合并时维护以下不变量：sample type 的数量、type 字符串和 unit 字符串必须一致；所有 source 字符串索引必须落在其原字符串表内；非零 mapping/function/location 引用随对应偏移一起变化；零 ID 保持零。时间窗口使用饱和算术，避免溢出。destination 已有的 period type、frame 过滤器和默认采样类型优先保留，缺失时才从 source 补入。

`started` 只从 `false` 变为 `true`，停止时不会复位；因此同一个 `Collector` 是单次启动对象，安全重采集应创建新实例。公开的 Go 对照也保留这一语义。源码注释和 Go API 都指出 Collector 本身不是可并发调用的对象；并发请求应各自持有独立实例。

## 依赖与调用关系

RustCodeGraph 显示 `ProfileHTTPHandler` 下调用 `parse_profile_seconds`、`durationExceedsWriteTimeout`、`NewCollector`、`StartCPUProfile`、`StopCPUProfile`、`shared_buffer_writer` 和 `serveError`；`StartCPUProfile` 下调用 `cpuprofile.rs::{Register, Unregister}`；`StopCPUProfile` 下调用 `cpuprofile.rs::profile_duration` 与 `buildProfileData`；`merge_profiles` 下调用兼容性校验和字符串重映射。

明确的 Rust 上游包括：

- `pkg/util/profile/profile.rs::Collector::cpu_profile_graph`：采集 CPU profile 并转换为 performance schema 使用的火焰图行。
- `pkg/util/cpuprofile/cpuprofile_test.rs`、`pprof_api_test.rs` 和 `migration_aster_unit_test.rs`：验证并发 collector、错误传播、HTTP 行为与 Go 对齐。

相邻但不直接调用本文件 collector 的 `pkg/util/topsql/collector/cpu.rs` 直接使用 `cpuprofile.rs::Register/Unregister` 消费同一全局数据源。这说明本文件与 TopSQL 是并行消费者关系，而非上下级关系。Cargo 搜索还显示 `pkg/util/profile`、`pkg/util/topsql`、`pkg/util/topsql/collector`、`pkg/server` 等 crate 声明了 cpuprofile 依赖；依赖声明本身不等于本文件 API 已被调用。

## 错误处理与边界

- 重复启动同一 collector 返回显式错误；未启动即停止返回 `Ok(())`。
- 首包等待超时不会单独报错；没有结果时停止成功但不写数据，这由 `pprof_api_test.rs::stop_without_profile_data_matches_go_timeout_and_result` 固化。
- `ProfileData::Error`、protobuf 解码/编码错误、采样类型不兼容、字符串下标越界、writer 锁中毒和写失败均沿 `CpuProfileError` 返回。
- collector state 使用 `expect` 处理锁中毒，工作线程 panic 会在 join 时转换为 `collector worker panicked`；HTTP 最终复制输出缓冲时的锁中毒同样使用 `expect`。这些 panic 边界与普通可恢复错误不同。
- `removeLabel` 对非法 label key 不报错，只因查不到 `sql` 而删除该 label；合并阶段的其他字符串索引则严格校验。
- WriteTimeout 仅在存在且非零时生效，seconds 等于超时也拒绝。seconds 无法解析、缺失、为零或负数都回退到 30，而不是返回 400。
- `serveError` 覆盖 Content-Type、设置 `X-Go-Pprof: 1`、删除下载头、清空旧正文，并追加带换行的 UTF-8 文本；已有的其他响应头（如 `X-Content-Type-Options`）保留。
- HTTP 处理是同步阻塞的：当前线程会完整 sleep 指定时长；此文件没有请求取消或客户端断开传播机制。

## 并发与资源生命周期

底层 pprof 原生采样器是进程级资源，但 `cpuprofile.rs` 将一次采样结果分发给多个消费者，所以多个独立 `Collector` 可以并行收集，而无需各自启动原生 sampler。`cpuprofile_test.rs::TestGetCPUProfile` 用十个并发 collector 验证这一模型。

每个 collector 启动一个工作线程。正常停止顺序是：等待首包上限、发布 `SeqCst` 取消、join、由线程注销消费者、构建结果、写入输出。join 保证 `StopCPUProfile` 返回时工作线程和注册关系已经结束。接收使用短超时而非永久阻塞，使取消标志能被定期观察。

资源安全仍依赖调用者配对调用 Stop：`Collector` 没有 `Drop` 实现自动取消或 join；若启动后直接丢弃实例，工作线程持有自己的 `Arc` 和通道端，可能继续运行并保持消费者注册。扩展或封装时必须维持显式停止，或另行设计 RAII guard。共享 writer 的锁只在最终一次 `write_all` 时持有；profile 合并在独立 state 锁下串行执行。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/cpuprofile/pprof_api.go`。两边都实现：seconds 默认 30；达到 WriteTimeout 即拒绝；下载头与 pprof 错误头；单次启动保护；等待最多两个 profile interval；注册/注销消费者；错误包传播；合并多段 profile；输出前仅保留 `sql` 标签；无 profile 时成功空返回。

Rust 用精简请求/响应值取代 Go 的 `http.ResponseWriter`/`*http.Request`，因此需要外部服务器适配。Go 可从 request context 取得 `http.Server.WriteTimeout`，Rust 则要求构造 `HttpRequest` 时显式传入。Go 工作协程以 context cancel 和 `sync.WaitGroup` 管理，Rust 使用 `AtomicBool`、10ms `recv_timeout` 和 `JoinHandle`。Go 依赖 `github.com/google/pprof/profile.Merge`，Rust 在本文件中显式完成字符串与 ID 重映射、元数据合并。

Go 的 `ProfileHTTPHandler` 已在 `pkg/server/http_status.go` 注册为 `/debug/pprof/profile`；当前 Rust 搜索只发现测试调用，没有发现对应 Rust 路由接线。这是迁移状态差异，不应从 Go 路由推断 Rust 已具备同样的线上入口。

测试对应关系包括：Go `cpuprofile_test.go::TestGetCPUProfile/TestProfileHTTPHandler` 与 Rust `cpuprofile_test.rs` 同名用例；Rust `migration_aster_unit_test.rs` 进一步直接验证合并、标签过滤、辅助函数和错误响应；`pprof_api_test.rs` 固化无数据时的等待和空成功语义。

## 扩展指南

- 接入 Rust HTTP 服务时，应在服务器适配层把真实 query 和 WriteTimeout 映射到 `HttpRequest`，再把 `HttpResponse` 的状态、所有 headers 和 body 原样写回；不得直接声称 Cargo 依赖已经完成路由接线。
- 修改采集生命周期时，重点审查 `StartCPUProfile`/`StopCPUProfile` 的注册配对、首包等待、线程 join 与单次实例不变量。若增加 Drop/取消能力，需要新增独立测试覆盖“启动后提前丢弃”和无 profile 通道活动两类路径。
- 修改合并算法时，应在 `migration_aster_unit_test.rs` 或新的同目录独立测试文件中覆盖：不同字符串表、重复字符串、非连续 ID、mapping/function/location 交叉引用、时间窗口、sample type 不兼容和越界索引。不要把 Rust 测试嵌入生产源文件。
- 修改标签策略时，应同步检查 `removeLabel`、TopSQL 在 `pkg/util/topsql/collector/cpu.rs` 对 `sql` 标签的消费，以及 Go `removeLabel` 语义。放宽标签会改变对外 profile 信息暴露面和文件大小。
- 修改 channel 容量或等待策略时，应评估全局 profiler 的非阻塞投递、丢包概率、停止延迟和高并发 profile 请求的内存/线程成本。
- 修改 HTTP 默认值或错误文本时，应同步 `cpuprofile_test.rs::TestProfileHTTPHandler`、`migration_aster_unit_test.rs::http_helpers_match_go_defaults_timeout_and_error_headers`、Go 对照测试及真实路由适配。

## 验证依据

- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点；读取了 `pprof_api.rs` 全部 534 行，并查询了 `ProfileHTTPHandler`、`StartCPUProfile`、`StopCPUProfile`、`handle_profile_data`、`buildProfileData`、`merge_profiles` 的调用关系。图明确给出 handler 到辅助函数、Start 到 Register/Unregister、Stop 到 profile_duration/buildProfileData、merge 到兼容性校验/字符串重映射的边。
- 源码与 crate：`pkg/util/cpuprofile/pprof_api.rs`、`cpuprofile.rs`、`lib.rs`、`Cargo.toml`。
- Rust 上游与测试：`pkg/util/profile/profile.rs`、`pkg/util/cpuprofile/pprof_api_test.rs`、`cpuprofile_test.rs`、`migration_aster_unit_test.rs`、`pkg/util/topsql/collector/cpu.rs`。
- Go 对照与路由：`pkg/util/cpuprofile/pprof_api.go`、`cpuprofile_test.go`、`pkg/server/http_status.go`。
- 全仓文本检索确认：Rust 中 `ProfileHTTPHandler` 的非定义调用仅出现在 cpuprofile 测试；`pkg/util/profile/profile.rs` 是本文件 `Collector` 的生产调用点；Cargo manifest 中存在多个 crate 依赖，但未据此推断调用关系。
- 本任务是纯文档分析，按计划不运行 Cargo；结构验证命令及最终退出状态在任务交付时记录。
