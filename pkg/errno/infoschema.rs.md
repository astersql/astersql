# `pkg/errno/infoschema.rs`

源文件：[`pkg/errno/infoschema.rs`](./infoschema.rs)

## 文件定位

本文件属于 `astersql-errno` crate。crate 入口 `pkg/errno/lib.rs` 以 `pub mod infoschema` 公开该模块；`pkg/errno/Cargo.toml` 指定 `lib.rs` 为库入口，并且本文件本身只依赖 Rust 标准库。它实现的是单个 AsterSQL/TiDB 进程内的客户端错误与警告聚合核心：以错误码为键，同时维护全局、用户、远端主机三个维度的计数和首次/末次出现时间。

Go 版本已经把同类状态接入网络连接响应、`INFORMATION_SCHEMA.CLIENT_ERRORS_SUMMARY_*` 表和 `FLUSH CLIENT_ERRORS_SUMMARY`。截至本次检索，Rust 生产源码中未找到对本模块公开统计 API 的调用，直接调用者仅为 `pkg/errno/infoschema_test.rs` 和 `pkg/errno/errname_2_aster_unit_test.rs`；因此它当前是已移植并经过独立测试的统计核心，不能据此宣称 Rust 服务主链已经完成接线。

## 核心职责

- `IncrementError` 与 `IncrementWarning` 把一次事件同步写入 global、user、host 三份聚合，不按会话保存明细。
- `GlobalStats`、`UserStats`、`HostStats` 在持锁期间复制容器，向调用方返回与后续写入隔离的快照。
- `FlushStats` 一次替换整个 `InstanceStatistics`，清空三个维度。
- `statistics` 用 `OnceLock<Mutex<_>>` 延迟建立进程级唯一状态；`lock_statistics` 统一处理加锁与 mutex poison 恢复。
- `incrementWithClock` 把时钟作为闭包注入，仅用于验证 Go 的两次取时顺序；正常入口通过 `increment` 传入 `SystemTime::now`。

该模块不负责把错误对象转换成 `u16`、提取登录用户/远端地址、授权、错误消息查表或生成 information-schema 行；这些职责在 Go 主链中分别位于 `pkg/server/conn.go`、`pkg/executor/infoschema_reader.go` 等文件，Rust 侧生产接线当前未发现。

## 主要符号

- `pub struct ErrorSummary`：一个错误码在一个聚合维度中的值。`ErrorCount`、`WarningCount` 为 `i32`；`FirstSeen`、`LastSeen` 为 `SystemTime`。`Default` 把计数置零、时间置为 `UNIX_EPOCH`。
- `struct InstanceStatistics`：模块内部容器；`global` 是 `HashMap<u16, Box<ErrorSummary>>`，`users` 和 `hosts` 是字符串到上述 map 的二级映射。该类型不公开，避免绕过模块同步规则。
- `fn statistics() -> &'static Mutex<InstanceStatistics>`：通过函数内静态 `STATS: OnceLock<_>` 延迟初始化全局状态。
- `fn lock_statistics() -> MutexGuard<'static, InstanceStatistics>`：取得全局锁；若先前持锁线程 panic 导致锁毒化，则用 `PoisonError::into_inner` 继续访问已有状态。
- `pub fn FlushStats()`：以默认空容器替换全部状态。
- `fn copyMap(...)`：克隆单层错误码 map；由于 `ErrorSummary: Clone` 且值装在 `Box` 中，克隆结果不共享汇总对象。
- `pub fn GlobalStats/UserStats/HostStats()`：分别返回三类深拷贝快照。后二者对外层、内层 map 及 boxed summary 一并克隆。
- `fn summary(first_seen)`：构造计数为零、`FirstSeen` 为指定采样、`LastSeen` 仍为 epoch 的新条目。
- `fn initCounters(errCode, user, host, seen)`：一次持锁，确保同一错误码在三个维度都存在；已有条目不会重置首次时间。
- `pub(crate) fn incrementWithClock(...)`：完整增量实现；crate 内可见是为了独立测试注入确定性时钟。
- `fn increment(...)`：正常时钟适配层。
- `pub fn IncrementError/IncrementWarning(...)`：公开写入口；用户和主机接受 `impl AsRef<str>`，最终以拥有所有权的 `String` 作为聚合键。

