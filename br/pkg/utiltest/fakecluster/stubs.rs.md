# `br/pkg/utiltest/fakecluster/stubs.rs`

源文件：[`stubs.rs`](./stubs.rs)

## 文件定位

本文件是 `astersql-br-pkg-utiltest-fakecluster` crate 的本地边界替身层。crate 入口 `br/pkg/utiltest/fakecluster/lib.rs` 通过 `#[path = "stubs.rs"]` 声明该模块，并把其中的协议结构、上下文、错误、`codec` 与 `oracle` API 扁平再导出；同 crate 的 `core.rs` 则使用这些类型实现内存假 TiKV/PD 集群。

它不是线上 RPC 或持久化实现。文件级说明明确其目的是在不引入 `kv/domain/kvproto/grpcio` 的条件下，为 macOS arm64 可用的 BR 测试夹具提供最小兼容边界。`Cargo.toml` 也只声明 `streamhelper`、`streamhelper-spans`、`rand` 和 `tracing`，没有 protobuf、gRPC 或 TiKV client 依赖。

## 核心职责

1. 用普通 Rust 数据结构表达 `kv.KeyRange`、`errorpb.Error`、日志备份请求/响应、flush 事件和 `txnlock.Lock` 的测试所需字段。
2. 用 `Code`、`StatusError` 和 crate 自有 `Error` 保留 `Canceled`、`Unimplemented` 两类 gRPC 风格错误及其文本。
3. 用 `Context`/`CancelHandle` 提供可克隆、可跨线程等待、一次记录首个取消原因的取消机制，供订阅清理和流接收逻辑使用。
4. 在 `oracle` 中提供 TSO 物理部分的拼装、提取和 `SystemTime` 转换，在 `codec` 中提供 TiDB memcomparable `EncodeBytes` 的局部实现。

这些职责都服务于测试夹具：类型不实现 protobuf 编解码，`Context` 没有 deadline/value/父子传播，错误码也不是完整 gRPC code 集合。

## 主要符号

- `KeyRange { StartKey, EndKey }`：半开区间 `[StartKey, EndKey)`；约定空 `EndKey` 表示正无穷。字段使用 `Vec<u8>`，实例拥有键字节。
- `ErrorPb { Message }`：仅保留 Go `errorpb.Error` 在 fakecluster 分支中实际断言的消息字段。
- `RegionIdentity`、`RegionCheckpoint`、`GetLastFlushTSOfRegionRequest`、`GetLastFlushTSOfRegionResponse`：构成 region checkpoint 批量查询的数据模型；`RegionCheckpoint.Err` 和 `Region` 都是可缺省值。
- `FlushEvent`、`SubscribeFlushEventRequest`、`SubscribeFlushEventResponse`：表达订阅请求和按批返回的键范围/checkpoint 事件。请求是零字段占位类型。
- `FlushNowRequest`、`FlushResult`、`FlushNowResponse`：表达立即 flush 的占位请求及逐任务结果。
- `Lock`：保存锁键、主锁键、事务 ID 与 TTL，仅用于 fakecluster 注入和快照测试，不含真实锁解析行为。
- `Code::{Canceled, Unimplemented}`、`StatusError`、`status_error`：最小状态码集合、带码错误及快捷构造器；`Display` 格式为 `Debug(code): message`。
- `Context`、私有 `ContextInner`、`CancelHandle`：`Arc` 共享取消状态；`Context::background`、`with_cancel`、`is_done`、`err_message`、`wait_cancelled` 分别负责构造、观察和阻塞等待，`CancelHandle::cancel`/`cancel_with` 负责触发取消。
- `oracle::PHYSICAL_SHIFT_BITS`、`ComposeTS`、`ExtractPhysical`、`GetTimeFromTS`、`GoTimeToTS`、`add_duration`：实现 18 位逻辑部分的 TSO 辅助操作。
- `codec::EncodeBytes`：按 8 字节分组、零填充并追加 `0xFF - pad_count` marker；即使输入长度恰好是 8 的倍数，也会追加终止组。
- `Result<T>`、`Error`：fakecluster 的统一结果与错误类型；`Error::from_status` 和 `From<StatusError>` 同时保存可检查的结构化状态和显示文本。

