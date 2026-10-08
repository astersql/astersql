# `pkg/session/runtime/bootstrap_wait.rs`

## 文件定位

本文件属于 `astersql-session` crate 的私有 `runtime::bootstrap_wait` 模块。模块由 [`pkg/session/runtime.rs`](../runtime.rs) 的 `mod bootstrap_wait` 装配，不对 crate 外公开；文件中的函数和类型也都限制为 `pub(super)`。它位于会话运行时创建 Domain 之前的 keyspace 启动门禁，以及规范化 bootstrap 流程结束时的版本发布点之间。

生产入口是 [`SessionFactory::from_tikv_store_with_server_info_options`](session.rs) 调用 `check_user_keyspace_bootstrap`：只有 `kv::IsUserKS(store)` 为真时才读取已注册的 SYSTEM storage 并进入本文的等待和版本检查。完成端是 [`BootstrapCanonicalDomain`](session.rs)，它在 mysql/sys 元数据和全局变量处理完毕后调用 `finish_store_bootstrap_version`，将当前 bootstrap 版本写入同一个元数据键。

## 核心职责

本文件承担四项紧密相关的职责：

1. `must_get_store_bootstrap_version` 在带 `InternalTxnBootstrap` 来源标记的全新可重试事务中读取 `BootstrapKey`，将键不存在解释为版本 `0`。
2. `wait_system_boot_version_with_clock` 最多轮询 SYSTEM keyspace 360 次，并按 1、2、4、随后恒定 5 秒退避；每 5 次未就绪尝试记录一次累计等待日志。
3. `check_system_bootstrap_version` 拒绝未完成 bootstrap 的 SYSTEM keyspace，也拒绝用户 keyspace 的目标版本领先于 SYSTEM keyspace。
4. `finish_store_bootstrap_version` 在 bootstrap 完成后通过新事务发布版本，使等待方观察到与完成方写入完全一致的键。

它不是完整的 session bootstrap 实现，也不创建或注册 SYSTEM storage；这些工作分别位于 [`pkg/session/runtime/session.rs`](session.rs) 和 store 注册表。它只提供同步所需的元数据读写、等待策略与版本不变量。

## 主要符号

- `must_get_store_bootstrap_version(store: &dyn kv::Storage) -> i64`：读取持久化 bootstrap 版本。值必须是 UTF-8 十进制 `i64`；不存在返回 `0`，其他读取、解码、解析或事务错误均走致命失败路径。
- `BootstrapWaitClock`：仅抽象 `sleep(Duration)` 与 `log_wait()`，不抽象元数据读取。因此测试可消除真实等待和捕获日志时点，但每轮仍通过真实 `kv::Storage` 事务读取状态。
- `SystemBootstrapClock(Instant)`：生产时钟。构造时保存起点；`sleep` 阻塞当前线程；`log_wait` 通过 `BgLogger` 输出从起点开始的 `total-waited`。
- `wait_system_boot_version_with_clock(store, clock) -> i64`：轮询核心。任何非零版本立即返回；耗尽 360 次后返回最后一次读到的 `0`。
- `check_system_bootstrap_version(store, target, current, clock)`：组合等待和门禁。`target` 是用户 keyspace 将运行到的当前代码版本，`current` 是用户 keyspace 已持久化版本，后者只用于 fatal 日志诊断。
- `finish_store_bootstrap_version(store, version)`：把十进制版本字节写入 `astersql_meta::transaction_meta_string_key(b"BootstrapKey")`。

文件没有模块级常量、条件编译项或公开 crate API；重试次数、日志频率与退避序列目前直接写在轮询函数中。

## 执行流程

启动侧流程如下：

1. `SessionFactory::from_tikv_store_with_server_info_options` 在调用 `from_storage` 构造 serving Domain 前执行 `check_user_keyspace_bootstrap`。
2. 非用户 keyspace 直接返回。用户 keyspace 先用 `must_get_store_bootstrap_version` 读取自身当前版本，再通过 `astersql_store::GetSystemStorage` 获取 SYSTEM storage，并转换成 canonical TiKV storage。
3. `check_system_bootstrap_version` 调用 `wait_system_boot_version_with_clock`。第 1 次读取发生在任何 sleep 之前；若版本非零，立即成功，不睡眠也不记等待日志。
4. 若版本为零，每轮在读取后按尝试序号睡眠：第 1、2、3 轮分别为 1、2、4 秒，从第 4 轮起为 5 秒。第 5、10、……、360 次失败读取后先记日志再睡眠。
5. 非零版本必须同时满足 `target <= system_version`；否则进程以 fatal/panic 停止，避免用户 keyspace 使用 SYSTEM keyspace 尚未具备的元数据版本。
6. 通过门禁后，调用者才继续 Domain 构造。

发布侧由 `BootstrapCanonicalDomain` 在 bootstrap 主体成功完成后调用 `finish_store_bootstrap_version(currentBootstrapVersion)`。写入经可重试新事务提交，后续等待轮次会重新开启事务，因此能观察到该提交。

