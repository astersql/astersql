# `pkg/util/cpuprofile/cpuprofile.rs`

## 文件定位

本文件是 `astersql-util-cpuprofile` crate 的进程级 CPU 采样核心。crate 入口 `pkg/util/cpuprofile/lib.rs` 以私有 `mod cpuprofile` 装配本文件，再用 `pub use cpuprofile::*` 重导出公开 API；同一入口还装配 `pprof_api.rs`，后者把这里周期产出的 pprof protobuf 片段合并成按需采集结果。`pkg/util/cpuprofile/Cargo.toml` 将 crate 对应到 Go 包 `pkg/util/cpuprofile`，直接对照文件是 `pkg/util/cpuprofile/cpuprofile.go`。

它解决的是“进程只能有一个底层 CPU sampler，但可以有多个上层消费者”的冲突：一个后台 worker 独占 `pprof-rs` 的进程级采样器，每个固定区间结束后把同一份 `Arc<ProfileData>` 非阻塞广播给全部注册者。当前 Rust 生产消费者之一是 `pkg/util/topsql/collector/cpu.rs::SQLCPUCollector`；同 crate 的 `pkg/util/cpuprofile/pprof_api.rs::Collector` 也通过 `Register` / `Unregister` 订阅片段。

需要区分真实实现与迁移桩：`cmd/tidb-server/main.rs` 当前从 `stubs` 导入 `cpuprofile`，其启动/退出调用命中 `cmd/tidb-server/stubs.rs` 的事件记录桩，并未直接启动本文件的全局 profiler。真实 crate 已被 `pkg/server`、TopSQL collector、TopSQL 和 `pkg/util/profile` 等 Cargo manifest 声明为依赖，且 server testkit 会直接调用真实 `StartCPUProfiler` / `StopCPUProfiler`。因此本文件不是桩，但完整 server 主入口尚未接到它。

本文件没有 `#[cfg(...)]` 条件分支，测试也没有内嵌在生产源文件中；独立测试由 `lib.rs` 的 `#[cfg(test)] mod cpuprofile_test` 和 `migration_aster_unit_test` 接入。

## 核心职责

1. 用 `parallelCPUProfiler` 串行管理进程级 `pprof::ProfilerGuard` 的启动、停止和后台线程生命周期，并拒绝同一实例重复启动。
2. 维护去重后的消费者 sender 集合；注册时唤醒 worker，无消费者时不启动下一轮采样。
3. 每轮以 100 Hz 构造 `pprof-rs` guard，结束时编码为 pprof protobuf 字节，并把成功或失败统一包装成 `ProfileData`。
4. 对所有消费者执行 `try_send`，让慢消费者或已断开的 channel 不阻塞采样循环，也不影响其它消费者。
5. 通过 `OnceLock<Mutex<parallelCPUProfiler>>` 提供进程内唯一的公开全局入口，并提供少量间隔、计数和状态重置辅助 API 供独立测试验证。

## 主要符号

- `DefProfileDuration: AtomicU64`：公开的默认采样区间，单位毫秒，初值 1000。`set_profile_duration` 把零时长提升为 1 ms、把超出 `u64` 的毫秒数截到 `u64::MAX`；`profile_duration` 再次保证返回值至少 1 ms。
- `CPU_PROFILE_COUNTER: AtomicU64` / `cpu_profile_count()`：私有全局计数器及公开读入口。每次存在消费者并准备创建新 guard 时递增；它统计“尝试开启的轮次”，不是成功报告数或消费者收到的数据包数。
- `CpuProfileError { message }`：本 crate 的可克隆字符串错误。`new` 接受任意 `Into<String>`；文件为 `pprof::Error`、`prost::{DecodeError, EncodeError}` 和 `std::io::Error` 提供转换，其中 Decode/I/O 转换主要供同 crate 的其它模块复用。
- `ProfileConsumer`：`crossbeam_channel::Sender<Arc<ProfileData>>` 的类型别名。sender 身份用于去重，`Arc` 让一次编码结果可被多个 channel 共享。
- `ProfileData { Data, Error }`：一轮结果。`success` 产生非错误字节，`failure` 产生空 `Data` 和错误；字段保持 Go 风格公开命名。
- `ProfilerCore`：后台线程和控制对象共享的内部状态，包含受 `Mutex` 保护的消费者向量、容量为 1 的注册/停止唤醒 channel、`stopped` 原子标志和上一轮数据长度。
- `ProfilerCore::consumers_count` / `send`：分别在锁内读取消费者数和非阻塞广播。`send` 忽略 `try_send` 的 Full/Disconnected 结果，不自动删除失效 sender。
- `parallelCPUProfiler`：可启停的控制对象。`core` 跨线程共享，`wg` 保存 worker join handle，`started` 防止重复启动；`profileData` 只用于测试注入和 `sendToConsumers`，正常后台采样数据不存入该字段。
- `newParallelCPUProfiler`：建立空消费者集合与 bounded(1) 唤醒 channel，返回未启动实例。
- `errProfilerAlreadyStarted`：产生固定消息 `parallelCPUProfiler is already started`，保持 Go 测试可见语义。
- `parallelCPUProfiler::{start, stop, register, unregister, consumersCount}`：实例级生命周期与消费者管理 API。`set_profile_data`、`has_profile_data`、`sendToConsumers` 是测试注入辅助。
- `profiling_loop`：worker 主循环；等待通知或区间超时，结束旧 guard、广播结果，再按当前消费者数决定是否开启新 guard。
- `finish_profile`：构建 report、转为 pprof protobuf、按上一轮大小的下一个 4096 字节档位预分配缓冲并编码；空编码也转为显式失败。
- `global_profiler`：通过函数内 `OnceLock` 延迟构造进程唯一的 `Mutex<parallelCPUProfiler>`。
- `StartCPUProfiler` / `StopCPUProfiler` / `Register` / `Unregister`：公开全局 API。注册与注销的 `None` 都是 no-op。
- `global_consumers_count` / `reset_global_profiler_for_test`：测试观察与隔离辅助；后者先停止 worker，再清空消费者和注入数据。

