# `br/pkg/restore/split/stubs.rs`

## 文件定位

`stubs.rs` 是 Cargo 包 `astersql-br-pkg-restore-split` 内的本地边界适配层。`br/pkg/restore/split/Cargo.toml` 将 `lib.rs` 指定为 crate root，`lib.rs` 再以 `pub mod stubs` 公开本模块，但没有把其符号通配重导出到 crate 根。因此同 crate 代码通过 `crate::stubs::{...}` 显式使用，外部调用者则需要经过 `astersql_br_pkg_restore_split::stubs` 路径。

该文件不是 PD/TiKV 生产客户端，而是为 Rust 移植期的 split 逻辑提供 darwin-safe 的 `context`、PD/metapb 数据形状、key codec、日志、重试与 option 替身。RustCodeGraph 索引将它识别为 642 行、91 个符号，并显示其类型被 `client.rs`、`split.rs`、`splitter.rs`、`sum_sorted.rs`及同目录独立测试直接使用。目录下没有 Go `stubs.go`；对照实现分散在 Go 标准库、PD/client-go protobuf/option 类型以及 `br/pkg/utils` 中。

## 核心职责

- 以 `Context`、`Canceled` 和 `DeadlineExceeded` 保留 split 链路可观察的取消、deadline 与父取消传播语义。
- 以 `KeyRange`、`RegionEpoch` 以及 `metapb`、`pdpb`、`pdhttp` 子模块提供 split client 需要的最小协议数据形状，避免本 crate 直接引入真实 kvproto/grpcio 边界。
- 把 `astersql-br-pkg-restore-utils` 的 codec/tablecodec 能力引入本 crate，并补充 RawKV 编码分支、row/common handle 组键等 split 专用辅助。
- 提供空日志、脱敏展示、字节边界比较和 `KeyNext`，使移植的 Go 风格算法能以相近的调用形状编译。
- 以 `BackoffStrategy`、`RetryState`、`WithRetry` 和 `WithRetryReturnLastErr` 驱动 split/scan 重试；通过 `HINT_SCAN_REGION_BACKOFF` 把 Go failpoint 转为测试可控的原子开关。
- 以 `CodecPDClient`、`Codec`、`GetRegionOption` 和 `GetStoreOption` 保留 client trait 的 codec-aware 与 PD option 接口形状，但不在此执行网络 I/O。

## 主要符号

- `Result<T> = Result<T, SharedError>` 是本 crate 一致的错误返回面；`Context` 用 `Arc<Mutex<Option<SharedError>>>` 存储本地取消原因，另持有可选父 context 和 `Instant` deadline。`Background`、`WithTimeout`、`WithCancel`、`Err`、`Done` 对应 Go `context` 最小观察面。
- `KeyRange` 表示 `[StartKey, EndKey)`，空 `EndKey` 表示正无穷。`metapb::{Peer, Region, Store, StoreLabel}`、`RegionEpoch` 仅保留本 crate 读取的字段；`Region::{GetStartKey, GetEndKey, GetId}` 保留 Go protobuf getter 形状。
- `pdpb::{ResponseHeader, Error, ErrorType, GetOperatorResponse, OperatorStatus, ScatterRegionResponse}` 表达 scatter/operator 状态机所需的最小 PD 响应。枚举只包含当前 Rust 调用面用到的值，不是完整 protobuf。`pdhttp::Rule` 同理只保留 placement rule 的组、ID 和键边界。
- `codec::EncodeBytesExt` 在 `is_raw_kv` 为真时直接追加原始字节，否则调用 restore-utils 的 memcomparable `EncodeBytes`。`tablecodec::{IntHandle, EncodeRowKeyWithHandle, EncodeCommonHandle}` 在通配再导出的 tablecodec 上补充 handle 编码。
- `CompareEndKey` 固定将空键视为正无穷；`CompareBytesExt` 由两个独立布尔参数决定哪一侧的空键是正无穷；其余情况使用 Rust 字节序。`KeyNext` 通过在末尾追加 `0x00` 得到 kv key 的紧邻后继边界。
- `BackoffStrategy` 只要求 `NextBackoff` 和 `RemainingAttempts`。`RetryState` 保存最大/已用次数及下一次/最大退避；`ExponentialBackoff` 先消耗一次尝试，再将下次时长倍增并截断到上限。`GiveUp` 耗尽次数，`ReduceRetry` 回退一次计数。
- `WithRetry` 在剩余次数为正时执行闭包，成功立即返回，失败时检查 context 并让 strategy 决定是否继续；`WithRetryReturnLastErr` 还会在首次执行前检查 context，并在耗尽时返回最后错误。
- `CodecPDClient::GetCodec` 返回无状态 `Codec`；当前 `DecodeRange`/`EncodeRegionRange` 都是字节原样往返。`WithAllowFollowerHandle` 仅构造 `allow_follower = true` 的 option，`GetStoreOption` 则是零大小占位类型。