## 数据与状态

唯一跨调用持久状态是事务元数据键 `transaction_meta_string_key(b"BootstrapKey")`，值为十进制 `i64` 字节。`0` 同时表示键不存在或显式写入零，即“尚未 bootstrap”；正数表示已发布版本。读取端没有进程内缓存，因此每轮以新的事务观察存储状态。

轮询的局部状态包括 `version` 和 `attempt`。360 次全部未就绪时会执行 360 次 sleep，总预算为 `1 + 2 + 4 + 357×5 = 1792` 秒，并记录 72 次等待日志。`SystemBootstrapClock` 的 `Instant` 只用于日志耗时，不参与重试判定。

`target`、`current` 与 SYSTEM `version` 的含义不可互换：门禁只比较 `target` 和 SYSTEM `version`；`current` 保留在错误日志中，用于说明用户 keyspace 的升级起点。

## 依赖与调用关系

上游调用边：

- `SessionFactory::from_tikv_store_with_server_info_options` → `check_user_keyspace_bootstrap` → `must_get_store_bootstrap_version`（用户 store）→ `check_system_bootstrap_version`（SYSTEM store）。
- `check_system_bootstrap_version` → `wait_system_boot_version_with_clock` → 每轮 `must_get_store_bootstrap_version`。
- `BootstrapCanonicalDomain` → `finish_store_bootstrap_version`，发布等待方所读的版本。
- [`bootstrap_wait_test.rs`](bootstrap_wait_test.rs) 直接调用四个函数并以测试时钟实现 `BootstrapWaitClock`。

下游依赖：

- `astersql-kv`：`Storage`/`Transaction`、`Context`、`WithInternalSourceType`、`InternalTxnBootstrap`、`RunInNewTxn`、`IsErrNotFound` 与统一错误构造。
- `astersql-meta`：生成 Go 兼容的事务元数据字符串键。
- `astersql-util-logutil`：生产 Info/Fatal 日志及结构化字段。
- 标准库 `Duration`、`Instant` 和 `thread::sleep`：同步等待与累计耗时。

[`pkg/session/Cargo.toml`](../Cargo.toml) 将上述三个 AsterSQL crate 声明为普通依赖；测试使用的 `fail` 是 dev-dependency。`bootstrap_wait.rs` 本身没有 feature gate，`nextgen` feature 通过配置相关 crate 传播，但实际是否进入门禁由运行时的 `kv::IsUserKS` 决定。

## 错误处理与边界

- `BootstrapKey` 不存在不是错误，映射为 `0` 并触发等待。
- 非 UTF-8 值或不能解析为 `i64` 的值转换为 `kv::Error`，随后由 `must_get_store_bootstrap_version` 记录 Fatal 并 panic。
- `RunInNewTxn(..., true, ...)` 允许 KV 层重试可重试事务错误；最终错误、不可重试错误和提交失败同样 Fatal/panic，绝不会伪装成“未 bootstrap”。
- 360 次均为零时，等待函数返回 `0`；`check_system_bootstrap_version` 将其升级为 “SYSTEM keyspace is not bootstrapped” 的 Fatal/panic。
- SYSTEM storage 未注册或不能转换为 canonical TiKV store 的错误在直接调用者 `check_user_keyspace_bootstrap` 中 panic，而不在本文件吞掉。
- `target > system_version` 是不兼容状态并 Fatal/panic；相等或 SYSTEM 版本更高均允许继续。
- `finish_store_bootstrap_version` 的事务错误直接 panic。它不更新 Go 版本中的 `StoreBootstrappedKey` 内存缓存；Rust 当前调用链依赖持久化键，并由读取端每次访问存储。

## 并发与资源生命周期

等待是同步、阻塞且不可取消的：`SystemBootstrapClock::sleep` 直接阻塞当前线程，函数没有 cancellation token、kill signal 或异步任务。该选择与 Go 注释描述的 bootstrap 背景上下文语义一致。最长正常耗尽约 29 分 52 秒，期间持有的是调用栈与时钟状态，而不是一个跨 sleep 存活的 KV 事务。

每次轮询都调用 `RunInNewTxn`，事务在闭包完成及提交/重试后结束，再进入 sleep；因此 SYSTEM bootstrap 的并发提交能在下一轮被观察。测试 `bootstrap_wait_observes_system_commit_after_logged_retry` 在第一次日志之后发布版本，证明下一轮读取会结束等待。

文件自身没有锁、通道、线程创建或共享可变状态。SYSTEM storage 的全局注册生命周期由 `astersql_store` 管理；本文件只借用 `&dyn Storage`。生产完成端必须在所有 bootstrap 元数据真正可用后再发布版本，否则等待方会过早通过门禁。

## 与 Go 版本的对应关系

直接对照是 [`pkg/session/session.go`](../session.go) 中的 `waitSystemBootVersion`、`mustGetStoreBootstrapVersion`、`bootstrapSessionImpl` 和 `finishBootstrap`：