## 执行流程

全局启动从 `StartCPUProfiler` 开始：获取全局 mutex，调用实例 `start`；若 `started` 已为真则返回固定错误，否则先置位、清除 `stopped`，排空唤醒 channel 中可能遗留的 token，再 spawn worker。worker 外层用 `catch_unwind` 捕获 panic 并记录日志，使 panic 不跨线程传播；handle 留在 `wg` 供停止时 join。

注册流程由 `Register(Some(sender))` 进入：持有全局 mutex 后调用实例 `register`，再锁住消费者向量，以 `Sender::same_channel` 判断同一 channel 是否已经存在；不存在才追加。释放消费者锁后向 bounded(1) 通知 channel 执行 `try_send(())`。通知只是唤醒提示，合并多个注册事件不会丢失消费者状态，因为真实集合保存在 mutex 内。

`profiling_loop` 每次先对通知 channel 执行 `recv_timeout(profile_duration())`。无论是注册唤醒、超时还是 channel 错误，都会先检查 `stopped`。如果上一轮 `active` guard 存在，循环调用 `finish_profile`，随后显式 drop guard 结束底层采样，记录实际字节长度并广播结果。只有完成旧轮后才重新读取消费者数量；数量为零就继续等待，不建立新 sampler。

若仍有消费者，循环先递增 `CPU_PROFILE_COUNTER`，再用 `ProfilerGuardBuilder` 设置 100 Hz 频率，并 blocklist `libc`、`libgcc`、`pthread`、`vdso`。构造成功时 guard 保存为下一轮的 `active`；失败时立即广播 `ProfileData::failure`。因此首次注册只触发新一轮开始，最早的成功数据要到下一次通知或区间超时结束该轮后才产生。

`finish_profile` 调用 `guard.report().build().and_then(report.pprof())`；成功后用 `(last_data_size / 4096 + 1) * 4096` 计算容量，编码 protobuf。report 构建、pprof 转换、编码或空输出都转成 `ProfileData`，不让普通采样错误终止 worker。

停止从 `StopCPUProfiler` 进入。未启动时直接返回；已启动时清除 `started`、设置 `stopped`、尝试发送唤醒 token，并取出 handle join。worker 观察停止标志后退出，最后直接 drop 尚在运行的 guard，不广播不完整尾包。join 完成后才记录 stopped，因此返回意味着该实例不再持有底层 sampler。

## 数据与状态

进程全局状态分成三层：`OnceLock` 保证全局控制对象只初始化一次；外层 `Mutex<parallelCPUProfiler>` 串行化公开启停和注册操作；`ProfilerCore` 中的独立 mutex/原子/channel 允许 worker 在不持有全局 mutex 时读取消费者、广播和响应停止。

消费者以 `Vec<Sender<_>>` 保存，并按 channel 身份去重，顺序即首次注册顺序。注销用 `retain` 移除所有同 channel sender；不存在的 sender 是 no-op。Full 或 Disconnected 的 sender 在广播时都只导致当前包被丢弃，不从向量自动清理，所以调用者仍应显式 `Unregister`，否则消费者计数会继续非零并驱动无用采样。

