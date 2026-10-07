# `br/pkg/utils/error_handling.rs`

## 文件定位

`error_handling.rs` 属于 `astersql-br-pkg-utils` library crate；crate 根在 `br/pkg/utils/lib.rs`，其中以 `#[path = "error_handling.rs"] pub mod error_handling` 挂载本模块，并将主要类型、构造函数和判定函数再导出到 crate 根。`br/pkg/utils/Cargo.toml` 的 `[package.metadata.porting]` 指明其 Go 包来源为 `br/pkg/utils`；同路径权威对照实现是 `br/pkg/utils/error_handling.go`。

该文件位于 BR 的错误到退避策略转换边界：它不执行 RPC、不睡眠，也不拥有完整重试循环，而是把 BR protobuf 风格错误或非结构化错误消息归类为 `StrategyRetry`、`StrategyGiveUp` 或中间态 `StrategyUnknown`。实际消费这一决策的直接生产入口是 `br/pkg/utils/backoff.rs` 中 `BackoffStrategyImpl::NextBackoff`，后者据此递减尝试次数、扩大延迟或停止退避。

当前 Rust crate 为适配精简构建，没有直接依赖完整 kvproto；`crate::kvproto::brpb::Error` 由 `br/pkg/utils/stubs.rs` 提供，只保留消息文本及 KV、Region、ClusterId 三个布尔标志。因此本文描述的是仓库当前可见实现，不把完整 protobuf oneof 或真实 RPC 接线当作已经具备的能力。

## 核心职责

本文件承担四项相互衔接的职责：

1. 用 `ErrorHandlingStrategy` 和 `ErrorHandlingResult` 表达“重试/放弃/尚未分类”及其运维可读原因。
2. 在 `handleBackupProtoError` 中优先识别结构化 KV、Region、ClusterId 错误，避免易变的文本匹配覆盖协议语义。
3. 在 `HandleUnknownBackupError` 中按固定优先级识别不可恢复存储错误、取消错误和瞬时网络/对象存储错误；无法识别的错误才进入逐 store 次数配额。
4. 用 `ErrorContext` 保存场景名和未知错误计数，使同一退避策略跨多次调用维持决策状态。

职责边界很明确：`MessageIsRetryableStorageError` 等函数只做大小写不敏感的子串分类；`HandleUnknownBackupError` 只返回策略并在可重试存储错误时记录警告；真正的延迟、最大尝试次数及错误类型白名单/黑名单由 `br/pkg/utils/backoff.rs` 管理。

## 主要符号

- `retryableErrorMsg: &[&str]`：瞬时网络、HTTP body、对象存储超时以及 S3 风格 `RequestTimeout`/`InvalidPart` 等可重试文本表。命中此表不会消耗 unknown 配额。
- `ioMsg`、`notFoundMsg`、`permissionDeniedMsg`、`credentialNotFoundMsg`：不可恢复存储错误的文本关键字。前三类判定最终给出含 store id 和处置建议的原因文案。
- `unreachableRetryMsg`、`retryOnKvErrorMsg` 等原因常量：构成稳定的策略说明契约；Rust 单测与 Go 测试都直接断言其中多项文案。
- `ErrorHandlingStrategy`：显式赋值为 `StrategyRetry = 0`、`StrategyGiveUp = 1`、`StrategyUnknown = 2`，与 Go `iota` 顺序一致。`StrategyUnknown` 是分类过程的中间态，不代表调用方应无条件重试。
- `ErrorHandlingResult { Strategy, Reason }`：一次纯决策的返回值；`Reason` 既用于断言，也用于日志和运维解释。
- `ErrorContext`：保存 `Arc<Mutex<HashMap<u64, i32>>>`、每个 UUID 的限制 `encounterTimesLimitation` 和场景描述 `description`。字段保持模块私有，调用方通过构造函数创建上下文。
- `NewErrorContext(scenario, limitation)`：通用构造器；限制按 UUID 分桶解释，而不是全局总次数。
- `NewDefaultContext()`：场景为 `default`、unknown 配额为 1。
- `NewZeroRetryContext(scenario)`：unknown 配额为 0，第一次未分类消息就放弃；多个 `backoff.rs` 策略工厂使用它。
- `HandleBackupError(err, store_id, ec)`：结构化 BR 错误总入口。`None` 被视为理论上的异常空值，但为兼容 Go 返回 Retry；非空值先交给 `handleBackupProtoError`，只有结果为 Unknown 且消息非空时才回落文本分类。
- `handleBackupProtoError(e)`：模块内部结构化分类器，依次检查 KV、Region、ClusterId 标志；前三者分别得到 Retry、Retry、GiveUp，均未设置时得到 Unknown。
- `HandleUnknownBackupError(msg, uuid, ec)`：公开文本分类器和 unknown 配额状态机，也是 `backoff.rs::BackoffStrategyImpl::NextBackoff` 的直接下游调用。
- `messageIsNotFoundStorageError`、`messageIsPermissionDeniedStorageError`、`messageIsCredentialNotFoundError`：不可恢复错误辅助判定；均先做 ASCII 小写化。
- `MessageIsRetryableStorageError`：遍历 `retryableErrorMsg` 并做包含判断，任一命中即返回 `true`。