## 执行流程

1. crate 根先装配 `stubs`；`client.rs`、`split.rs`、`splitter.rs` 和 `sum_sorted.rs` 在编译期选取所需符号，本文件自身没有全局初始化流程。
2. split client 的公开方法接收 `Context`、本地 `metapb`/`pdpb` 数据和 option；真正的 PD 操作被 `client.rs` 的 `PdBackend`/`PdHttpBackend` trait 隔离，此文件只提供参数与返回值形状。
3. 扫描路径使用 `WithRetry`：每轮调用闭包，成功结束；失败时保存最后错误，如 context 已取消则返回当轮业务错误，否则取得下一个 backoff。strategy 可通过 `GiveUp` 将剩余次数清零，使循环在本轮后结束。
4. client 的 split/scatter 路径使用 `WithRetryReturnLastErr`：先拒绝预先取消的 context，再循环执行、计算 backoff、二次检查取消，最终只暴露最后一个业务错误。
5. key 处理路径按 RawKV/TxnKV 选择 `EncodeBytesExt`；codec-aware client 通过 `CodecPDClient` 走 range decode/encode 接口。当前后者为恒等变换，所以只保留调用结构，不代表已实现 keyspace codec。
6. `sum_sorted.rs::Merge` 用 `CompareBytesExt(..., true, ..., true)` 判定右端开放区间；`client.rs::splitKeys` 用 `KeyNext` 把最后一个 split key 转成半开扫描上界；`split.rs` 则通过 follower option、hint 开关和重试类型执行分页 Region 扫描。

## 数据与状态

`Context` 的 clone 共享本地 `cancelled` 槽，但子 context 对父 context 保存的是创建时的 clone；由于 clone 内部仍指向父的同一 `Arc<Mutex<_>>`，后续父取消会被子的 `Err` 递归观察到。deadline 是子 context 自有的 `Instant`，到期时不写回 `cancelled`，而是每次 `Err` 调用即时判定。

`HINT_SCAN_REGION_BACKOFF` 是进程全局 `AtomicBool`，读写使用 `SeqCst`。它只提供 Go `hint-scan-region-backoff` failpoint 的布尔提示；具体如何缩短 backoff 由 `split.rs::WaitRegionOnlineBackoffer::NextBackoff` 消费。独立测试必须在结束时恢复为 `false`，否则会泄漏到其他并行测试。

`RetryState` 是可变的单调计数器，但 `ReduceRetry` 可将 `retry_times` 减一，用于“本轮有进展则不消耗重试”的上层策略。它不做下界防护；调用者必须只在已消耗计数的情况下回退，否则剩余次数会超过初始上限。所有 protobuf/option 替身均是内存值对象，不持有 socket、runtime handle 或外部资源。

## 依赖与调用关系

Cargo 层面，本文件直接使用 `astersql-errors::{New, SharedError}` 和 `astersql-br-pkg-restore-utils::stubs::{codec, tablecodec}`；所属 crate 另声明 `astersql-br-pkg-errors` 与 `hex`，但它们不是本文件的直接 import。标准库依赖为字节比较、格式化、`Arc`/`Mutex`/`AtomicBool` 和时间类型。

直接上游调用边已由 RustCodeGraph 的文件使用关系和精确文本引用交叉确认：

- `client.rs` 消费 context、PD 数据类型、codec client、`KeyNext`、`InitialRetryState` 和 `WithRetryReturnLastErr`，完成 split/scatter 客户端编排。
- `split.rs` 消费 `BackoffStrategy`、`RetryState`、`WithRetry`、follower option 和 hint 原子开关，完成 Region 扫描与一致性重试。
- `splitter.rs` 消费 `Context`、`Result` 和日志/codec 适配；`sum_sorted.rs` 消费 `KeyRange`、`CompareBytesExt` 和 range 展示。
- `region.rs` 使用 `metapb::Region` 和 `logutil::Region`；`mock_pd_client.rs` 使用 PD/metapb/codec/option 替身实现可控后端。

本文件的下游不是真实 PD/TiKV：协议操作最终交给 `client.rs` 的 trait 实现，codec/tablecodec 则下沉到 restore-utils 的本地实现。`CompareEndKey` 在当前 split crate 中未找到定义外的直接引用，应视为与 Go utils 对齐的兼容表面，而不能宣称它已在主链上执行。