一个 `ProfileData` 表示完整区间或该轮启动/编码失败。成功对象通常 `Error == None` 且 `Data` 非空；构造器没有禁止 `success(Vec::new())`，但真实 `finish_profile` 会把空编码改成失败。广播时所有消费者收到同一个 `Arc` 指向的不可变对象，字节向量不会为每个消费者复制。

`last_data_size` 只保存最近一次完成结果的 `Data.len()`；错误结果因 `Data` 为空会把它重置为零。预分配公式即使上一轮大小恰为 4096 的倍数也会多分一个 4096 档位，目的是给下一轮增长留余量，不影响最终编码长度。

`parallelCPUProfiler.profileData` 与 Go 结构同名，但当前 Rust 正常循环使用局部 `active` 和局部结果，不写这个字段；它仅服务 `set_profile_data` / `sendToConsumers` 的非阻塞分发测试。扩展者不能把 `has_profile_data` 当作后台 profiler 是否正在采样的状态查询。

## 依赖与调用关系

`pkg/util/cpuprofile/Cargo.toml` 的直接依赖为：`crossbeam-channel` 提供有界通知与数据 channel、sender 身份判断及非阻塞发送；`pprof`（启用 `prost-codec`）提供采样 guard 和 protobuf profile；`prost` 提供编码 trait/错误；`thiserror` 派生错误展示；`log` 记录启停和线程异常。唯一 dev dependency `serial_test` 用于串行化会争用进程级 sampler 的测试。

RustCodeGraph 将本文件识别为 36 个符号，并显示被 122 个索引文件使用；精确 `callers` / `callees` 对这些函数未返回边，因此调用关系又以窄范围符号引用核对。确认的直接上游包括：

- `pkg/util/cpuprofile/pprof_api.rs::Collector::StartCPUProfile` 在 worker 中注册自身的 bounded(1) sender，接收区间片段并合并，停止时注销；它依赖 `profile_duration()` 决定首包等待上限。
- `pkg/util/topsql/collector/cpu.rs::SQLCPUCollector` 根据 TopSQL 开关调用 `cpuprofile::Register` / `Unregister`，解码 `ProfileData` 后按 SQL/plan 标签聚合 CPU 时间。
- `pkg/util/profile/profile.rs` 使用同 crate Collector/`ProfileData` 做 profile 处理；相关独立测试和 TopSQL 测试直接启动全局 profiler。
- `pkg/server/tests/servertestkit/testkit.rs` 直接从 `astersql_util_cpuprofile` 导入真实启停 API，给 server 测试生命周期使用。

crate 依赖声明存在于 `pkg/server/Cargo.toml`、`pkg/server/tests/servertestkit/Cargo.toml`、`pkg/util/topsql/{Cargo.toml,collector/Cargo.toml}` 和 `pkg/util/profile/Cargo.toml`。仓库 facade `pkg/lib.rs` 还在 `util::cpuprofile` 下重导出该 crate。

下游调用链集中在本文件内部：公开 API经 `global_profiler` 进入实例方法；`start` spawn `profiling_loop`；循环调用 `profile_duration`、`finish_profile`、`ProfilerCore::{consumers_count,send}` 和 `pprof::ProfilerGuardBuilder`。没有 SQL、网络、磁盘或存储访问，profile 字节的解析和业务聚合由消费者负责。

## 错误处理与边界

- 重复 `start` 返回 `CpuProfileError`，不会创建第二个 worker；重复 `stop` 安全且立即返回。成功停止后同一实例可以再次启动。
- `Register(None)` / `Unregister(None)` 是 no-op；重复注册同一 channel 只保留一项；注销不存在的 channel 也是 no-op。
- 广播严格非阻塞。消费者 channel 已满时丢弃最新包；receiver 已断开时同样忽略错误。这样保护采样线程，但调用者没有补发、排队或丢包通知保证。
- `pprof-rs` 使用进程级 sampler。被其它 guard 占用时，本轮 builder 错误会立即送给全部消费者；只要消费者仍在，后续循环还会继续尝试，而不是停止 profiler。
- report 构建、pprof 转换、protobuf 编码和空编码均作为数据面错误广播。此类错误不从 `profiling_loop` 返回。
- 所有 mutex 以 `expect(... poisoned)` 处理 poisoning；锁中毒会 panic。worker 主体的 panic 被 `catch_unwind` 记录后结束，但不会自动把 `started` 改回 false，也不会把异常作为 `ProfileData` 发送；控制方仍需 `stop` 来 join 和复位。
- `stop` 对当前 active guard 只 drop、不发送尾包，保证消费者只看到完成区间。最后一个消费者注销后，已有 guard 要等下次通知或超时才完成并广播一次；之后才因消费者数为零停止建立新 guard。测试以最终能重新取得原生 sampler 验证释放，而不是要求注销瞬间同步释放。
- `set_profile_duration` 的原子存储不会改变当前已经开始的 `recv_timeout`；新间隔在下一次循环等待时生效。
- `CPU_PROFILE_COUNTER` 使用 relaxed 原子且可能在极长运行后按 `u64` 溢出；它只用于观察，不参与控制正确性。

