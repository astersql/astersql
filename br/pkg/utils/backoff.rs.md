# `br/pkg/utils/backoff.rs`

## 文件定位

本文件属于 `astersql-br-pkg-utils` library crate；crate 根由 `br/pkg/utils/Cargo.toml` 的 `[lib] path = "lib.rs"` 指向 `br/pkg/utils/lib.rs`，后者以 `pub mod backoff` 挂载本文件，并把 `BackoffStrategy` 再导出到 crate 根。它位于 BR 公共工具层，不执行具体的备份或恢复 RPC，而是把“还能尝试几次、失败后等待多久、哪些错误应继续”封装成可变策略，交给 `br/pkg/utils/retry.rs` 的 `WithRetry`、`WithRetryV2` 和 `WithRetryReturnLastErr` 驱动。

RustCodeGraph 的文件关系显示本文件由 `br/pkg/utils/lib.rs`、`br/pkg/restore/snap_client/import.rs` 和 `br/pkg/restore/log_client/client.rs` 使用；仓库源码搜索还能确认 `br/pkg/backup/store.rs` 和 `br/pkg/conn/util/util.rs` 通过 crate API 消费其策略。因而它是 BR 的重试“决策层”，上游业务负责发起操作，下游等待与取消由重试循环负责。

## 核心职责

1. 用 `BackoffStrategy` 统一暴露 `NextBackoff(&mut self, err)` 与 `RemainingAttempts()`，把错误分类、计数推进和等待时长计算从重试循环中分离。
2. 用 `RetryState` 提供不关心错误类型的基础指数退避状态机；用 `BackoffStrategyImpl` 提供结合 `ErrorContext`、重试/非重试判定函数的 BR 通用策略。
3. 为 Import/Download/Peer Download/Backup SST、PD、磁盘检查、recovery、flashback、checksum、raw client 等场景提供具名工厂和参数预置。
4. 对齐 Go 的特殊语义：只检查组合错误链最后一个错误；未知备份错误先交给 `HandleUnknownBackupError`；真实上下文取消应停止，而 peer-download 可把仅由 gRPC 返回的 `Canceled` 当作可重试。
5. 提供 `SqlErrNoRows` 与 `IoEof` 本地哨兵，以及基于错误类型/错误文案的分类辅助，以适应当前 Rust crate 未直接依赖完整 SQL、gRPC 类型体系的边界。

## 主要符号

- `BackoffStrategy`：公开 trait。`NextBackoff` 会推进策略状态并返回本次等待时间，`RemainingAttempts` 供外层循环判断是否继续；它不是纯查询接口。
- `ConstantBackoff(Duration)`：固定等待策略，忽略错误并始终报告 `i16::MAX` 次剩余机会。它表达“足够大的近似无限重试”，真正退出仍依赖成功或外部取消。
- `RetryState { maxRetry, retryTimes, maxBackoff, nextBackoff }`：错误无关的指数退避状态。`InitialRetryState` 初始化；`ShouldRetry` 判断上限；`ExponentialBackoff` 先增加计数、返回当前等待值，再把下一次等待翻倍并封顶；`GiveUp` 直接耗尽；`ReduceRetry` 无边界检查地回退计数。
- `BackoffOption = Box<dyn FnMut(&mut BackoffStrategyImpl)>` 与 `WithRemainingAttempts`、`WithDelayTime`、`WithMaxDelayTime`、`WithErrorContext`、`WithRetryErrorFunc`、`WithNonRetryErrorFunc`、`WithRetryableGRPCCanceled`：函数式配置入口。
- `BackoffStrategyImpl`：通用实现，保存剩余次数、当前/最大延迟、会跨重试累计的 `ErrorContext`、两个错误判定函数及 gRPC Canceled 特例开关。
- `NewBackoffStrategy`：默认组装器。默认 1 次、1 秒当前延迟、10 秒上限、`NewZeroRetryContext("default")`、全错误可重试、无错误为非重试；选项按传入顺序覆盖字段。
- `NewTiKVStoreBackoffStrategy` / `NewTiKVStoreBackoffStrategyWithOptions`：TiKV 通用策略。重试 EpochNotMatch、DownloadFailed、IngestFailed、PDLeaderNotFound 和指定 gRPC code；停止于上下文取消、空 range、缺失 rewrite rule。
- SST 工厂：`NewImportSSTBackoffStrategy` 为 16 次/40ms/10s，`NewDownloadSSTBackoffStrategy` 与 `NewPeerDownloadSSTBackoffStrategy` 为 8 次/1s/4s，`NewBackupSSTBackoffStrategy` 为 5 次/2s/3s；peer 版本额外打开传输层 gRPC Canceled 重试。
- PD 工厂：`NewPDBackoffStrategy` 将 TotalKVMismatch、`IoEof` 和 PD gRPC code 视为可重试，将上下文取消、`DeadlineExceeded`、`SqlErrNoRows` 视为不可重试；aggressive 预置为 32 次/50ms/2s，conservative 为 600 次/500ms/300s。
- 其他工厂：`NewDiskCheckBackoffStrategy`、`NewRecoveryBackoffStrategy`、`NewFlashBackBackoffStrategy`、`NewChecksumBackoffStrategy`、`NewRawClientBackoffStrategy`；其中 recovery/flashback/checksum 没有调用 `WithMaxDelayTime`，实际沿用构造器默认 10 秒上限。
- `is_tikv_retry_err`、`is_tikv_non_retry_err`、`is_pd_retry_err`、`is_pd_non_retry_err`、`is_disk_check_retry_err`：错误白名单/黑名单；`grpc_code_is_retryable` 与 `grpc_message_has_code` 通过多种字符串格式匹配 gRPC code。

