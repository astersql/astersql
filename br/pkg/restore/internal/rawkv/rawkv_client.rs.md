# `br/pkg/restore/internal/rawkv/rawkv_client.rs`

## 文件定位

本文件是 Cargo library crate `astersql-br-pkg-restore-internal-rawkv` 的主体实现。包入口 `br/pkg/restore/internal/rawkv/lib.rs` 通过 `#[path = "rawkv_client.rs"]` 挂载模块并执行 `pub use rawkv_client::*`，所以 `RawkvClient`、`PdRawkvDialer`、`RawKVBatchClient` 及构造函数都从 crate 根扁平导出。`br/pkg/restore/internal/rawkv/Cargo.toml` 把它映射到 Go 包 `br/pkg/restore/internal/rawkv`，并将 porting lane 标为 2。

它描述 BR 恢复场景中的非事务 RawKV 批量写入边界：上层提供带时间戳后缀的键值，客户端在一个批次内按去掉时间戳的逻辑键去重，再把保留的原始键值交给 `RawkvClient::BatchPut`。这对应 Go 恢复链中 `br/pkg/restore/log_client/client.go` 使用 rawkv 包写回元数据的能力。

当前 Rust crate 的 `[dependencies]` 为空，未链接 `tikv/client-rust`、kvproto、gRPC 或 Prometheus。`NewRawkvClient` 使用的默认 `UnconfiguredPdDialer` 必然返回错误；RustCodeGraph 也未发现目标实现被生产 Rust 主链调用，已确认的语义调用者集中在本 crate 的独立测试。因而本文件目前是可注入的本地移植边界和批处理实现，不代表真实 PD/TiKV 网络链路已经接通。

## 核心职责

1. 用 `RawkvClient` trait 固定恢复侧需要的 Get、Put、BatchGet、BatchPut 和 Close 接口形状。
2. 用 `PdRawkvDialer` 隔离 PD 地址、安全配置及客户端创建，并确保构造时固定传递 10 秒自定义超时。
3. 用 `RawKVBatchClient` 缓冲写入，达到逻辑键容量后批量提交，结束时由 `PutRest` 刷出不足一批的残余数据。
4. 在单个批次内以 `TruncateTS(key)` 为键去重，只保留 `originTs` 更大的原始 key/value，避免 resolved-ts 开启时同一批出现重复逻辑键。
5. 为每次实际 `BatchPut` 记录列族、批大小和耗时，并在成功后重置缓冲；失败时保留缓冲并传播错误。
6. 提供轻量 `Context`、`Security`、`RawOption`、`Error` 和 metrics 替身，使行为可在没有真实网络依赖的环境中测试。

## 主要符号

- `RAWKV_CUSTOM_TIMEOUT: Duration`：固定为 10 秒，由 `NewRawkvClientWithDialer` 传给 `PdRawkvDialer::NewClient`，对应 Go 的 `opt.WithCustomTimeoutOption(10*time.Second)`。
- `Result<T>` 与 `Error`：本地结果和字符串错误类型。`Error::Trace` 是恒等函数，`Error::Errorf` 仅构造消息，保留 Go `pingcap/errors` 的调用形状但没有堆栈或错误链。
- `Context`：以 `Arc<Mutex<Option<Error>>>` 保存取消原因；克隆值共享状态。`Background` 与 `TODO` 都创建同样的空状态，`cancel` 写入错误，`Err` 返回克隆。
- `Security`：保存 CA、证书和私钥字符串，只作为拨号参数透传；本文件不解析或验证 TLS 材料。
- `RawOption` 与 `SetColumnFamily`：本地 RawKV 选项的最小表示，目前只有可选列族字段。
- `RawkvClient`：公共传输 trait，要求实现 `Send + Sync`，是批量客户端的直接下游。
- `PdRawkvDialer`：公共客户端工厂 trait；`UnconfiguredPdDialer` 是默认私有实现，总是返回“未配置”错误。
- `NewRawkvClient`：使用默认拨号器的公共构造入口，当前必然失败。
- `NewRawkvClientWithDialer`：可用的注入入口，把 context、PD 地址、安全配置和固定 10 秒超时传给外部 dialer。
- `KVPair`：私有缓冲条目，保存用于版本选择的 `ts`，以及提交时使用的完整 `key`、`value`。
- `RawKVBatchClient`：批处理状态机，持有列族 `cf`、容量 `cap`、不同逻辑键计数 `size`、去重表 `kvs` 和共享底层客户端 `rawkvClient`。
- `NewRawKVBatchClient`：以空列族、空 map 和零计数创建批处理客户端。
- `RawKVBatchClient::{Close, SetColumnFamily, Put, PutRest}`：分别负责尽力关闭、更新目标列族、缓冲/满批提交和残余提交。
- `TruncateTS`：私有辅助函数，长度至少 8 字节时移除末尾 8 字节；空键返回空向量，短键保持原样。
- `metrics::{Observation, RawKVBatchPutBatchSize_Observe, RawKVBatchPutDurationSeconds_Observe}`：进程内全局观测桩；`take_*` 和 `clear_observations` 是测试辅助接口。