文件没有 trait、enum、条件编译项或后台任务。

## 执行流程

一次 `IncrementError(code, user, host)` 或 `IncrementWarning(...)` 的路径如下：

1. 公开入口把 `user`、`host` 转为 `&str`，并用布尔值区分 error 与 warning，调用 `increment`。
2. `increment` 将 `SystemTime::now` 交给 `incrementWithClock`。
3. `incrementWithClock` 先采样 `seen`，它稍后写入三个条目的 `LastSeen`。
4. 随后再次采样时间并调用 `initCounters`。后者在一个临界区内按需建立 global、指定 user、指定 host 的条目；三个新条目的 `FirstSeen` 使用第二次采样。
5. 初始化锁释放后，函数重新取得全局锁。局部 `update` 对三个已存在条目分别增加 `ErrorCount` 或 `WarningCount`，并把相同的第一次采样写为 `LastSeen`。

所以首次事件有意保留 Go 的取时语义：`LastSeen` 的采样可能早于 `FirstSeen`。`pkg/errno/infoschema_test.rs::first_increment_uses_go_timestamp_sampling_order` 用 epoch 后 1 秒、2 秒的确定值验证这一点。

读取路径更短：三个 `*Stats` 函数取得同一把锁、克隆所需 map、释放锁并返回快照。`FlushStats` 同样持有该锁，整体替换容器，不逐项清除。

## 数据与状态

状态的生命周期与进程相同：首次调用 `statistics` 时创建，此后没有销毁或持久化。键空间是遇到过的错误码、完整用户名字符串和完整主机字符串；模块没有容量上限、淘汰、归一化或跨节点合并。因此内存规模随不同用户、主机和错误码组合增长，直到 `FlushStats` 或进程退出。

每次事件在三个维度各增加一次，而且三个条目的 `LastSeen` 使用同一份 `SystemTime`。`FirstSeen` 只在相应 key 第一次插入时设置；对已有条目的后续增量不会改变它。用户或主机首次出现时先创建空的内层 map，再创建错误码条目。

快照拥有独立的 `HashMap`、`String`、`Box<ErrorSummary>`，因此获取快照后的写入和 flush 不会改变旧快照。返回类型没有排序保证；需要稳定展示的上层必须自行排序。计数使用 `i32`，实现采用普通 `+= 1`，没有溢出检测策略、饱和或重置阈值。

## 依赖与调用关系

下游仅为标准库：`HashMap` 存储聚合，`OnceLock` 延迟初始化，`Mutex/MutexGuard` 串行访问，`SystemTime/UNIX_EPOCH` 表示时间。`pkg/errno/Cargo.toml` 的 `astersql-parser-mysql` 依赖服务于同 crate 其他模块，本文件没有引用它；dev dependency `astersql-testkit-testsetup` 也不在本文件直接使用。

模块内调用边为：`IncrementError/IncrementWarning -> increment -> incrementWithClock -> initCounters -> lock_statistics -> statistics`；`incrementWithClock` 还调用局部 `update`；三个读取 API 经 `lock_statistics` 访问状态，其中 `GlobalStats` 再调用 `copyMap`；`FlushStats` 直接经 `lock_statistics` 替换状态。

RustCodeGraph 将 `pkg/errno/infoschema.rs` 标为 18 个符号。进一步的精确 callers/callees 命令没有产生可用文本，局部 Rust 文本检索确认生产源码没有本模块 API 的调用：直接上游为 `pkg/errno/infoschema_test.rs` 和 `pkg/errno/errname_2_aster_unit_test.rs`。crate 被多个 Rust crate 依赖，但当前检索到的生产引用使用的是 `errcode` 或 `errname`，这不能作为 infoschema 统计已接线的证据。