## 执行流程

取消流程从 `Context::with_cancel` 开始：先用 `background` 建立 `Arc<ContextInner>`，再返回共享同一内部状态的上下文和句柄。`cancel_with` 先在 `err` 的互斥区内写入首个错误文本，再以 `SeqCst` 将 `cancelled` 置为真，最后 `notify_all` 唤醒所有 `wait_cancelled` 调用者；重复取消不会覆盖首个文本，但仍会再次通知。`core.rs::Store::SubscribeFlushEvent` 会克隆上下文并启动清理线程，线程在 `wait_cancelled` 返回后删除订阅者；`trivialFlushStream::Recv` 则用 `is_done`/`err_message` 将取消转换为 `Code::Canceled`。

TSO 流程以 `ComposeTS(physical, logical)` 将毫秒物理值左移 18 位并加入逻辑值，`ExtractPhysical` 反向右移。`GetTimeFromTS` 把物理毫秒拆成秒和纳秒后换算为 `SystemTime`；`GoTimeToTS` 计算相对 Unix epoch 的整数毫秒并把逻辑位清零。`core.rs::AllocTSO`、`AdvanceCheckpointBy` 和 `AdvanceClusterTimeBy` 使用这些函数推进假集群时钟，`utiltest/crr/pd_sim.rs::NewPDSimWithTestContext` 也使用 `ComposeTS` 构造任务起始时间。

键编码流程由 `EncodeBytes` 预留 `(len / 8 + 1) * 9` 字节，然后逐组追加 8 字节数据（末组不足时补零）和 marker。`core.rs::FlushExcept` 与 `ApplyCheckpointToStore` 用它编码 flush 事件边界；图索引还显示 `br/pkg/utils/stubs.rs::EncodeMetaKey` 和 `br/pkg/utils/key.rs::EncodeTxnMetaKey` 调用该符号。

## 数据与状态

协议替身结构均为值类型，常用结构实现 `Clone` 和 `Default`；这使请求、响应与事件能在内存通道和测试断言之间复制，但不代表它们具备 protobuf wire compatibility。`RegionCheckpoint` 与响应类型没有实现 `PartialEq`，调用方通常逐字段检查。

`ContextInner` 是本文件唯一长期共享的可变状态：`AtomicBool` 表示完成信号，`Mutex<Option<String>>` 保存取消原因，独立的 `Mutex<()> + Condvar` 承担阻塞/唤醒。所有 `Context` 与 `CancelHandle` clone 都共享同一个 `Arc`，对象会在最后一个 clone 释放后销毁。

`StatusError` 在转换为 `Error` 时被保存在 `Error.status` 中，同时 `msg` 固化为显示字符串；普通 `Error::new` 的 `status` 为 `None`。因此调用方既能匹配 `Code`，也能沿用 Go 测试中的错误文案断言。

## 依赖与调用关系

上游入口是 `br/pkg/utiltest/fakecluster/lib.rs`，它再导出本文件的全部公开边界。主要直接消费者是 `br/pkg/utiltest/fakecluster/core.rs`：它导入请求/响应、事件、锁、上下文、错误、`codec`、`oracle` 和 `status_error`，据此实现 checkpoint RPC、flush 订阅、TSO 与 region 状态。`br/pkg/utiltest/crr/pd_sim.rs` 和 `pd_sim_service.rs` 通过 crate 公共 API 使用 `Context`、协议结构和 oracle，形成更上层的 PD/日志备份测试适配。

