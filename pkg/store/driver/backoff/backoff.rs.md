# `pkg/store/driver/backoff/backoff.rs`

## 文件定位

`backoff.rs` 是 `astersql-store-driver-backoff` crate 的主要业务实现，由同目录 `lib.rs` 的 `mod backoff; pub use backoff::*;` 对外重导出。它位于 SQL 层与 TiKV 客户端之间的 driver 适配边界：利用上游 `tikv_client::Backoff` 产生单次延迟，在外层补齐 TiDB/client-go 所需的会话变量、总睡眠限额、错误归一化、取消和统计语义（`TiKvBackoffer::backoff`、`Backoffer::Backoff`）。

crate 边界由 `pkg/store/driver/backoff/Cargo.toml` 确认：内部依赖 `astersql-store-driver-error` 和 `astersql-kv`，外部延迟生成器固定为 `astersql/client-rust` 标签 `v0.4.2-aster.10` 中的 `tikv-client = 0.4.2`，取消原语来自 `tokio-util`。本文件不是单独的重试循环；调用者在可重试错误出现时调用它进行一次退避，然后自行重试业务操作。

## 核心职责

- 将退避类型抽象为 `BackoffConfig`，根据 `Jitter` 选择 no/full/equal/decorrelated jitter，并为已知的 client-go 配置名映射标准 driver 错误（`BackoffConfig::schedule`、`default_config_error`）。
- 在 `TiKvBackoffer` 中按配置名复用独立的延迟序列，维护总睡眠时间、分类次数/时长、最近错误和特殊排除时间（`schedules`、`backoff_times`、`backoff_sleep_ms`、`errors`、`excluded_sleep_ms`）。
- 实现 client-go 兼容的停止条件：`Context` 取消、`kv::Variables::Killed` 中断、普通总睡眠上限，以及 `tikvServerBusy` 的有界排除额度。
- 通过 `Backoffer` 保留 Go 包的公开门面，并用 `driver_error::ToTiDBErr` 将 TiKV/driver 错误转换为 TiDB 可见错误。

## 主要符号

- `ExecDetails`：两个 `AtomicI64` 分别累加退避纳秒和次数。`record_backoff` 对毫秒转纳秒使用饱和算术，公开 getter 使用 `Ordering::Relaxed`。
- `Context`：可克隆取消上下文。`CancellationToken` 在克隆间共享取消状态；`identity: Arc<()>` 让 `PartialEq` 比较上下文身份，而不是当前字段值；`with_exec_details` 可选附加共享统计。
- `Jitter`：`NoJitter`、`FullJitter`、`EqualJitter`、`DecorrJitter` 四种策略枚举。
- `BackoffConfig`：包含策略名、base/cap 毫秒、jitter 和可选的超限错误。`new` 按策略名填充默认错误，`with_error` 允许覆盖，`schedule` 将 base 至少夹到 2ms，并用 `UNLIMITED_ATTEMPTS` 将终止权留给外层总限额。
- `BackoffError`：记录触发错误的文本和 `SystemTime`；`TiKvBackoffer::append_error/latest_errors` 以长度 3 的环形缓冲按从旧到新返回最近错误。
- `Variables<'a>`：在借用调用者 `kv::Variables` 和持有默认静态变量之间统一访问，使 `TiKvBackoffer<'a>` 不必复制会话中的 `Killed` 原子标志。
- `TiKvBackoffer<'a>`：核心可变状态机。`from_parts/new/new_with_vars`、`sleep`、`backoff`、`longest_sleep_config` 分别负责构造、可取消睡眠、一次退避和超限错误选择。
- `Backoffer<'a>` 及 `NewBackofferWithVars`、`NewBackofferWithTikvBo`、`NewBackoffer`：TiDB 一侧的公开包装与三种构造路径。`Backoff`、`BackoffWithMaxSleepTxnLockFast`、`GetBackoffTimes`、`GetCtx`、`GetVars`、`GetBackoffSleepMS`、`GetTotalSleep` 保留 Go 命名和查询语义。

## 执行流程