## 执行流程

典型调用链为“业务工厂 → `NewBackoffStrategy` → 重试循环 → `NextBackoff`”。例如 `br/pkg/backup/store.rs` 把 `NewBackupSSTBackoffStrategy()` 传入重试调用；`br/pkg/restore/snap_client/import.rs::downloadWithOptionalPeerRetry` 根据能力探测选择 peer 或 legacy download 策略；`br/pkg/conn/util/util.rs` 在 PD/store 查询中使用 aggressive PD 策略。

`br/pkg/utils/retry.rs::WithRetryV2` 在 `RemainingAttempts() > 0` 时调用业务闭包：成功立即返回；失败先收集错误并检查上下文，再调用 `NextBackoff` 推进状态，最后通过可被取消打断的超时等待。`WithRetryReturnLastErr` 的控制流相近，但最终只返回最后一次错误。

`BackoffStrategyImpl::NextBackoff` 的决策顺序不可随意交换：

1. 用 `Errors(err)` 展开错误链，并取最后一个错误作为分类依据。
2. 将最后错误的文本交给 `HandleUnknownBackupError`，原地更新 `errContext`，让未知错误的 encounter 次数跨轮次累计。
3. 若错误处理结果要求重试，调用 `doBackoff`；若原因是上下文取消，仅在开启 `retryGRPCCanceled`、错误并非真实 context cancel 且文案表示 gRPC `Canceled` 时重试，否则停止。
4. 再依次检查不可重试函数和可重试函数；均不命中时记录 warning 并停止。
5. 本地 `inject_failpoint` 当前为空操作；随后返回当前延迟与最大延迟的较小值。

`doBackoff` 先把 `delayTime` 饱和翻倍，再将 `remainingAttempts` 减一，因此通用实现第一次返回的是“初始延迟的两倍并封顶”；这与同路径 Go 实现一致。`RetryState::ExponentialBackoff` 则先保存并返回当前值，再更新下一次值，两种状态机的首次等待语义不同。

## 数据与状态

文件没有全局可变状态。所有运行时状态都由每个策略实例独占：`RetryState` 记录累计尝试次数和下一次延迟，`BackoffStrategyImpl` 记录剩余次数、当前延迟、最大延迟及一份克隆的 `ErrorContext`。工厂每次返回新的 `Box<dyn BackoffStrategy>`，不同任务之间不会共享计数。

等待参数均为编译期 `Duration`/整数常量。公开的 `FlashbackMaxWaitInterval` 和 `ChecksumMaxWaitInterval` 在本文件工厂中没有传给 `WithMaxDelayTime`；`recoveryMaxDelayTime` 也未被工厂消费。因此三类工厂当前实际封顶均是 `NewBackoffStrategy` 的 10 秒默认值，`br/pkg/utils/backoff_test.rs::test_factory_default_max_delay_matches_go` 对此有明确断言，不能仅按常量名推断运行行为。

错误上下文会被 `HandleUnknownBackupError` 原地修改，是影响后续轮次判断的隐藏状态。错误判定函数是函数指针而非捕获环境的闭包；配置选项本身可以捕获数值或克隆 `ErrorContext`，但只在构造阶段执行。

## 依赖与调用关系

