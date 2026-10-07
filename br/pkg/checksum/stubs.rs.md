# `br/pkg/checksum/stubs.rs`

## 文件定位

`stubs.rs` 是 `astersql-br-pkg-checksum` crate 的本地依赖边界适配层。crate 根 `br/pkg/checksum/lib.rs` 以 `#[path = "stubs.rs"] pub mod stubs` 挂载它，并把其公共 API 扁平重导出；相邻的 `executor.rs` 才负责把表元数据展开为请求、发送 checksum 并聚合结果。`br/pkg/checksum/Cargo.toml` 没有声明依赖，且注释明确 arm64 路径不引入 kvproto、grpcio、domain、kv、distsql、metautil 等重依赖，因此本文件把这些 Go/Rust 外部边界所需的最小类型和行为集中到单文件中。

它是可编译的生产模块，但不是完整 TiKV 客户端，也没有真实网络、Region 路由、session 或 protobuf 生成代码。其当前用途是让 checksum 执行器及独立测试能在轻量 crate 中保留 Go 版本的请求形状、关键编码、取消、重试和流式响应契约。

## 核心职责

- 提供轻量基础设施：`Error`/`Result`、可传播父取消的 `Context`/`CancelFunc`，以及 `CIStr`、`TableInfo`、`IndexInfo`、`PartitionInfo`、`MetaTable` 等元数据子集。
- 表示 checksum 请求协议：`Request`、`Variables`、`ChecksumRequest`、`ChecksumResponse`、`ChecksumRewriteRule`，并手写当前所需的 protobuf wire 编解码。
- 复刻 checksum 所需键空间：`EncodeInt`、`GenTableRecordPrefix`、`EncodeTableIndexPrefix` 和三类全范围函数，再由 `RequestBuilder` 生成半开 `KeyRange`。
- 定义 `Client`/`Response` trait 与 `DistSQLChecksum` 薄适配，使 `executor.rs::sendChecksumRequest` 可以面向接口消费响应流。
- 提供 `ChecksumBackoffStrategy`、`WithRetry` 和线程局部测试钩子，支撑 `Executor::Execute` 的失败重试与取消路径。
- 提供 `GetPartitionByName`，使旧表分区按名称映射到 rewrite 所需的物理 ID。

## 主要符号

- 错误与取消：`Error { msg }`、`Error::Trace`、`Error::Annotate`、`Context::{Background, TODO, WithCancel, Err, Done}`、`CancelFunc::cancel`。`Trace` 当前原样返回，`Annotate` 仅拼接字符串，均是 `pingcap/errors`/Go context 的轻量替代。
- 元数据：`CIStr` 同时保存原文 `O` 和小写 `L`；`TableInfo` 仅保留 ID、名称、索引、分区和 `IsCommonHandle`；`PartitionInfo::GetPartitionIDByName` 未命中返回 0。`StatePublic = 5` 供 `executor.rs::buildRequest` 过滤索引。
- 请求模型：`Request` 保存类型、TS、wire 数据、缓存标志、并发、优先级、资源组、来源和键范围；`NewVariables` 复制 killed 占位值并令退避权重为 0。`ReqTypeChecksum = 105`，优先级常量对应 Go `kv.Priority*`。
- 协议模型：`ChecksumScanOn::{Table, Index}`、`ChecksumAlgorithm::Crc64_Xor`、`ChecksumRewriteRule`、`ChecksumRequest`、`ChecksumResponse`。三类消息各自实现 `Marshal`/`Unmarshal`；内部 `write_varint`、`read_varint`、`checked_length_end` 处理 wire 细节和边界。
- 键与范围：`EncodeInt` 将有符号整数异或 `SIGN_MASK` 后按大端编码；记录前缀为 `t + encoded(table_id) + _r`，索引前缀再带 `_i + encoded(index_id)`。`FullIntRange`、`FullNotNullRange`、`FullRange` 分别服务普通句柄、common handle 和索引。
- 构建与传输：`RequestBuilder::{SetHandleRanges, SetIndexRanges, SetStartTS, SetChecksumRequest, SetConcurrency, SetResourceGroupName, SetRequestSource, Build}`；`Client::Send` 返回可空 boxed `Response`，`Response` 暴露 `NextRaw` 与 `Close`；`DistSQLChecksum` 拒绝空响应。
- 重试：`BackoffStrategy`、`ChecksumBackoffStrategy`、`NewChecksumBackoffStrategy`、`WithRetry`。常量定义 8 次尝试、1 秒初始间隔和 30 秒文档最大值，但构造器实际采用 Go 通用策略默认的 10 秒封顶。
- 测试钩子：`set_skip_backoff_sleep` 控制线程局部 sleep；`inject_checksum_retry_err`/`take_checksum_retry_err` 实现一次性 failpoint。它们是公开 API，但调用证据位于测试和 `executor.rs::Execute`，不应当被当作线上故障注入框架。