## 并发与资源生命周期

全局对象的公开控制面由 `Mutex<parallelCPUProfiler>` 串行化。实例 `start` 在持有该锁时 spawn 后立即返回；worker 只持有 `Arc<ProfilerCore>`，不会反向申请全局 mutex，所以 `StopCPUProfiler` 持有全局锁并 join 时不会与 worker 形成同锁死锁。消费者 mutex 的临界区只覆盖集合操作或一轮 `try_send`；发送非阻塞，锁不会因慢 receiver 长期占用。

通知 channel 容量为 1，注册和停止都用 `try_send`。多个并发通知可以合并，但 worker 醒来后读取的是原子停止标志和完整消费者集合，控制状态不会依赖 token 数量。`stopped` 采用 `SeqCst` 保证控制线程与 worker 之间的停止可见性；计数和上一包大小只用于统计/容量提示，使用 `Relaxed`。

每轮最多持有一个 `ProfilerGuard<'static>`。guard 从建立到下一次唤醒/超时覆盖一个采样区间，完成报告后显式 drop；worker 退出也 drop active guard。`wg: Option<JoinHandle<()>>` 确保每次启动只有一个可 join handle，停止取走后才允许下一轮完整生命周期。

`parallelCPUProfiler` 没有实现 `Drop`。局部实例若启动后直接被丢弃，会分离仍持有 `Arc<ProfilerCore>` 的线程，因没有设置停止标志而可能继续运行；正确使用必须成对调用 `start` / `stop`，公开全局 API通过进程常驻对象避免控制对象提前析构。新增封装应像测试的 `RunningGlobalProfiler` 一样提供 RAII 收尾。

消费者的生命周期由注册方负责。sender clone 注册后，即使调用方丢弃自己的 clone，集合仍持有 sender；必须用同 channel 的 sender clone 调用 `Unregister` 才能移除。数据用 `Arc` 跨线程共享，最后一个 receiver 释放后自动回收。

## 与 Go 版本的对应关系

`pkg/util/cpuprofile/cpuprofile.go` 是行为基准。公开 `DefProfileDuration`、`ProfileConsumer`、`ProfileData`、`StartCPUProfiler`、`StopCPUProfiler`、`Register`、`Unregister`，以及内部 `parallelCPUProfiler`、重复启动错误、消费者计数和非阻塞发送，均有直接对应。两端都保证单个进程级 sampler、多消费者共享、无消费者不继续采样、满 channel 丢弃最新包、重复启停语义和按上一轮大小以 4096 为单位预分配。

Rust 用 `pprof-rs::ProfilerGuard` 替代 Go `runtime/pprof.StartCPUProfile` / `StopCPUProfile`：旧 guard 在新 guard 建立前生成报告并 drop，对应 Go `doProfiling` 先 Stop、发送旧 buffer、再 Start 的顺序。Rust 的 builder 固定 100 Hz 并带 native blocklist，这是实现层差异；Go runtime 的采样频率/过滤由运行时决定。

并发表达不同：Go 用结构体 mutex、`context.CancelFunc`、ticker、无缓冲通知 channel和 `WaitGroup`；Rust 用外层全局 mutex、共享 core、`AtomicBool`、`recv_timeout`、bounded(1) 通知 channel 和 `JoinHandle`。Go 的 `profileData != nil` 同时表示 active buffer；Rust 正常路径以局部 `Option<ProfilerGuard>` 表示 active 状态，结构体 `profileData` 只保留作测试辅助。

数据表达也有差异：Go `ProfileData.Data` 是可写 `*bytes.Buffer`，Rust 是拥有所有权的 `Vec<u8>`；Go consumer 是可比较的双向 channel，Rust公开别名是 sender，并用 `same_channel` 去重；Rust 通过 `Arc<ProfileData>` 共享只读结果。Rust `Option<ProfileConsumer>` 显式表达 Go nil channel，避免对不存在 sender 调用方法。