直接标准库依赖仅为 `std::time::Duration`。错误类型与 `Is`/`IsContextCanceled` 来自 `astersql-br-pkg-errors`，错误链展开和 cause 获取来自 `astersql-errors`，告警日志来自 `astersql-br-pkg-logutil`；未知错误策略、`ErrorContext` 和取消原因常量来自同 crate 的 `error_handling.rs`。这些依赖均由 `br/pkg/utils/Cargo.toml` 声明为工作区路径 crate，没有由本文件引入 feature 条件或平台条件。

直接消费关系包括：

- `br/pkg/utils/retry.rs` 以 trait object 驱动策略，是主要执行器。
- `br/pkg/backup/store.rs` 使用 backup-SST 策略保护流式备份请求。
- `br/pkg/restore/snap_client/import.rs` 使用 download/peer-download 策略保护 SST 下载，并根据 peer retry 能力切换。
- `br/pkg/restore/log_client/client.rs::getMaxReplica` 直接循环调用 aggressive PD 策略，失败耗尽后回退默认副本数 3。
- `br/pkg/conn/util/util.rs` 在获取 TiKV store、PD TSO 和 store config 时将 aggressive PD 策略交给本地可取消重试循环。

RustCodeGraph 对目标文件报告上述模块使用关系，但对带 `--file br/pkg/utils/backoff.rs` 的精确 `callers` 查询没有返回函数级边；这是索引边覆盖限制，不代表这些源码调用不存在。仓库中若干子模块另有同名 stub 策略，不能把那些局部替身误认为本文件的调用者。

## 错误处理与边界

策略不返回错误，只通过等待时长和剩余次数表达决策：`stopBackoff` 把两者置零，外层循环在下一轮退出。实际错误的聚合、最后错误返回和上下文取消由 `retry.rs` 负责。

`NextBackoff` 假定 `Errors(err)` 至少包含一个元素；代码以 `unwrap_or(err)` 保护 `last()` 的空结果，但输入本身仍必须是有效 `SharedError`。错误分类优先级为未知错误处理结果、取消特例、非重试、重试、兜底停止；调整优先级可能改变上下文取消和未知错误的行为。

gRPC code 并非通过结构化 status 类型解析，而是匹配 `Display` 文本中的 `code = X`、`code=X`、`Code(X)` 或 `rpc error: code = X`。这保持了当前瘦依赖边界，但存在误匹配或漏掉新文案格式的兼容风险。`SqlErrNoRows` 和 `IoEof` 也只是本地类型哨兵，调用者必须传递可向下转型到这些类型的错误才能命中。

`RetryState::ReduceRetry` 直接执行减一，不阻止负数；只有确信此前已增加计数的调用方才应使用。`BackoffStrategyImpl::doBackoff` 同样不单独防止剩余次数变负，正确用法依赖外层先检查 `RemainingAttempts() > 0`。延迟翻倍使用 `saturating_mul(2)`，避免 `Duration` 算术溢出。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、网络连接或计时器；trait 也没有声明 `Send`/`Sync`，因此不能假定任意策略对象可在线程间共享。通常每个重试操作独占一个可变策略，并由外层同步循环串行调用 `NextBackoff`。

真正的等待生命周期位于调用侧：`WithRetryV2` 使用 context-aware timeout，使等待可被取消；`br/pkg/restore/log_client/client.rs::getMaxReplica` 则把单次等待切成至多 10ms 的 sleep 片段并反复检查上下文。`ConstantBackoff` 自身不会观察取消，`backoff_test.rs::test_constant_backoff` 的线程退出能力来自 `WithRetryV2` 的 Context，而不是该策略内部。

`ErrorContext` 与计数随 `Box<dyn BackoffStrategy>` 生命周期存在，策略被丢弃后全部状态释放。工厂返回独立实例，不需要锁；若未来要跨线程共享，应在调用层明确加同步包装，并先决定并发请求是否应共享重试预算。

## 与 Go 版本的对应关系

Rust 文件明确移植自 `br/pkg/utils/backoff.go`，公开 trait、常量组、`RetryState`、option 构造器、场景工厂和 `NextBackoff` 决策顺序基本一一对应。Rust 的 `Box<dyn BackoffStrategy>` 对应 Go interface，`SharedError`/`Errors` 对应 Go error 与 `multierr.Errors`，本地 `SqlErrNoRows`/`IoEof` 对应 `sql.ErrNoRows`/`io.EOF`。