## 错误处理与边界

- `Context::Err` 的优先级是本地显式取消、本地 deadline、父 context；因此子 deadline 已到时，即使父也已取消，可观察错误仍是 `context deadline exceeded`。`WithTimeout` 返回的 cancel 只在 `Err().is_none()` 时写入 `Canceled`；deadline 已到后再 cancel 不会改写错误。
- `WithCancel` 和 `cancel` 通过 `Mutex::lock().unwrap()` 写入错误；锁中毒会 panic，不会转成 `SharedError`。`Context` 也没有 Go `Done()` channel 的阻塞唤醒能力，其 `Done` 只是当下轮询布尔值。
- `WithRetry` 与 Go `utils.WithRetry` 不完全等价：Go 版聚合所有失败为 multierr，Rust 版只保留最后错误。如 context 在业务失败后取消，Rust 版返回当轮业务错误；`WithRetryReturnLastErr` 则只在进入循环前返回 context 本身错误。
- 两个重试函数为保持测试快速，对大于 1 ms 的计算 backoff 仅 sleep 1 µs，对不大于 1 ms 的值则不 sleep。因此这里不具备生产级限流时序，不能用测试耗时推断 Go 实际 backoff。零次尝试或 strategy 在调用闭包前已耗尽时，返回新建的 `retry exhausted`。
- `CompareEndKey`/`CompareBytesExt` 对空键的特殊顺序只适用于半开区间结束边界；将同一规则用于普通 start key 会产生错误排序。`KeyNext` 不做整数进位，其契约是 kv key 字节序下追加零字节。
- `log::{Info, Warn, Debug, Error}` 为空实现，会丢弃诊断信息；`Fatal` 固定 panic `log.Fatal`。`logutil::Region` 和 `logutil::Key` 返回固定占位字符串，只有 `redact::Key`/`StringifyRange` 会真正将键转成 hex。

## 并发与资源生命周期

`Context` 可通过 `Arc<Mutex<_>>` 在 clone 间共享取消状态，`WithCancel` 返回的 `Box<dyn FnOnce() + Send>` 可移到其他线程执行。它没有 condition variable、waker 或 channel；消费者只会在主动调用 `Err`/`Done` 时发现变化。deadline 也不启动计时线程，`Context` 丢弃后相关 `Arc`、错误和父引用自动释放。

`HINT_SCAN_REGION_BACKOFF` 是唯一进程全局可变状态；开关更新为顺序一致，但没有 RAII guard 自动恢复。`RetryState` 本身不含锁且通过 `&mut self` 串行更新，重试闭包也在调用线程同步执行。唯一的 sleep 是同步 `std::thread::sleep`；没有 async task、worker、后台定时器或持久连接。因此把本桩用于真实服务会同时缺少可唤醒取消、真实 backoff 和网络资源管理。

## 与 Go 版本的对应关系

Go 语义并非来自同路径单文件，而是以下组合：

- `Context`、`Canceled`、`DeadlineExceeded` 对应 Go 标准库 `context.Context`、`context.WithCancel`、`context.WithTimeout`。Rust `parity_test.rs::context_cancellation_matches_go` 覆盖显式取消、deadline 和父取消传播；但 Rust 没有 channel 和取消原因 API 的完整实现。
- `RetryState`/`InitialRetryState`/`ExponentialBackoff`/`GiveUp`/`ReduceRetry` 直接对应 `br/pkg/utils/backoff.go`；`WithRetry`/`WithRetryReturnLastErr` 对应 `br/pkg/utils/retry.go`。核心次数与最后错误契约被保留，但 Rust 的 `WithRetry` 未保留 Go multierr，两个 Rust 函数也故意不执行真实时长的 backoff。
- `CompareEndKey` 和 `CompareBytesExt` 按分支对应 `br/pkg/utils/key.go`；Rust `parity_test.rs::compare_bytes_ext_honors_each_empty_as_infinity_flag` 固定两侧 empty-as-infinity 的独立行为。`KeyRange`/`KeyNext` 对应 client-go `kv.KeyRange`/`kv.Key.Next`。
- `metapb`、`pdpb`、`pdhttp`、`GetRegionOption`/`GetStoreOption` 对应 kvproto 和 PD client 类型；Rust 只复制 split 当前需要的字段与枚举。`WithAllowFollowerHandle` 对应 `opt.WithAllowFollowerHandle()`，在 Go `split.go` 的首次 Region 扫描中使用。
- `codec`/`tablecodec`/`redact`/`logutil`/`log` 对应 TiDB 各包的跨模块调用面，而不是完整移植。尤其 `Codec` 是恒等替身，日志大多是空操作，PD 数据是内存结构；它们只能证明 split 算法与测试已接线，不能证明生产集群交互已等价。