Go `sendToConsumers` 用 recover 防止向已关闭 channel 发送导致 panic；Rust receiver 断开只会让 `try_send` 返回错误，所以直接忽略即可。Go 通过 `metrics.CPUProfileCounter.Inc()` 写全局指标；Rust 当前只更新文件私有 `AtomicU64`，没有连接 metrics crate，这是可观测性差异。

`pkg/util/cpuprofile/cpuprofile_test.go` 与 Rust 的 `cpuprofile_test.rs` 保持主要意图：重复启动/停止、nil 与重复消费者、占用 sampler 的错误、有效 protobuf、注销后释放 sampler、并发 Collector 和 HTTP profile。Rust 额外的 `migration_aster_unit_test.rs` 聚焦去重、满 channel 丢包和真实数据交付。Rust 测试用 `#[serial]` 防止同一测试进程争抢全局 sampler。

## 扩展指南

新增采样参数（频率、blocklist 或动态间隔）应集中修改 `profiling_loop` 的 builder 和间隔 API，同时更新 `cpuprofile_test.rs` / `migration_aster_unit_test.rs`，并核对 Go runtime 是否能表达相同参数。动态配置必须说明对当前 active 区间还是下一轮生效，不能在存在 guard 时启动第二个 sampler。

新增消费者策略时优先保持 `ProfilerCore::send` 非阻塞。若要记录丢包、自动清理断连 sender或实现背压，需要分别区分 Full 与 Disconnected，并评估持有消费者 mutex 时的工作量；不要在锁内执行阻塞发送、解析 protobuf、外部回调或 I/O。

若把真实 profiler 接入 `cmd/tidb-server`，应替换/绕过 `cmd/tidb-server/stubs.rs::cpuprofile`，显式引用 `astersql-util-cpuprofile`，并验证正常退出、信号退出和 keyspace 激活退出三条收尾路径都调用真实 `StopCPUProfiler`。这是主入口接线工作，不应通过修改本文件伪装为已经完成。

修改生命周期时应补齐异常路径。当前 worker panic 后 `started` 不自动复位，局部实例也没有 `Drop` 停止；若增加状态回报或 RAII，必须避免 worker 与 `stop().join()` 互相等待，并保持重复 Stop 与 Stop 后重启的 Go 契约。

修改 `ProfileData` 或错误类型会影响 `pprof_api.rs`、TopSQL collector、profile 工具和 server testkit 等多个 crate。字段或编码变化必须仍产出可由 `pprof::protos::Profile::decode` 读取的数据，并明确错误包是否仍要求空 `Data`。

测试逻辑必须继续放在独立文件：核心对照场景放 `pkg/util/cpuprofile/cpuprofile_test.rs`，迁移特有场景放 `pkg/util/cpuprofile/migration_aster_unit_test.rs`；同步参考 `cpuprofile_test.go`，不要在生产文件中加入 `#[cfg(test)]` 测试模块或为了测试删减 Go 版生命周期语义。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/util/cpuprofile` 返回 13 个 Rust/Go 文件，覆盖目标、装配、Go 对照和独立测试。
- RustCodeGraph `node --file pkg/util/cpuprofile/cpuprofile.rs --offset 1 --limit 400` 及尾部补读：读取目标全部 403 行，确认 36 个符号、公开性、无条件编译分支和完整控制流。
- RustCodeGraph `query StartCPUProfiler`、`query Register`、`query profiling_loop`、`query finish_profile`：确认 Rust/Go 同名符号及内部函数位置；对精确符号执行 `callers` / `callees` 未返回边，因此未据此声称无调用者，而是用精确 `rg` 补核真实引用。
- 已读生产与装配文件：`pkg/util/cpuprofile/{cpuprofile.rs,cpuprofile.go,pprof_api.rs,lib.rs,Cargo.toml}`、`pkg/util/topsql/collector/cpu.rs`、`cmd/tidb-server/{main.rs,stubs.rs}`、`pkg/lib.rs`；Cargo 依赖又核对了 server、TopSQL collector、TopSQL 和 profile crate 的 manifest。
- 已读独立测试：`pkg/util/cpuprofile/cpuprofile_test.rs`、`pkg/util/cpuprofile/cpuprofile_test.go`、`pkg/util/cpuprofile/migration_aster_unit_test.rs`。它们给出重复启停、None/重复/断连消费者、非阻塞丢包、采样器冲突、有效 pprof、最后消费者释放 guard 和串行测试要求的直接证据。
- 本任务只新增说明文档，依计划未运行 Cargo。交付前使用任务指定的结构命令确认文件存在且恰有 11 个固定二级标题，并人工复核“真实实现但 server 主入口仍命中桩”等迁移边界没有被写成已接线事实。