## 执行流程

客户端创建分两条路径。`NewRawkvClient` 取得 `default_pd_dialer()` 后调用 `NewRawkvClientWithDialer`；后者只做参数转发，始终把 `RAWKV_CUSTOM_TIMEOUT` 作为 timeout。默认拨号器不尝试联网而是立即报错；测试通过实现 `PdRawkvDialer` 注入 `Arc<dyn RawkvClient>`。

`RawKVBatchClient::Put` 的主流程如下：

1. 调用 `TruncateTS(key)` 生成逻辑键。正常 MVCC 编码键由“业务键 + 8 字节时间戳”组成，因此不同版本映射到同一个 map key。
2. 若逻辑键首次出现，则保存完整 key/value 与 `originTs`，并将 `size` 加一；若已经存在，只有新 `originTs` 严格大于旧值时才覆盖，较小或相等的版本不改变缓冲和计数。
3. 当 `size >= cap` 时，从 `HashMap` 的全部值构建 keys/values 数组，并附带当前 `cf` 的 `SetColumnFamily` 选项调用 `BatchPut`。数组顺序由 `HashMap` 迭代决定，不保证稳定排序，但同一次迭代中 key 与 value 的位置保持配对。
4. 无论 `BatchPut` 成功或失败，都记录实际 map 长度和调用耗时。失败经 `Error::Trace` 返回，缓冲保持不变；成功后调用 `reset` 清空 map 并把 `size` 归零。

`PutRest` 仅在 `size > 0` 时执行与满批路径相同的组批、提交、指标记录和成功重置；空缓冲直接返回成功。调用方因此必须在输入结束后显式调用 `PutRest`，否则最后不足容量的一批仍留在内存中。`Close` 不会隐式刷新，只调用底层 `Close` 并忽略其结果。

## 数据与状态

`size` 表示当前 map 中不同逻辑键的数量，而不是累计 `Put` 次数；在正常流程中它与 `kvs.len()` 相等。覆盖同一逻辑键不会增加 `size`，所以只有不同逻辑键达到 `cap` 才触发满批。成功提交把两者同时清零，提交失败则同时保留，便于调用方再次调用 `Put` 或 `PutRest` 重试；本文件没有退避或自动重试。

map 的值保留原始带时间戳 key，而 map 的键仅用于批内判重。版本选择依据显式 `originTs`，并不解码 key 尾部的 8 字节；调用方必须保证两者语义一致。长度不足 8 字节的 key 被整体视作逻辑键，空 key 也允许进入缓冲。跨批没有去重状态：一个逻辑键在前一批成功 reset 后再次出现，会作为新条目提交，所以跨批可保留多个版本。

`cf` 初始为空字符串；若调用方没有先执行 `SetColumnFamily`，空列族仍会包装进 `RawOption` 并下传，本层不校验。`cap` 是 `i32` 且没有正数校验：零或负数会使第一次 `Put` 后的 `size >= cap` 成立，实际表现为每次 Put 都尝试刷新。

metrics 的两个 `Mutex<Vec<Observation>>` 是进程全局状态，并按成功或失败的每次实际请求追加记录。`take_*` 会取走并清空对应序列，`clear_observations` 同时清空两类数据；它们主要服务独立测试，而非真实 Prometheus 指标后端。

## 依赖与调用关系

直接下游调用边为：`NewRawkvClient -> default_pd_dialer -> NewRawkvClientWithDialer -> PdRawkvDialer::NewClient`；`Put -> TruncateTS -> RawkvClient::BatchPut -> metrics observers -> reset`；`PutRest -> RawkvClient::BatchPut -> metrics observers -> reset`；`Close -> RawkvClient::Close`。RustCodeGraph 的 callees 结果验证了 `NewRawkvClientWithDialer` 到 `NewClient` 以及 `PutRest` 到 `BatchPut`、`SetColumnFamily`、指标和 `reset` 的关系。