Go 使用 `errors.Is`、`errors.Cause` 和 `status.Code` 进行结构化分类；Rust 对 BR sentinel 仍使用 `Is`/downcast，但 gRPC code 改为错误文本匹配，这是当前最重要的实现差异。Go failpoint 会真实改变剩余次数，Rust 的 `inject_failpoint` 是空桩，因此本文件当前没有等价的 failpoint 注入能力。

两端都保留了一个容易误读的行为：recovery、flashback、checksum 虽定义了场景最大等待常量，却没有在对应工厂设置 `maxDelayTime`，所以实际使用默认 10 秒上限。Rust 独立测试专门锁定了这一 Go 兼容语义。

`br/pkg/utils/backoff_test.go` 与 `backoff_test.rs` 共同证明主要路径对齐：可重试错误最终成功、致命错误立即停止、PD 对 EOF 的处理、SST 场景调用次数、ConstantBackoff 的取消/近似无限重试。Rust 额外覆盖 peer-download 中“传输层 gRPC Canceled 可重试、真实 context cancel 停止”以及默认最大延迟。

## 扩展指南

新增业务策略时，优先组合 `NewBackoffStrategy` 和既有 option，而不是复制 `NextBackoff`。必须明确四项契约：尝试次数是否包含首次业务调用、通用实现首次等待会翻倍、最大等待是否显式设置、未知错误是否应受 `ErrorContext` 阈值影响。若新增错误类别，应同时评估白名单与黑名单的优先级，并保留真实上下文取消不可重试的约束。

修改 gRPC 分类时，应优先考虑引入结构化错误证据；如果仍使用文案匹配，至少同步 `grpc_message_has_code` 的格式测试，防止普通业务消息中包含相似文本造成误判。改变 `RetryState::ReduceRetry` 或计数算法时，要检查 `br/pkg/restore/split`、`br/pkg/restore/log_client` 等使用同类状态机或局部 stub 的模块，避免只更新 canonical 文件而留下语义分叉。

测试应继续放在独立的 `br/pkg/utils/backoff_test.rs`，并由 `br/pkg/utils/lib.rs` 的 `#[cfg(test)]` 模块声明接入，不要内嵌到生产源文件。Go 语义变更还应同步核对 `br/pkg/utils/backoff.go` 与 `backoff_test.go`。至少覆盖：首轮/封顶等待、预算耗尽、fatal 与 retry 同时可能命中时的优先级、真实取消与 gRPC Canceled 的身份差异、组合错误只看最后一项、未知错误阈值累计。

兼容风险主要来自公开工厂参数、公开常量及 trait 行为；性能风险主要是过大的默认重试预算和等待上限延长故障恢复时间，或零等待造成忙循环。新增共享/并发用法前还需确认 trait 的 `Send`/`Sync` 约束，不能从当前串行使用方式外推。

## 验证依据

- RustCodeGraph：`status` 确认索引含 7032 个 Rust 文件；`files --filter br/pkg/utils` 确认目标、Go 对照和独立测试均已索引；`node --file br/pkg/utils/backoff.rs` 读取全部 644 行并得到文件级使用方；`node` 还读取了 `br/pkg/utils/lib.rs`、`retry.rs`、`br/pkg/restore/snap_client/import.rs`、`br/pkg/restore/log_client/client.rs`、`br/pkg/backup/store.rs`、`br/pkg/conn/util/util.rs` 的直接接线片段。
- RustCodeGraph 对 `NewImportSSTBackoffStrategy` 等符号的 `query` 能区分 Go/Rust 定义；带目标文件限定的 `callers` 查询为空，因此调用关系以文件级图结果和上述真实源码接线交叉验证，未虚构函数级图边。
- crate/入口：`br/pkg/utils/Cargo.toml`、`br/pkg/utils/lib.rs`；目标 package 未发现 `doc.go`。
- Go 对照：`br/pkg/utils/backoff.go`、`br/pkg/utils/backoff_test.go`。
- Rust 测试：`br/pkg/utils/backoff_test.rs`，覆盖默认封顶、成功/未知/致命/可重试路径、PD、三类 SST、ConstantBackoff 与 peer-download 取消身份。
- 生产调用证据：`br/pkg/utils/retry.rs`、`br/pkg/backup/store.rs`、`br/pkg/restore/snap_client/import.rs`、`br/pkg/restore/log_client/client.rs`、`br/pkg/conn/util/util.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前只执行任务规定的 11 章节结构检查以及文档/变更范围检查。