作为设计对照，Go 主链是：`pkg/server/conn.go::clientConn.flush` 收集 warning，`clientConn.writeError` 收集 error；`pkg/executor/infoschema_reader.go::setDataForClientErrorsSummary` 读取三个快照并执行 PROCESS 权限过滤；`pkg/executor/simple.go` 在 `FlushClientErrorsSummary` 分支清空状态。它们是 Rust 后续接线的参照，而不是当前 Rust 调用关系。

## 错误处理与边界

公开 API 不返回 `Result`。正常路径中唯一被显式恢复的异常状态是 mutex poison：`lock_statistics` 保留并继续使用毒化锁中的数据，优先保证后续测试或服务读取可以继续，但不会检查数据是否因先前 panic 处于业务一致状态。

`incrementWithClock` 在第二次加锁后用 `expect` 取得三个刚初始化的条目。通常 `initCounters` 已保证它们存在；但初始化和增量分属两个临界区，如果另一线程恰在两者之间调用 `FlushStats`，这些 `expect` 会 panic。这一竞态也意味着其他读取者可能短暂看到计数为零、只有 `FirstSeen` 的新条目。现有测试覆盖并发增量不丢计数，却没有覆盖与 flush 并发；扩展时不能假设“单次增量对 flush/快照原子”。

注入时钟闭包必须至少返回两个值；测试闭包不足时会由闭包自身 panic。系统时钟可能回拨，再加上首次事件刻意使用两次采样，模块不保证 `FirstSeen <= LastSeen`。`u16` 错误码、用户和主机字符串不做合法性校验；空字符串也是合法聚合键。普通整数加法在计数达到边界后的行为受 Rust overflow 配置影响，模块没有定义业务级溢出语义。

## 并发与资源生命周期

所有生产统计共享一把进程级 `Mutex<InstanceStatistics>`。同一锁覆盖三维 map，因此每个初始化临界区、每个三维计数更新临界区、每次快照复制和每次 flush 内部都是互斥的；不存在异步任务、channel、事务、文件句柄或网络资源。锁内进行完整快照 clone，用户/主机/错误码基数越高，持锁时间和复制内存越大。

单次增量会依次获取两次同一 mutex：一次初始化、一次更新。它不会嵌套持锁，因此没有本文件内部的锁顺序死锁；代价是前述 flush 竞态和两次锁开销。并发增量共享第二个临界区，`pkg/errno/errname_2_aster_unit_test.rs::concurrent_increments_are_not_lost` 以 8 个线程、每线程 250 次 error 验证 global/user/host 均得到 2000。

测试自身还用独立的 `STATS_TEST_LOCK` 串行化会修改全局统计的测试。这把锁只存在于测试模块，不保护生产调用者；新增测试如果读写全局统计，应复用 `lock_stats_test` 并在开始、结束时 flush，避免并行测试互相污染。

## 与 Go 版本的对应关系

Rust 的 `ErrorSummary`、三层 map、公开函数命名和总体流程直接对应 `pkg/errno/infoschema.go`。主要一致点包括：单实例共享状态；全局/用户/主机三维同步计数；读取返回深拷贝；flush 同时清空三维；首次增量先采样将写入 `LastSeen` 的时间，再在初始化函数中采样 `FirstSeen`。Rust 特意用 `incrementWithClock` 固化了最后一点，即使它会产生首次 `FirstSeen` 晚于 `LastSeen` 的结果。

实现层差异包括：Go 把 mutex 嵌入 `instanceStatistics` 并在包初始化时调用 `FlushStats`，Rust 用 `OnceLock<Mutex<_>>` 延迟获得等价的空默认状态；Go 计数为平台宽度 `int`，Rust 为固定 `i32`；Go 的快照逐项构造，Rust 借助 `Clone` 深拷贝；Rust 对 poison 做恢复，而 Go mutex 没有 poison 概念；Rust 公开入口接受 `impl AsRef<str>`，Go 接受 `string`。