文件没有 trait、`impl` 块、条件编译项或异步函数；公开 API 使用 Go 风格命名，crate 根通过 `#![allow(non_snake_case, ...)]` 明确保留这种迁移期接口风格。

## 执行流程

结构化入口 `HandleBackupError` 的流程如下：

1. `err == None` 时立即返回 `StrategyRetry / unreachable retry`，不读取或修改 `ErrorContext`。
2. 对非空错误调用 `handleBackupProtoError`。KV error 返回 Retry，Region error 返回 Retry，ClusterId error 返回 GiveUp。
3. 若结构化分类得到 Unknown 且 `err.get_msg()` 非空，则把消息、store id 和上下文传给 `HandleUnknownBackupError`。
4. 若消息为空，保留 `StrategyUnknown / unknown error` 原样返回；因此只有带文本的未知协议错误才会进入启发式和配额逻辑。

`HandleUnknownBackupError` 严格按下列优先级短路：

1. 同时含 `io` 与 `notfound`：认为 TiKV 节点找不到共享文件或目录，GiveUp。
2. 含 `permissiondenied`：认为 TiKV 无存储权限，GiveUp。
3. 含 `credential info not found`：认为对象存储凭证缺失，GiveUp。
4. 含 `context canceled`：认为上下文已取消，GiveUp。
5. 命中 `retryableErrorMsg`：记录含场景和原消息的 Warn 日志并 Retry，不触碰 unknown 计数。
6. 其余消息取得计数表互斥锁，递增当前 `uuid` 的次数；新次数小于等于限制时 Retry，超过限制时 GiveUp。

这一顺序是行为契约。例如同时带有不可恢复存储关键字和可重试网络关键字的消息会先被判定为 GiveUp；改变分支顺序会改变实际恢复策略。

在生产侧直接接线中，`br/pkg/utils/backoff.rs::BackoffStrategyImpl::NextBackoff` 取错误链最后一项，以固定 UUID `0` 调用 `HandleUnknownBackupError`。Retry 会进入 `doBackoff`；取消类原因还会结合 gRPC Canceled 与上下文取消信息复判；其他结果再交给策略自己的 retry/non-retry predicate，最终决定继续或停止。也就是说，本文件提供基础分类，`backoff.rs` 仍保留场景特定的二次判定。

## 数据与状态

唯一可变状态是 `ErrorContext::encounterTimes`。键是调用方传入的 store UUID，值是当前上下文内该 UUID 遇到的“未命中任何已知规则”的消息次数。计数发生在所有确定性分支之后，因此 KV/Region、缺文件、权限、凭证、取消和可重试存储错误都不会消耗该配额。

配额判断采用“先加一，再判断 `times <= limitation`”：限制为 3 时第 1、2、3 次 Retry，第 4 次 GiveUp；限制为 0 时第 1 次即 GiveUp。计数不会在成功、已知错误或 GiveUp 后自动清零，也没有淘汰 UUID 的机制；上下文的拥有者负责控制其生命周期，长期跨大量 store 复用会让 `HashMap` 随不同 UUID 数量增长。

`ErrorContext` 的 `Clone` 是共享而非快照语义：`Arc` 被克隆，两个句柄访问同一计数表。`error_handling_test.rs::cloned_error_context_shares_unknown_retry_quota` 明确验证限制为 1 时，第一个克隆句柄 Retry 后，第二个句柄对同 UUID 立即 GiveUp。`description` 和限制值则按值克隆，创建后没有公开修改入口。

`ErrorHandlingResult` 与策略枚举都是拥有型的小值对象；原因字符串每次构造或复制，不借用错误输入。辅助分类函数没有隐藏状态。

## 依赖与调用关系

上游与导出关系：