## 执行流程

1. `executor.rs::buildTableRequest` 或 `buildIndexRequest` 先用本文件的前缀函数构造可选 rewrite rule，再选择 `FullIntRange`/`FullNotNullRange`/`FullRange`。
2. `RequestBuilder` 将范围的 Low/High 分别拼接记录或索引前缀；随后写入 TS、checksum wire 数据、并发、资源组和 `RequestSource`。`SetChecksumRequest` 同时设置 `Tp = ReqTypeChecksum` 与 `NotFillCache = true`，最终 `Build` 返回请求克隆。
3. `Executor::Execute` 为每个请求创建 `Variables`，调用本文件 `WithRetry`。每次尝试进入 `sendChecksumRequest`，后者经 `DistSQLChecksum -> Client::Send` 获取 `Response`。
4. `sendChecksumRequest` 重复调用 `Response::NextRaw`，用 `ChecksumResponse::Unmarshal` 解码每个分片并在执行器中 XOR/累加；结束或出错时调用 `Close`。本文件只定义流契约，不提供生产 client/response 实现。
5. 尝试失败时 `WithRetry` 收集错误；若上下文已取消则立即合并返回，否则由 `ChecksumBackoffStrategy::NextBackoff` 先倍增等待、扣减剩余次数，再 sleep。成功则立即退出，耗尽后以 `"; "` 合并所有错误消息。
6. 构造分区 rewrite 时，`executor.rs::buildChecksumRequest` 调用 `GetPartitionByName`，通过 `CIStr.L` 找到旧表同名分区 ID；非分区表和名称未命中走不同错误分支。

## 数据与状态

绝大多数协议与元数据对象是按值拥有的 `String`/`Vec`，请求构建完成后不共享可变数据。`RequestBuilder` 唯一内部状态是正在组装的 `Request` 和延迟错误 `err`；当前 `ChecksumRequest::Marshal` 没有业务失败分支，但 builder 仍保留“setter 记录、Build 一次返回”的外部契约，且 `Build` 会 `take` 错误。

`Context` 的本地取消原因存于 `Arc<Mutex<Option<Error>>>`，子上下文另持 `Arc<Context>` 父链，所以父对象在派生后才取消也能被 `Err` 递归观察。`CancelFunc` 共享子上下文的 mutex，并消费自身写入固定错误。两个故障钩子使用 `thread_local! Cell<bool>`：测试线程之间隔离，`CHECKSUM_RETRY_ERR` 由 `replace(false)` 一次性消费；这与进程级全局 failpoint 并不等价。

`ChecksumBackoffStrategy` 保存剩余次数、当前延迟和最大延迟。初始 1 秒在第一次失败后先翻倍为 2 秒，随后为 4、8、10 秒封顶；每次 `NextBackoff` 扣一次计数。`KeyRanges::FirstPartitionRange` 当前直接返回全部范围，而非完整 Go 分区范围容器语义。

## 依赖与调用关系