1. 调用者通过三个 `NewBackoffer*` 构造函数获得 `Backoffer`。`TiKvBackoffer::from_parts` 选择借用会话变量或默认变量；仅 `new_with_vars` 路径在正数上限且乘法不越界时用 `BackOffWeight` 放大 `max_sleep_ms`。
2. 业务重试分支传入 `BackoffConfig` 和本次触发错误，调用 `Backoffer::Backoff`；快速锁等待则调用 `BackoffWithMaxSleepTxnLockFast`，它固定构造 equal-jitter 的 `txnLockFast` 配置并另外截断单次睡眠。
3. `TiKvBackoffer::backoff` 先检查 Context 取消；再以 `total_sleep_ms - excluded_sleep_ms` 检查普通限额，并单独限制排除类型。超限时优先返回累计睡眠最长的非排除配置的标准错误，否则回退到当前触发错误。
4. 未超限时，先记录错误和配置。配置名以 `HashMap` key 区分延迟序列；但 `txnLockFast` 的 base 匹配不区分 ASCII 大小写，并取 `Variables::BackoffLockFast` 作为 override。
5. 从 `tikv_client::Backoff` 取下一个 `Duration`，必要时用单次 `max_sleep_ms` 截断。`sleep` 以 5ms 片段执行同步 `std::thread::sleep`，每片之间检查取消；取消时返回 0 作为实际睡眠值。
6. 睡眠后以饱和加法更新总时间、可选排除时间、分类时间/次数和 `ExecDetails`。最后读取 `Killed`：非零则返回带 signal 的 `QueryInterruptedWithSignal`，否则本次退避成功。
7. `Backoffer` 将内层错误经 `ToTiDBErr` 归一化后交还调用者；成功只表示该次等待完成，并不代表原业务操作成功。

## 数据与状态

`TiKvBackoffer` 的状态是单个重试上下文的累计账本。`max_sleep_ms` 是有效总预算，`total_sleep_ms` 包含所有类型，`excluded_sleep_ms` 当前只累计精确名称 `tikvServerBusy`。因此普通超限判定使用两者之差，但 server-busy 仍受 600,000ms 排除上限与总预算的联合条件限制（`excluded_sleep_limit`）。

`schedules` 保留每个精确配置名的 jitter 内部进度；`configs` 保留历次配置克隆，供超限时根据 `backoff_sleep_ms` 找回错误。`errors` 是固定三格环，`errors_num` 是总写入次数。两个分类 `HashMap` 返回时会克隆，调用者修改返回 map 不会改变 backoffer 内部状态。

整数时间和次数采用 `isize`，与当前 64 位平台的 Go `int` 对齐；时间累加使用饱和算术。对配置 base/cap 的负值会分别被夹到 2/0 再转为 `u64`，不会发生符号翻转。

## 依赖与调用关系

下游依赖分为四类：`tikv_client::Backoff` 计算 jitter 延迟；`kv::Variables` 提供 `BackoffLockFast`、`BackOffWeight`、`Killed`；`driver_error`/`SharedError` 承载类型化错误与 TiDB 转换；`CancellationToken` 传播取消。文件内部的主调用边是 `Backoffer::Backoff -> TiKvBackoffer::backoff -> BackoffConfig::schedule/tikv_client::Backoff::next_delay_duration + TiKvBackoffer::sleep`，成功后再更新 `ExecDetails::record_backoff`。

RustCodeGraph 对当前 Rust 树的精确查询只识别到 `NewBackofferWithVars` 被本文件和 `backoff_test.rs` 调用，`BackoffWithMaxSleepTxnLockFast`/`GetTotalSleep` 也未显示生产 Rust 上游。因此只能确认 crate API 已实现且有独立 Rust 测试覆盖，不应据此宣称 Rust 生产主链已全面接线。对照的 Go API 已在 `pkg/store/copr/coprocessor.go`、`batch_coprocessor.go`、`mpp.go`、`pkg/store/helper/helper.go` 和 `pkg/store/gcworker/gc_worker_test.go` 等位置被调用；其中 coprocessor 锁解决路径使用 `BackoffWithMaxSleepTxnLockFast`，运行统计路径读取 `GetBackoffSleepMS`。

## 错误处理与边界

- Context 在进入退避前已取消时，直接返回调用者传入的触发错误，不写入次数、时长或错误环。睡眠中取消则使本次记账时长为 0，但本方法随后仍会记录一次退避并进行 `Killed` 检查；它没有在睡眠返回后再为 Context 取消单独返错。
- 总限额在下一次 `backoff` 开头检查，因此某次睡眠可以使累计值达到或超过上限，当次仍成功，下次才返错。`max_sleep_ms <= 0` 会跳过总限额终止判定。
- 超限错误只从非排除类型中选累计睡眠最长的配置；未知配置名的 `default_config_error` 为 `None`，若未通过 `with_error` 填充，超限时可回退到当前触发错误。
- `default_config_error` 仅对列出的精确名称映射错误；只有 `txnLockFast` base override 是不区分 ASCII 大小写的。`tikvServerBusy` 的排除判定也是大小写敏感的。
- `Backoffer::Backoff*` 对 `ToTiDBErr(Some(error))` 的 `None` 使用 `expect`；这里依赖“非空输入错误不会被转换为空”的 driver-error 契约。

## 并发与资源生命周期

`Backoffer`/`TiKvBackoffer` 本身依赖 `&mut self` 串行推进 schedule 与账本，文件没有内部任务、通道或锁，也没有实现异步 sleep。因而 `Backoff` 会阻塞当前 OS 线程；在 async executor 上使用时必须考虑不要占用关键 runtime worker。