- `br/pkg/utils/lib.rs` 声明模块，并在 crate 根再导出 `ErrorContext`、两个处理入口、三个构造器、策略/结果类型及辅助判定函数。
- `br/pkg/utils/backoff.rs` 的多个策略工厂通过 `NewZeroRetryContext`、`NewDefaultContext` 或 `NewErrorContext` 配置不同场景；`BackoffStrategyImpl::NextBackoff` 直接调用 `HandleUnknownBackupError`。
- `br/pkg/utils/error_handling_test.rs` 覆盖主要分类分支和共享计数语义；`br/pkg/utils/parity_test.rs::go_rust_public_contract_matches` 从 crate 根验证空错误、取消消息和连接重置等公开契约；`br/pkg/utils/backoff_test.rs` 间接使用默认上下文验证退避行为。

下游依赖：

- `crate::kvproto::brpb`：提供当前精简的 `Error` 消息壳及 `has_*`/`get_msg` 方法，真实位置是 `br/pkg/utils/stubs.rs`。
- `std::collections::HashMap`、`std::sync::{Arc, Mutex}`：实现按 UUID 的共享同步计数。
- `astersql_br_pkg_logutil::{log, Field}`：仅在可重试存储文本命中时写 Warn 日志，字段为 `description` 与原始 `error`。

RustCodeGraph 显示目标文件被 `backoff.rs`、`backoff_test.rs`、`error_handling_test.rs`、`lib.rs`、`parity_test.rs` 五个文件使用；其中业务链上的关键边是 `BackoffStrategyImpl::NextBackoff -> HandleUnknownBackupError`。图还确认 `HandleBackupError -> handleBackupProtoError`，以及 Unknown 且有消息时 `HandleBackupError -> HandleUnknownBackupError`。

## 错误处理与边界

- `HandleBackupError(None, ...)` 返回 Retry 是 Go 兼容兜底，不表示“无错误也应正常重试”；原因名 `unreachable retry` 已说明正常路径不应到达。
- 空的、未设置结构化标志的 `brpb::Error` 返回 `StrategyUnknown`，不会消耗配额。调用者不能把 Unknown 自动等同 Retry。
- 文本分类只做 ASCII 小写和子串包含，不解析错误链、状态码或结构化对象。`io` 极短，NotFound 规则必须同时命中 `io` 才成立，但仍存在误匹配风险；Go 文件也将这类规则标为 UNSAFE/TODO。
- 当前 Rust stub 用三个独立布尔值模拟 protobuf oneof；若异常地同时设置多个标志，固定检查顺序为 KV、Region、ClusterId，最先命中的分支胜出。完整 Go protobuf oneof 通常不会出现该组合。
- 获取计数锁使用 `expect("mutex poisoned")`。若持锁线程 panic 导致 poisoning，后续分类会 panic，而不是返回 GiveUp；这是当前 Rust 与 Go `sync.Mutex` 行为的实现差异。
- `to_ascii_lowercase` 只标准化 ASCII；关键字本身均为 ASCII，符合当前消息表，但不是通用 Unicode 大小写折叠。
- 负数 `limitation` 没有被拒绝，第一次 unknown 在递增到 1 后就会超过限制并 GiveUp。构造器没有参数校验。
- 不可恢复原因文案包含 store id 和明确 workaround；修改标点、空格或大小写可能破坏 Go/Rust 测试及外部日志匹配，应视作兼容性变更。

## 并发与资源生命周期

模块不创建线程、异步任务、通道、网络连接、文件句柄或计时器。并发边界仅存在于 `ErrorContext` 的共享计数表：`Arc` 允许上下文克隆后跨所有权边界共享，`Mutex` 将一次 UUID 查找、递增和阈值判断包在同一临界区，避免相同 store 的并发 unknown 错误丢失更新。

锁只在最后的 unknown 分支获取；所有已知分支、文本检查和日志都在锁外完成，临界区短且不调用外部代码。互斥锁守卫在函数返回前按 RAII 自动释放。调用签名仍要求 `&mut ErrorContext`，所以安全 Rust 中同一个句柄不能被同时可变借用；并发共享通常需要各线程持有 `Clone` 句柄，而这些句柄底层共享同一 `Arc<Mutex<_>>`。

计数与创建它的 `ErrorContext` 同寿命：最后一个克隆释放后，`Arc` 计数归零并释放 `HashMap`。没有后台清理或显式 close。可重试消息日志同步发生在判定期间；本文件不控制日志后端的缓冲和刷新。

## 与 Go 版本的对应关系

Rust 逐项复刻 `br/pkg/utils/error_handling.go` 的核心契约：错误表和原因常量文本一致；策略枚举顺序一致；`NewDefaultContext` 限制为 1、`NewZeroRetryContext` 限制为 0；结构化错误优先于消息启发式；未知错误按 store 计数并在超过限制后放弃；不可恢复错误的 workaround 文案与 Go 测试相同。