本文件只依赖标准库的 `Cell`、`Arc<Mutex<_>>`、线程与时间。crate 装配链是 `lib.rs -> stubs.rs + executor.rs`，并由 `lib.rs` 统一重导出。RustCodeGraph 对精确符号的结果确认：`DistSQLChecksum` 调用 `Client::Send`；`WithRetry` 调用 `BackoffStrategy::{RemainingAttempts, NextBackoff}`、`Context::Done` 与 `join_errors`；`GetPartitionByName` 调用 `PartitionInfo::GetPartitionIDByName`。

直接上游主要是 `executor.rs`：请求构建路径使用元数据、编码、范围和 `RequestBuilder`；发送路径使用 `Client`、`Response`、wire response 和变量；执行路径使用重试、取消及一次性错误注入。`parity_test.rs` 与 `executor_test.rs` 提供内存 `Client`/`Response` 实现，覆盖真实上游接口。没有证据表明此 crate 当前接入生产 TiKV client；`Cargo.toml` 的空依赖表反而确认它仍是独立的本地 stand-in 边界。

## 错误处理与边界

- protobuf 解码会拒绝截断 varint、超过 64 位的 varint、长度转 `usize`/加法溢出、越过输入末尾，以及未支持的 wire type；未知的 varint 与 length-delimited 字段会跳过。枚举值则采取收敛策略：未知 scan 值回落 `Table`，algorithm 始终解析为唯一支持的 `Crc64_Xor`。
- `DistSQLChecksum` 原样传播 `Client::Send` 错误，并把 `Ok(None)` 转为 `client returns nil response`；它不负责 Close，资源关闭由 `sendChecksumRequest` 处理。
- `WithRetry` 只在某次尝试失败后检查取消，等待阶段使用阻塞 `thread::sleep`，不会在 sleep 中被取消唤醒。错误通过文本拼接聚合，不保留 Go multi-error 的类型或错误链。
- `Context` 的 mutex 使用 `unwrap`；若持锁线程 panic 导致 poisoned mutex，后续访问也会 panic。父链只由 `WithCancel` 建立，没有 deadline/value 等完整 context 能力。
- `GetPartitionByName` 把 `part_id > 0` 视为成功，因此 ID 0 等同未找到；无分区错误刻意保留 Go 历史拼写 `parition`。
- `FullIntRange` 忽略 `_is_unsigned`，`SetHandleRanges` 忽略 dctx 与 `_is_common_handle`；调用方必须先选好正确范围。`FirstPartitionRange` 的退化实现、无真实路由和无 RPC 均是当前边界，不能据此宣称完整 distsql 行为已支持。

## 并发与资源生命周期

`Client: Send + Sync` 允许执行器通过共享引用发送，`Response: Send` 允许响应对象跨线程所有权移动；本文件本身没有启动 worker 或异步任务。请求仍由 `Executor::Execute` 顺序处理，`Concurrency` 只是透传字段，不会在此处创建并发扫描。

取消状态通过 `Arc<Mutex<_>>` 在线程间共享，父上下文由 `Arc` 保活到所有子上下文释放。响应资源的生命周期是 `Client::Send` 产生 boxed response，`sendChecksumRequest` 消费流并无论成功、读取失败或解码失败都尝试 `Close`；测试 `close_error_overrides_response_read_error` 证明 Close 错误按 Go named-return defer 语义覆盖先前读取错误。测试钩子为线程局部，测试必须恢复 `SKIP_BACKOFF_SLEEP`，而 consume-once 注入无需显式清除已消费状态。

## 与 Go 版本的对应关系

本文件没有同路径 `stubs.go`；它聚合了 Go `executor.go` 依赖的多个包边界。`br/pkg/checksum/executor.go` 是主对照：Go 使用真实 `model.TableInfo`、`tipb.Checksum*`、`kv.Request/Client/Variables`、`distsql.RequestBuilder/Checksum`、`ranger`、`tablecodec` 和 `utils.WithRetry`，Rust 则由本文件提供相同用途的窄接口。