`Context` 的 `CancellationToken` 和可选 `Arc<ExecDetails>` 在克隆间共享，适合另一线程取消或读统计。`ExecDetails` 与 `Killed` 均使用 relaxed 原子操作：它们只承载计数/中断信号，不为其他数据建立跨线程 happens-before 关系。借用的 `Variables<'a>` 将 backoffer 生命周期绑定到外部会话变量；默认路径则引用进程级 `DEFAULT_KILLED`。所有 HashMap、错误环和 schedule 都由单个 `TiKvBackoffer` 拥有，随其 drop 释放。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/store/driver/backoff/backoff.go`。两者的外层 API 一一对应：Go `Backoffer` 包装 `*tikv.Backoffer`，Rust `Backoffer<'a>` 包装本 crate 的 `TiKvBackoffer<'a>`；三个构造函数、`TiKVBackoffer`、两个 backoff 方法以及五个查询方法保留相同的职责与 Go 风格名称。两个 `Backoff*` 方法都在边界调用 `derr::ToTiDBErr`。

关键差异是 Go 文件将延迟、Context、会话变量、限额与统计全部委托给 `github.com/tikv/client-go/v2/tikv.Backoffer`；Rust 上游 `tikv_client::Backoff` 只提供延迟序列，所以本文件在 `TiKvBackoffer` 中本地实现了其余语义。Rust `Context` 不是通用 key/value context，只有取消、身份和可选 `ExecDetails`；它是为此同步兼容 API 设计的精简表示。Rust 还将 Go `int` 表示为 `isize`，并显式用饱和算术防止记账溢出。

`migration_aster_unit_test.rs` 针对上述迁移语义验证构造、weight、记账、超限错误、单次截断、Killed/取消、三条错误环、server-busy 排除和 `ExecDetails`；`backoff_test.rs` 额外验证 `txnLockFast` base override 的大小写不敏感。同目录没有 Go `*_test.go`，因此 Go 语义的直接依据是包装实现和上游调用点，而不是包内 Go 单元测试。

## 扩展指南

- 新增退避类型时，首先在调用端构造 `BackoffConfig`；若需要稳定的 TiDB 超限错误，同步扩展 `default_config_error` 或显式调用 `with_error`，并在独立的 `backoff_test.rs`/`migration_aster_unit_test.rs` 增加测试，不把测试嵌入生产文件。
- 新增“不消耗普通预算”的类型需修改 `excluded_sleep_limit`，并复核 `longest_sleep_config` 的过滤、独立上限和错误选择是否与 client-go 一致；这会直接改变故障期间的延迟与流量压力，属于性能和可用性敏感变更。
- 修改 jitter/base/cap 时应保留 `schedule` 中 client-go 的最小 base 约束，并注意 schedule 按精确字符串名称缓存。若要普遍改为大小写不敏感，必须同时定义统计 map 的 key 兼容性。
- 如果要用 async sleep 替代当前的同步 5ms 分片，会改变公开方法形态和所有调用者，不应只替换 `sleep`。需要同时验证取消时记账、Killed 检查时机及 driver 错误归一化。
- 新增生产 Rust 调用者时，应将构造与调用放在对应重试循环，确保每次失败只推进一次 backoffer，并消费 `Result` 来终止循环。同步为调用链添加独立集成/单元测试，不能仅依赖本 crate 的状态机测试。

## 验证依据

- 源码与模块：`pkg/store/driver/backoff/backoff.rs` 的 `ExecDetails`、`Context`、`BackoffConfig`、`TiKvBackoffer`、`Backoffer` 及其 impl；`pkg/store/driver/backoff/lib.rs` 的模块声明和重导出。
- crate 边界：`pkg/store/driver/backoff/Cargo.toml` 中的 package、path 依赖、固定 tag 的 `tikv-client` 与 `tokio-util`。
- RustCodeGraph：`status`、`files --filter pkg/store/driver/backoff`、`node --file pkg/store/driver/backoff/backoff.rs --offset 1 --limit 500`、`node --file ... --offset 495 --limit 40`、`query NewBackofferWithVars --kind function`、`callers NewBackofferWithVars`、`callees backoff`，以及包含 `NewBackofferWithVars BackoffWithMaxSleepTxnLockFast GetTotalSleep callers callees` 的精确 `explore` 查询。图显示文件被 crate 广泛引用，但所查公开符号在 Rust 生产链的静态调用证据不足，已在“依赖与调用关系”中作限定。
- Go 对照与调用：`pkg/store/driver/backoff/backoff.go`；用 `rg` 核对了 `pkg/store/copr/coprocessor.go`、`batch_coprocessor.go`、`mpp.go`、`region_cache.go`、`pkg/store/helper/helper.go` 等调用点。
- 独立测试：`pkg/store/driver/backoff/backoff_test.rs` 和 `pkg/store/driver/backoff/migration_aster_unit_test.rs`。本件为纯文档任务，按任务约束不运行 Cargo；测试文件仅用于核对边界、不变量和已有验证意图。