Go `br/pkg/restore/split/split_test.go` 的 scan-region backoff failpoint 场景在 Rust `split_test.rs::{test_scan_region_back_offer_with_success, test_scan_region_back_offer_with_fail, test_scan_region_back_offer_with_stop_retry}` 中对应；`parity_test.rs::retry_return_last_err_checks_context_before_first_attempt` 另外锁定了 Go `WithRetryReturnLastErr` 的预取消契约。

## 扩展指南

- 新增 split 需要的 PD/metapb 字段或状态时，只扩展实际被 `client.rs`/`split.rs` 消费的最小表面，并同步所有 `PdBackend`、`SplitClient` mock 与 `client_test.rs`/`split_test.rs`；不要把手写结构误当成完整 protobuf 兼容层。
- 修改 context 时必须继续在独立 `parity_test.rs` 覆盖本地取消、父取消、deadline 优先级、首次执行前取消和 clone 传播；若引入阻塞等待，需明确 waker/channel 的释放与竞态契约。
- 修改重试时以 `br/pkg/utils/{retry,backoff}.go` 为语义基准，不得用更少次数、吞错或空实现换取测试通过。如将当前微秒级测试 sleep 改为生产 backoff，应先拆分时钟/sleeper 依赖，以确定性 fake clock 测试，避免实时长单元测试。
- 修改 key/codec 时同时审查 `client.rs::getEncodedKeys`、`split.rs::PaginateScanRegionWithCodecAware`、`sum_sorted.rs::Merge` 和 restore-utils 的 codec/tablecodec 实现；测试应放在 `client_test.rs`、`split_test.rs`、`sum_sorted_test.rs` 或 `parity_test.rs`，不内嵌到 `stubs.rs`。
- 如需真实 codec-aware PD 或 RPC，应在独立上游客户端依赖中移植、提交并打 tag，然后由 AsterSQL Cargo manifest 引用同一 tag；不应继续扩张本桩或将外部实现复制到本仓库。
- 修改 `HINT_SCAN_REGION_BACKOFF` 时优先引入作用域 guard 或测试局部状态，降低并行测试泄漏；并同步 `split_test.rs` 的成功、耗尽和非重试错误三类计数断言。

## 验证依据

- RustCodeGraph：`status` 显示当前索引含 7,032 个 Rust 文件；`files --filter br/pkg/restore/split` 列出本 crate 的 Rust/Go 源与独立测试；`node --file br/pkg/restore/split/stubs.rs --offset 1 --limit 500` 及 `--offset 500 --limit 250` 覆盖全部 642 行源码；`explore` 和文件使用关系确认本文件被 split 客户端、算法与测试消费。
- crate 与模块边界：`br/pkg/restore/split/Cargo.toml`、`br/pkg/restore/split/lib.rs`；该目录无 `doc.go`，Go package 对应由 Cargo 的 `package.metadata.porting.go-package` 和同路径 Go 文件确认。
- Rust 直接调用者：`br/pkg/restore/split/client.rs`、`split.rs`、`splitter.rs`、`sum_sorted.rs`、`region.rs`、`mock_pd_client.rs`；使用搜索核对了 `Context`、codec/PD 类型、`CompareBytesExt`、`KeyNext`、两个 retry 入口及 option 的精确引用点。
- Go 对照：`br/pkg/utils/retry.go` 的 `WithRetry`/`WithRetryReturnLastErr`，`br/pkg/utils/backoff.go` 的 `RetryState`，`br/pkg/utils/key.go` 的两个比较函数，以及 `br/pkg/restore/split/split.go` 中 follower scan、failpoint 和 backoffer 接线。
- Rust 独立测试：`br/pkg/restore/split/parity_test.rs` 直接覆盖 context、空键比较和预取消 retry；`split_test.rs` 直接覆盖 `WithRetry`、`EnableHintScanRegionBackoff` 及三类 scan backoff 结果；`client_test.rs`、`mock_pd_client_test.rs`、`region_test.rs`、`splitter_test.rs`、`sum_sorted_test.rs` 通过上层行为覆盖本桩的数据类型与辅助函数。Go 对照测试是 `br/pkg/restore/split/split_test.go`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付时只运行任务指定的 11 个章节结构检查，并人工复核“桩而非真客户端”、当前调用边与 Go/Rust 差异均有上述源文件或测试依据。