测试对应关系：`pkg/errno/infoschema_test.rs::test_copy_safety` 移植 `pkg/errno/infoschema_test.go::TestCopySafety`，并通过公开快照读取 live 状态；Rust 还增加确定性取时测试，以及 `pkg/errno/errname_2_aster_unit_test.rs` 中的 flush 与多线程覆盖。Go 的 `pkg/infoschema/test/clustertablestest/tables_test.go::TestInfoSchemaClientErrors` 进一步验证权限与 SQL 表行为，但 Rust 当前没有与其等价的生产接线证据。

## 扩展指南

- 接入 Rust 服务主链时，写侧应在生成最终客户端错误包和收集语句 warning 的边界调用两个增量入口，并从已认证会话取得稳定的 user/host；读侧应在 information-schema reader 中消费快照，同时复刻 Go 的 PROCESS/本人可见规则；flush 还需复刻 RELOAD 权限。不要把这些策略塞进本统计容器。
- 新增聚合维度时，应同时修改 `InstanceStatistics`、`initCounters`、`incrementWithClock`、`FlushStats` 和对应快照 API，并在独立测试文件增加初始化、增量、深拷贝、flush、并发覆盖；Rust 测试逻辑应继续对齐 Go，而不是内嵌到生产文件。
- 若要消除 flush 竞态，最小安全方向是把“按需初始化 + 三维更新”合并到同一个锁临界区，同时保留两次时钟采样顺序；任何调整都应增加“increment 与 flush 并发”回归测试，并明确 flush 的线性化语义。
- 若要改变计数类型或溢出策略，必须核对 information-schema 列类型和 Go 兼容语义；不要只为测试方便采用饱和/回绕。若要限制内存，需定义淘汰后 `FirstSeen`、快照一致性及 SQL 可见行为。
- 优化快照成本时不能返回内部引用或共享可变 summary；可评估分片锁或不可变快照，但必须继续保证调用方无法修改 live 状态，并用高基数和并发测试验证一致性与锁竞争。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/errno` 确认目标、Go 对照和测试均已索引；`node --file pkg/errno/infoschema.rs --offset 1 --limit 260` 读取完整 204 行并报告 18 个符号；`explore 'pkg/errno/infoschema.rs symbols responsibilities callers callees'` 给出本文件内部调用与测试调用线索。精确 `callers/callees` 未返回可用文本，因此按技能规则以局部 `rg` 补查。
- Rust 源与边界：`pkg/errno/infoschema.rs`、`pkg/errno/lib.rs`、`pkg/errno/Cargo.toml`。
- Rust 独立测试：`pkg/errno/infoschema_test.rs` 覆盖两次时钟采样及深拷贝；`pkg/errno/errname_2_aster_unit_test.rs` 覆盖深拷贝、三维 flush 与并发不丢计数。
- Go 对照：`pkg/errno/infoschema.go`、`pkg/errno/infoschema_test.go`；生产主链证据为 `pkg/server/conn.go::clientConn.flush/writeError`、`pkg/executor/infoschema_reader.go::setDataForClientErrorsSummary`、`pkg/executor/simple.go` 的 `FlushClientErrorsSummary` 分支；SQL 权限与可见性证据为 `pkg/infoschema/test/clustertablestest/tables_test.go::TestInfoSchemaClientErrors`。
- 局部调用检索：在全部 `*.rs` 中检索七个公开/测试入口，只发现目标文件和两个 errno 独立测试；对 Cargo manifests 与 Rust 源检索 `astersql-errno`/`astersql_errno`，确认其他 crate 依赖主要引用 `errcode`、`errname`，未发现 infoschema 统计生产调用。
- 本任务为纯文档分析，按任务约束未运行 Cargo。交付前另运行固定的 11 章节结构命令，并人工核对本文明确区分 Rust 当前事实与 Go 目标链路。