已核对的关键对应包括：表/索引 scan 与 `Crc64_Xor` 字段、低优先级与 `NotFillCache` 请求、common-handle/整数句柄/索引范围、表和索引 rewrite 前缀、8 次 checksum 重试、10 秒实际封顶，以及 `br/pkg/utils/misc.go::GetPartitionByName` 的两条错误文案。Rust `ChecksumRequest`/`Response`/`RewriteRule` 手写的字段号分别对应 tipb wire；这只覆盖当前用到的字段，并非生成类型的完整替代。

语义差异必须保留可见：Rust context 没有 deadline/value，`Trace` 不增加栈，错误聚合只有字符串，sleep 不可取消，范围 builder 没有真实分区与 unsigned 分支，client 只有 trait 而无 TiKV 实现。Go `executor_test.go` 使用 mock cluster 与真实 storage client；Rust `executor_test.rs`/`parity_test.rs` 使用内存实现验证协议和控制流，因此不能把测试成功外推成真实集群互操作已验证。

## 扩展指南

- 新增 tipb 字段时，应同时修改对应结构的 `Marshal`/`Unmarshal`、未知字段策略和截断/溢出测试；先核对真实 proto 字段号与 wire type，避免仅按字段名猜测。测试应放在独立的 `parity_test.rs` 或 `executor_test.rs`，不要内嵌到 `stubs.rs`。
- 扩展键范围或 unsigned handle 时，应修改 `FullIntRange`/`RequestBuilder` 并与 Go `ranger`、`distsql.RequestBuilder`、`tablecodec` 的字节边界逐项对照；错误会直接造成漏扫或错误 checksum，兼容风险高。
- 接入真实 client 时，应在独立上游 crate 提供并发布带 tag 的依赖，再让此 crate 统一引用；不要把 client 复制到 vendor/third_party，也不要用本地 `[patch]`。需要明确 `Response::Close`、取消、Region 重试和变量语义后再替换 stand-in。
- 改动重试时，必须说明次数含义、首个等待值、最大值与取消时机，并同步测试一次性 failpoint、耗尽错误和父取消；阻塞 sleep 若改为可取消等待，会改变当前生命周期语义。
- 扩充 `Context` 或元数据模型时，优先复用仓库已有 canonical crate，避免本地副本继续漂移；若仍需保留适配层，应明确字段转换和不支持项。
- 性能敏感点是 wire 分配、每段范围前缀克隆、响应流分片和错误向量积累；优化不能改变稳定字节编码、半开范围或错误覆盖次序。

## 验证依据

- 源码与 crate：`br/pkg/checksum/stubs.rs`（完整 954 行）、`br/pkg/checksum/lib.rs`、`br/pkg/checksum/Cargo.toml`、直接调用方 `br/pkg/checksum/executor.rs`。
- Go 对照：`br/pkg/checksum/executor.go`，以及分区查询的真实来源 `br/pkg/utils/misc.go::GetPartitionByName`。
- 独立 Rust 测试：`br/pkg/checksum/parity_test.rs` 覆盖整数/common-handle/索引范围、Close 错误优先级、精确 `CIStr` 索引匹配、父取消传播、截断 length-delimited 字段、请求常量和公开契约；`br/pkg/checksum/executor_test.rs` 覆盖取消、请求数量、rewrite、聚合、common handle 与 failpoint；`br/pkg/checksum/executor_nokit_test.rs` 覆盖 `RequestSource` setter。Go 对照测试为 `executor_test.go`、`executor_nokit_test.go` 和 `main_test.go`。
- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter br/pkg/checksum` 确认 crate 文件集合；`node DistSQLChecksum`、`node WithRetry`、`node GetPartitionByName` 核对源码与直接调用边。`explore` 对常见符号产生大量跨仓库同名结果，因此调用方又以 `executor.rs` 的显式 import/调用和独立测试引用补证。
- 本任务为纯文档分析，未运行 Cargo。交付结构检查要求目标文件存在且固定二级标题恰好 11 个；人工复核重点是 stand-in 限制、真实调用链、Go 差异和扩展风险均未被写成未经验证的完整能力。