- 两版都把 `0` 视为 `notBootstrapped`，最多尝试 360 次，退避为 1、2、4、5……秒，每 5 次输出累计等待日志，总等待接近 30 分钟。
- 两版都在带 `InternalTxnBootstrap` 来源的可重试新事务中读取版本，并把最终读取错误作为 fatal。
- 两版都在用户 keyspace 启动时先等 SYSTEM keyspace，再保证用户目标版本不高于 SYSTEM 版本。
- 两版的等待都不响应 kill/cancel；Go 源码明确记录了该限制，Rust 以同步 `thread::sleep` 保持相同行为。
- Go 通过 `meta.Reader/Mutator` 访问 bootstrap 版本；Rust 直接使用相同语义的 `BootstrapKey` 编码。Rust 的完成函数接受显式 `version`，Go `finishBootstrap` 固定写 `currentBootstrapVersion`。
- Go `finishBootstrap` 同时设置 store 的 `StoreBootstrappedKey` 内存选项；Rust 此文件只发布持久化键。Rust 的调用者显式传当前版本，等待读取也不走缓存，因此本文覆盖的 SYSTEM 等待契约仍闭合；不能据此声称 Rust 已复刻 Go 的全部 bootstrap 缓存行为。

Go 测试 [`pkg/session/session_test.go`](../session_test.go) 验证等待能观察并发提交、耗尽确实超过 29 分钟，以及用户目标版本领先 SYSTEM 时在创建 session 前 fatal。Rust 独立测试以可替换时钟保留相同分支，同时避免真实等待。

## 扩展指南

- 修改退避、最大尝试次数或日志频率时，应集中调整 `wait_system_boot_version_with_clock`，并同步更新 [`bootstrap_wait_test.rs`](bootstrap_wait_test.rs) 中对 sleep 序列、总时长、日志次数和最后一次 sleep 的断言；同时核对 Go `waitSystemBootVersion`，避免无意产生跨语言启动差异。
- 增加可取消等待时，需要修改 `BootstrapWaitClock` 或函数参数，并明确取消是返回错误还是 fatal。该变化会偏离 Go 当前“bootstrap 期间不可 kill”的契约，必须补独立测试，不能只在生产时钟中提前返回。
- 改变版本存储格式或键名时，读取和 `finish_store_bootstrap_version` 必须原子地保持一致，并核对 `astersql-meta` 的 Go 兼容键编码及 Go `meta.Reader/Mutator`。
- 新增错误返回而非 panic 时，应从 `must_get_store_bootstrap_version`、`check_system_bootstrap_version` 一路接到 `check_user_keyspace_bootstrap` 和 SessionFactory 构造结果，保留事务错误与“未完成”状态的区分。
- 新增测试应继续放在独立的 [`bootstrap_wait_test.rs`](bootstrap_wait_test.rs)，不要内嵌到生产文件。并发可见性应使用真实 mock storage；纯时间策略可继续通过测试时钟验证。
- 性能风险主要是缩短间隔造成存储轮询放大，或延长间隔造成节点启动延迟；兼容风险主要是允许用户 keyspace 领先 SYSTEM、过早发布版本，或让读写双方采用不同键/编码。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标仓库索引可用。
- RustCodeGraph `node --file pkg/session/runtime/bootstrap_wait.rs`：核对文件全部 130 行、所有符号、事务读写、退避和 fatal 分支；图报告该文件由 `runtime/session.rs` 等文件使用。
- RustCodeGraph `query`：确认 `BootstrapWaitClock`、`must_get_store_bootstrap_version`、`wait_system_boot_version_with_clock`、`check_system_bootstrap_version`、`finish_store_bootstrap_version` 的精确符号与签名。
- RustCodeGraph/源码引用：[`runtime/session.rs`](session.rs) 第 848、1978–1991、2484–2646 行确认启动门禁、SYSTEM storage 获取和 bootstrap 完成发布；[`runtime.rs`](../runtime.rs) 第 83–85 行确认生产模块与独立测试模块装配。
- [`pkg/session/Cargo.toml`](../Cargo.toml)：确认 crate 名、`nextgen` feature，以及 `astersql-kv`、`astersql-meta`、`astersql-util-logutil` 和测试 `fail` 依赖。
- [`bootstrap_wait_test.rs`](bootstrap_wait_test.rs)：确认可重试提交、并发发布可见性、`1792` 秒/72 次日志预算、立即就绪、版本领先、无效元数据和不可重试提交错误等边界。
- [`pkg/session/session.go`](../session.go) 与 [`pkg/session/session_test.go`](../session_test.go)：确认 Go 的 360 次指数封顶退避、不可取消说明、SYSTEM/user 版本门禁、并发提交与长时间耗尽语义。
- 本任务为纯文档分析，按计划未运行 Cargo；交付前使用任务指定命令验证目标文档存在且固定二级章节恰好为 11 个，并人工复核未把 Go 缓存行为或未接线能力写成 Rust 现状。