RustCodeGraph 的精确调用结果包括：`status_error` 被 `core.rs::GetLastFlushTSOfRegion`、`SubscribeFlushEvent` 和 `trivialFlushStream::Recv` 调用；`ComposeTS` 被 `core.rs::AllocTSO` 和 `crr/pd_sim.rs::NewPDSimWithTestContext` 调用；`GetTimeFromTS`、`GoTimeToTS` 与 `add_duration` 被两个集群时钟推进方法调用；`EncodeBytes` 被 `core.rs` 的 flush/checkpoint 事件生成路径调用。

下游只依赖 Rust 标准库：原子变量、`Arc`、`Mutex`、`Condvar`、`Duration` 和 `SystemTime`。本文件自身不依赖 `Cargo.toml` 中的第三方 crate；第三方和相邻 BR crate 是 `core.rs` 的依赖。

## 错误处理与边界

`StatusError` 只覆盖当前 fakecluster 所需的取消和未实现分支。`From<StatusError> for Error` 保留 code；若新增 RPC 错误类别，只拼接文本而不扩展 `Code` 会使结构化断言失真。

`Context` 的所有 `Mutex::lock` 与 `Condvar::wait` 都直接 `unwrap`，所以持锁线程 panic 导致 poison 时，观察或等待方也会 panic；这是测试桩的边界，不应当被描述成生产级恢复策略。`wait_cancelled` 用循环重新检查原子值，可以抵御虚假唤醒。取消没有超时、deadline、父 context 或自动取消能力。

oracle 函数按 TiKV TSO 的非负物理毫秒使用场景设计。`ComposeTS` 对传入负数或溢出值使用 Rust `as` 转换和 wrapping addition，`GoTimeToTS` 对 Unix epoch 之前的时间也会把负毫秒转换成 `u64`；这些结果不应当视作受支持的生产时间语义。逻辑值没有显式限制为 18 位，调用方必须维持该不变量。

`EncodeBytes` 只编码、不解码，也不验证已有前缀 `b`。其循环条件 `idx <= d_len` 是协议必要条件：空输入输出 8 个零加 `0xF7`，整组输入仍附加终止组。更改该条件会破坏字节序与 Go/TiDB codec 对齐。

## 并发与资源生命周期

`Context` 的 clone 是轻量 `Arc` clone；取消是跨线程广播事件。`cancel_with` 通过互斥锁保证只有第一个原因被保存，通过 `SeqCst` 原子和条件变量保证等待者最终可见完成状态。`notify_all` 允许多个订阅清理线程或其他等待者同时退出。

本文件不创建线程、通道或网络连接；线程生命周期由消费者控制。当前直接证据是 `core.rs::SubscribeFlushEvent` 为每个订阅创建一个线程，该线程持有 `Context` 和 `Store`，直到取消后移除 sender 并退出。如果调用方永久使用 `Context::background` 且不取消，该清理线程也会持续等待；因此可取消订阅必须保留并调用 `CancelHandle`。

协议结构和 `Error` clone 都会复制拥有的 `String`/`Vec`；`Context`/`CancelHandle` clone 则共享状态。文件没有异步 runtime、锁顺序协议或网络背压逻辑，不能把这些替身推断为真实 gRPC 的资源保证。

## 与 Go 版本的对应关系

Go 同目录只有 `core.go`，没有一一对应的 `stubs.go`。Go 文件直接导入标准库 `context`，kvproto 的 `errorpb`/`logbackuppb`，gRPC `codes/status`，TiKV client-go `oracle`/`txnlock`，以及 TiDB `kv`/`codec`；Rust 为避免这些重依赖，把 `core.go` 实际用到的子集集中在本文件。

字段对应关系保持 Go 命名和数据流，例如 `KeyRange`、`RegionIdentity`、`RegionCheckpoint`、`FlushEvent` 与 `Lock`。Rust 用 `Option<T>` 表达 Go protobuf 指针可空性，用 owned `Vec`/`String` 表达 Go slice/string，用 `StatusError` 加 `Error.status` 替代 gRPC status error 的分类能力。