`br/pkg/restore/internal/rawkv/lib.rs` 是模块入口，并将 `parity_test.rs`、`rawkv_client_test.rs` 作为独立 `#[cfg(test)]` 模块挂载。`Cargo.toml` 没有任何外部依赖，根 workspace 只把该目录列为成员；这意味着 crate 可以独立编译其本地抽象，但没有真实 TiKV client 实现。

RustCodeGraph 报告目标文件被 `parity_test.rs` 和 `br/pkg/restore/snap_client/import_test.rs` 两个文件“使用”。前者是实际公共契约调用者；后者只因通用名称 `reset` 被图索引关联，没有导入或调用本文件的 RawKV 类型，因此不能作为业务上游证据。仓库清单搜索也未发现其他 Rust Cargo manifest 依赖该 crate。当前可确认的 Rust 上游只有本 crate 的测试模块。

Go 侧生产关系不同：`rawkv_client.go` 被 `br/pkg/restore/log_client/client.go` 使用，Bazel 的 `br/pkg/restore/log_client/BUILD.bazel` 也依赖 `//br/pkg/restore/internal/rawkv`。这是 Go 实现的主链证据，不代表同名 Rust crate 已经接入 `log_client`；Rust 的 `br/pkg/restore/log_client/client.rs` 当前使用自身的移植类型/桩。

## 错误处理与边界

拨号错误和批写错误都以本地 `Error` 返回。`Error::Trace` 不增加上下文，所以下游消息原样暴露；`NewRawkvClientWithDialer` 也不检查空 PD 地址、TLS 字段或 context 取消状态。默认构造返回确定的未配置错误，这是防止本地模式静默联网的显式边界。

`Put` 和 `PutRest` 在 `BatchPut` 失败后不执行 `reset`，保证数据没有在本层丢失；但若底层实现发生“部分写入后返回错误”，本层重试可能重复提交，因为接口没有幂等结果或逐项确认。指标在检查错误之前记录，因此失败请求同样计入批大小和耗时。

`Close` 有意忽略底层错误，且不会先调用 `PutRest`。调用方必须自己处理“先刷残余、后关闭”的顺序；仅调用 Close 可能遗留未提交缓冲。`RawkvClient::BatchPut` 的 key/value 长度一致性由本文件的同一 map 遍历保证，但 trait 的其他调用者仍需由实现自行校验。

所有互斥锁都通过 `unwrap()` 获取。若持锁线程 panic 导致锁中毒，`Context` 操作或 metrics 记录/读取会继续 panic，而不是返回 `Error`。`TruncateTS` 只按字节长度裁剪，不验证 key 确实含时间戳；传入普通长度大于等于 8 的 key 会错误地把尾部当作时间戳用于去重。

## 并发与资源生命周期

`RawkvClient` 与 `PdRawkvDialer` 都要求 `Send + Sync`，并通过 `Arc` 共享；`Context` 的取消状态和 metrics 观测也由互斥锁保护。相反，`RawKVBatchClient::Put`、`PutRest` 和 `SetColumnFamily` 需要 `&mut self`，内部 `HashMap` 没有锁，文件注释明确该批处理器不是线程安全的共享写入器。若要跨线程使用，外层必须串行化整个批处理状态机，而不仅是底层 client。

一个批处理器的生命周期为：由 `NewRawKVBatchClient` 接管底层 client 的共享引用，持续缓冲和多次成功 reset，输入结束时显式 `PutRest`，最终显式 `Close`。它没有 `Drop` 实现，离开作用域不会自动 flush 或调用底层 Close。因为持有 `Arc`，`Close` 也不等于释放底层对象；其他 Arc 持有者仍可继续访问，真正关闭语义由 trait 实现决定。

`Context::cancel` 可被其他克隆持有者并发调用，但本文件的 `Put`、`PutRest` 和构造函数都不主动读取 `ctx.Err()`；能否响应取消完全依赖注入的 dialer/client。全局 metrics 锁仅包围一次 Vec 追加或取出，不覆盖网络调用；不同客户端可并发提交，但观测记录的全局顺序只反映取得锁的顺序。

## 与 Go 版本的对应关系

Rust 的 `RawkvClient` 方法面、`NewRawkvClient` 的 10 秒超时、`KVPair`、`RawKVBatchClient` 字段、满批条件、批内去重、较大 `originTs` 胜出、`PutRest`、成功 reset、失败保留以及忽略 Close 错误，逐项对应 `br/pkg/restore/internal/rawkv/rawkv_client.go`。Rust 独立测试 `rawkv_client_test.rs` 复刻 Go `rawkv_client_test.go` 的容量 3、五条输入、3+2 刷新和重复 key1/key4 场景，并通过排序消除 map 遍历顺序差异。