两侧测试也保持对应：Go 的 `TestHandleError`、`TestHandleErrorMsg`、`TestHandleCredentialNotFoundError` 分别映射到 Rust 的 `test_handle_error`、`test_handle_error_msg`、`test_handle_credential_not_found_error`。Rust 另外增加 `cloned_error_context_shares_unknown_retry_quota`，锁定 `Arc` 克隆共享状态这一 Rust 特有语义；`parity_test.rs` 再从公开再导出层做抽样契约检查。

需要注意的实现差异：

- Go 构造器返回 `*ErrorContext`；Rust 返回拥有型 `ErrorContext`，调用时传 `&mut`。
- Go 将 mutex 与 map 分字段持有，复制指针自然共享整体对象；Rust 只把 map/锁放进 `Arc`，克隆时描述和限制按值复制、计数共享。
- Go 使用真实 `backuppb.Error.Detail` oneof 的类型 switch；当前 Rust 使用 `stubs.rs` 的三个布尔标志，属于迁移期精简协议边界。
- Go mutex 不存在 poisoning；Rust 在 poisoning 时 panic。
- Go 注释指出凭证规则当前针对 Azure Blob；Rust 实现保留相同文本规则和原因，但源码注释没有重复限定云厂商，不能据此推断支持面已经扩大。

## 扩展指南

新增结构化错误类别时，应先扩展真实/精简 `brpb::Error` 边界，再在 `handleBackupProtoError` 中明确其优先级、策略和稳定原因；同步更新 `error_handling_test.rs` 及 Go 对照测试。若未来从 stub 切换到完整 kvproto，应重点验证 oneof 互斥语义，不能只机械替换方法名。

新增文本规则时，优先推动上游提供错误类型或状态码；确需子串规则时，应决定它属于不可恢复还是瞬时错误，并把规则放到正确的短路位置。必须增加大小写变体、组合消息和“不得消耗 unknown 配额”的回归断言，同时检查短关键字带来的误报。修改 `retryableErrorMsg` 需评估可能导致无限或过多重试的兼容与性能风险。

修改配额逻辑时，应保持“按 UUID 分桶”“克隆共享计数”和阈值边界的明确选择；至少同步 `cloned_error_context_shares_unknown_retry_quota` 和 `test_handle_error_msg`。若要支持重置、淘汰或有界 map，最合适的接入点是 `ErrorContext` 的私有状态与新增显式方法，不应让调用者直接修改 map。

改变公开符号、枚举值或原因文案时，还要同步 `br/pkg/utils/lib.rs` 的再导出、`br/pkg/utils/parity_test.rs` 的公开契约测试，以及 `backoff.rs::BackoffStrategyImpl::NextBackoff` 对 `contextCancelledMsg` 和策略值的依赖。测试逻辑必须继续放在独立的 `br/pkg/utils/error_handling_test.rs`，不要嵌回生产文件。

## 验证依据

本说明基于以下直接证据完成：

- RustCodeGraph `status`：索引包含 11,467 个文件，其中 Rust 7,032 个；目标文件可索引。
- RustCodeGraph `node --file br/pkg/utils/error_handling.rs`：读取目标文件 1–260 行，核对全部常量、类型、构造器、分类函数及分支顺序。
- RustCodeGraph 对 `HandleBackupError`、`HandleUnknownBackupError`、`handleBackupProtoError`、`MessageIsRetryableStorageError` 的 `query`，以及围绕目标文件和 `backoff.rs::NextBackoff` 的 `explore`：核对公开/内部符号、直接调用边与五个使用文件。
- RustCodeGraph `node`：读取 `br/pkg/utils/backoff.rs` 400–549 行，确认 `BackoffStrategyImpl::NextBackoff` 的实际消费方式；读取 `br/pkg/utils/stubs.rs` 585–679 行，确认当前 `brpb::Error` 是消息加三个标志的本地精简壳；读取 `br/pkg/utils/lib.rs` 1–220 行，确认模块挂载、再导出和独立测试挂载；读取 `br/pkg/utils/error_handling_test.rs` 1–182 行及 `br/pkg/utils/parity_test.rs` 1–190 行，确认边界与公开契约测试。
- 直接读取 `br/pkg/utils/Cargo.toml`：确认 crate 名、library 根、Go 包映射、精简构建说明，以及日志依赖来自 `astersql-br-pkg-logutil`。
- 直接读取 `br/pkg/utils/error_handling.go` 与 `br/pkg/utils/error_handling_test.go`：核对 Go 的类型 switch、分支顺序、计数边界、原因字符串与三组对应测试。

按任务范围未运行 Cargo 或任何代码测试；本任务只新增说明文档。交付前使用任务指定命令检查目标文件存在且恰好包含上述 11 个固定二级标题，并人工复核文档未把 stub 描述为完整生产协议实现。