`Context` 对齐 `ctx.Done()`、`ctx.Err()` 和 `CancelFunc` 的当前用法，而不是完整复制 Go context API。`core.go::SubscribeFlushEvent` 等待 `<-ctx.Done()` 后删除订阅；Rust 对应路径等待 `wait_cancelled`。Go `trivialFlushStream.Recv` 在取消时会优先尝试取走已到达事件，Rust `core.rs` 也先 `try_recv` 再返回 `Canceled`。

`codec::EncodeBytes` 与 `pkg/util/codec/bytes.go::EncodeBytes` 的 8 字节分组、零填充、marker 和终止组一致，且同仓库 canonical Rust 实现位于 `pkg/util/codec/bytes.rs`。oracle 的 18 位物理移位规则对齐 client-go 常用契约，但本地实现只覆盖 fakecluster 用到的转换函数。

## 扩展指南

新增 fakecluster RPC 时，先确认 `core.go` 实际访问的 protobuf 字段，仅在本文件增加必要请求/响应字段，再在 `lib.rs` 明确再导出，并把行为放在 `core.rs`；不要在数据桩中偷偷加入网络或持久化语义。相关测试应继续放在独立的 `parity_test.rs`、`core_test.rs` 或调用方 crate 的 `*_test.rs`，不要内嵌到 `stubs.rs`。

扩展取消语义时，应同步检查 `ContextInner`、`Context`、`CancelHandle`、`core.rs::SubscribeFlushEvent` 和 `trivialFlushStream::Recv`。增加 deadline 或父子传播会改变线程退出与首错语义，需要独立覆盖重复取消、多个等待者、事件与取消竞争、订阅者清理；不能仅增加字段而不验证生命周期。

扩展错误码时，应同时更新 `Code`、构造/转换路径以及所有匹配 `Error.status` 的测试。扩展 oracle 时必须明确物理/逻辑位宽、溢出与 epoch 前时间策略。扩展 codec 时优先复用 canonical `pkg/util/codec` crate；若精简 crate 边界仍要求本地实现，应逐字节对齐 Go 黄金样例并警惕重复实现漂移。

兼容性风险集中在公开字段名、错误文本和编码字节；性能风险主要是 `Context` 的顺序一致原子、每订阅一线程，以及事件 clone 的内存复制。当前是测试工具，除非真实测试规模证明需要，不应牺牲确定性去做复杂优化。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、7,032 个 Rust 文件；目标 `stubs.rs` 已索引，共 386 行、41 个符号。
- 已读取源码：`br/pkg/utiltest/fakecluster/stubs.rs`、`lib.rs`、`core.rs`；已读取 crate 声明：`br/pkg/utiltest/fakecluster/Cargo.toml`。
- 已核对 Go 与 canonical codec：`br/pkg/utiltest/fakecluster/core.go`、`pkg/util/codec/bytes.go`、`pkg/util/codec/bytes.rs`。Go 入口的 import 列表证明本文件替代的是外部 context/kvproto/gRPC/oracle/txnlock 与仓库 codec 边界。
- 已读取独立 Rust 测试：`br/pkg/utiltest/fakecluster/parity_test.rs` 和 `core_test.rs`。其中 parity 测试覆盖空键编码黄金值、TSO 推进、`Unimplemented`/取消状态、订阅取消清理和 flush 事件；`core_test.rs` 覆盖与 Go 一致的缺失对象 panic 边界。
- RustCodeGraph 调用证据：`status_error -> core.rs::{GetLastFlushTSOfRegion, SubscribeFlushEvent, Recv}`，`oracle::ComposeTS -> {core.rs::AllocTSO, crr/pd_sim.rs::NewPDSimWithTestContext}`，时间转换函数进入 checkpoint/cluster time 推进路径，`codec::EncodeBytes` 进入 flush/checkpoint 事件编码路径。
- 人工边界复核：确认本文件是测试替身而非生产协议实现；确认公开数据结构无 wire serialization；确认 `Context` 首错保留、广播唤醒、永久 background 等待，以及 oracle 的非负 TSO 前提。