关键差异在基础设施边界。Go `NewRawkvClient` 直接调用 `tikv/client-go/v2/rawkv.NewClient`，使用真实 `context.Context`、`config.Security`、可变参数 `rawkv.RawOption` 和全局 Prometheus 指标；Rust 用本地 trait、轻量取消容器、字符串安全配置、slice options 和 Vec 指标桩替代，默认拨号器不能联网。Go 用 `hack.MutableString` 避免为 map key 复制字节，Rust 的 `TruncateTS` 总是创建 `Vec<u8>`，并在组批时再次克隆 key/value，内存分配语义不同。

Rust `parity_test.rs::go_rust_public_contract_matches` 在 Go 原测试之外补充了契约边界：空 `PutRest`、较小时间戳不覆盖、BatchPut 错误消息、失败不写入、默认拨号失败、PD 地址和 10 秒超时转发、列族 option、指标观测及 Close 恰好一次。这些是当前 Rust 实现的直接测试证据，但没有证明真实 PD/TiKV、TLS、取消或 Prometheus 集成。

## 扩展指南

若增加新的 RawKV 操作，应先扩展 `RawkvClient` trait，再同步所有 fake/真实实现和独立测试文件 `br/pkg/restore/internal/rawkv/rawkv_client_test.rs`、`parity_test.rs`；不要把测试逻辑内嵌回生产源文件。若 Go 同路径已有对应 API，还需核对 options、错误传播和 context 行为，而不能只增加一个始终成功的桩。

若要接通真实 TiKV，正确扩展点是提供可复用的 `PdRawkvDialer`/`RawkvClient` 实现并让生产上游使用 `NewRawkvClientWithDialer` 或替换默认拨号器。按照仓库规则，所需外部 Rust 客户端能力必须在独立上游仓库移植、提交并发布 tag，再由本仓库以统一 tag 的 Git 依赖引用；不能把依赖复制进 vendor/third_party，也不能使用本地 `[patch]`。接线时必须补真实安全配置、deadline/cancellation、关闭和部分失败语义验证。

若调整批处理或去重规则，应重点覆盖：相等/逆序 `originTs`、短 key 与非 MVCC key、跨批重复、`cap <= 0`、失败后重试、空/非法列族和底层部分成功。改变 `HashMap` 或排序策略还要确保 keys/values 始终配对，并评估额外复制、峰值内存和批次吞吐。

若希望 Close 自动刷盘，应谨慎改变当前 Go 对齐契约：Close 没有 context 且忽略底层错误，无法可靠报告 flush 失败。更安全的调用约定仍是显式 `PutRest(ctx)` 成功后再 Close，并在上层测试错误退出时的数据保留策略。

## 验证依据

- RustCodeGraph `status`：索引包含 7032 个 Rust 文件；`files --filter br/pkg/restore/internal/rawkv` 确认模块内 Rust/Go 实现和独立测试集合。
- RustCodeGraph `node --file br/pkg/restore/internal/rawkv/rawkv_client.rs --offset 1 --limit 420`：读取目标文件完整 361 行，并核对常量、类型、trait、函数、impl、私有辅助和 metrics 模块。
- RustCodeGraph `query/callers/callees`：核对 `NewRawkvClientWithDialer`、`NewRawKVBatchClient`、`PutRest` 等符号；确认 dialer、BatchPut、列族、指标与 reset 的下游边，并识别 `snap_client/import_test.rs::reset` 为同名误匹配而非业务调用。
- crate 边界：`br/pkg/restore/internal/rawkv/Cargo.toml`、`lib.rs` 和根 `Cargo.toml`，确认 library 入口、workspace 成员、Go 包映射、独立测试挂载及空依赖。
- Go 对照：`br/pkg/restore/internal/rawkv/rawkv_client.go`、`rawkv_client_test.go`，以及 Go 生产上游/Bazel 依赖 `br/pkg/restore/log_client/client.go`、`br/pkg/restore/log_client/BUILD.bazel`。
- Rust 测试：`br/pkg/restore/internal/rawkv/rawkv_client_test.rs` 与 `parity_test.rs`，核对满批/残余刷新、批内与跨批重复、较大 TS 胜出、列族、指标、错误、拨号超时和关闭行为。
- 本任务为纯文档分析，按计划未运行 Cargo。结构验证使用任务指定命令检查文档存在且恰好包含 11 个固定二级章节；人工复核区分了 Go 生产主链与尚未接线的 Rust 本地 trait 模式。
