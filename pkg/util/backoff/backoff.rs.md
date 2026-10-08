# `pkg/util/backoff/backoff.rs`

## 文件定位

本文件是 `astersql-util-backoff` crate 的核心实现，提供一个只负责“计算下一次等待时长”的无抖动指数退避策略。crate 根 `pkg/util/backoff/lib.rs` 通过 `pub mod backoff` 声明模块并用 `pub use backoff::*` 重新导出这里的公开符号；`pkg/util/backoff/Cargo.toml` 将库入口指定为 `lib.rs`，并用 `package.metadata.porting.go-package = "pkg/util/backoff"` 记录 Go 来源。

该 crate 已列入根 workspace，且被 `pkg/ddl`、`pkg/dxf/importinto`、`pkg/dxf/importinto/conflictedkv`、`pkg/dxf/framework/{handle,scheduler,taskexecutor}` 的 Cargo manifest 声明为依赖。不过当前生产 Rust 源码没有引用 `astersql_util_backoff` 或调用 `NewExponential`；Rust 侧可确认的直接使用者只有本 crate 的独立测试。因而它目前是已打包、已验证 API，但尚不能据此宣称已进入 Rust 应用运行主链。Go 对应实现则已用于 session、DDL 和 DXF/importinto 等重试路径。

## 核心职责

- `Backoffer` trait 定义按从 0 开始的重试序号取得等待时长的统一接口。
- `Exponential` 保存基础时长、增长倍率、上限和当前状态，实现无随机抖动的指数增长。
- `NewExponential` 建立一次操作专用的有状态退避器，初始下一次时长等于基础时长。
- `Exponential::Backoff` 保留 Go 风格的固有方法入口，并转发给 trait 实现，调用者无需显式导入 trait 即可使用。

本文件不执行 sleep、不调度重试、不判断错误是否可重试，也不记录累计等待时间；这些职责属于未来的上层重试循环。它只根据调用顺序更新并返回 `Duration`。

## 主要符号

- `pub trait Backoffer`：公开策略接口。`fn Backoff(&mut self, retryCnt: usize) -> Duration` 需要可变借用，表明求值会推进内部状态；`retryCnt` 仅用于识别是否为第 0 次调用。
- `pub struct Exponential`：公开状态类型。四个字段均为 `pub`：`baseBackoff` 是复位起点，`multiplier` 是浮点倍率，`maxBackoff` 是增长上限，`nextBackoff` 是当前将返回或继续增长的状态。
- `pub fn NewExponential(baseBackoff, multiplier, maxBackoff) -> Exponential`：公开构造函数，直接保存三个配置，并令 `nextBackoff = baseBackoff`。返回值是 Rust 所有权值，不是 Go 版本的指针。
- `impl Backoffer for Exponential` 中的 `Backoff`：实际状态机实现。第 0 次复位；非 0 次先按纳秒计算倍率，再与上限取较小值。
- `impl Exponential::Backoff`：公开固有方法，唯一行为是调用 `<Self as Backoffer>::Backoff(self, retryCnt)`，避免复制算法。

文件没有模块级常量、枚举、条件编译项或错误类型。

## 执行流程

1. 调用者用 `NewExponential` 传入基础时长、倍率和最大时长；构造后 `nextBackoff` 等于 `baseBackoff`。
2. 调用 `Backoff(0)` 时，方法无条件把 `nextBackoff` 重置为 `baseBackoff` 并返回它。该规则允许复用同一实例启动新一轮序列。
3. 调用 `Backoff(n)`（任意 `n != 0`）时，方法读取当前 `nextBackoff` 的纳秒数，转成 `f64` 与 `multiplier` 相乘，再以 `as u64` 转回整数纳秒。
4. 用 `Duration::from_nanos` 重建时长，随后执行 `scaled.min(maxBackoff)`；结果写回 `nextBackoff` 并返回。
5. 后续非 0 调用基于上一次写回值继续增长。算法不校验重试序号是否连续，因此 `Backoff(5)` 与 `Backoff(1)` 在相同当前状态下都只推进一次，而不是一次性计算五级增长。

典型配置 `base=1ns, multiplier=2, max=10ns` 产生 `1, 2, 4, 8, 10, 10...`；此序列由 `backoff_test.rs::test_exponential` 和 `migration_aster_unit_test.rs::migration_exponential_matches_go_sequence_and_cap` 固定。

## 数据与状态

`Exponential` 的配置和运行状态位于同一结构中。核心不变量是在通常的正倍率、`baseBackoff <= maxBackoff` 配置下，第一次返回基础值，之后 `nextBackoff` 不超过上限；到达上限后，正倍率不小于 1 时保持封顶。`migration_retry_zero_resets_a_reused_backoffer` 证明状态推进后再次传入 0 会恢复基础值。

实现按整数纳秒对齐 Go：非 0 分支显式使用 `as_nanos -> f64 -> u64 -> Duration::from_nanos`，从而让 1.5 倍等结果截断到整数纳秒；动态 trait 测试验证 `2ns -> 3ns -> 4ns`。这里保存的是单实例状态，不存在全局变量、缓存、计数器或持久化数据。

由于字段全部公开，外部 Rust 代码可以绕过构造函数修改配置或 `nextBackoff`。这属于当前 API 事实；依赖上述不变量的上层逻辑应使用构造函数并避免中途直接改字段。

## 依赖与调用关系

下游依赖只有 Rust 标准库 `std::time::Duration`：构造、取纳秒、从纳秒恢复以及 `Duration::min` 均不涉及外部 crate。`Backoff` 的固有方法调用同类型的 trait 实现，这是文件内唯一明确的函数调用边。

上游模块边界为 `pkg/util/backoff/lib.rs -> backoff.rs`，随后由 crate 根重新导出。RustCodeGraph 将目标文件识别为 7 个符号，并显示相关文件包括 Go/Rust retry 与 DDL 文件；但精确调用查询受仓库内大量同名 `Backoff` 符号干扰。因此又以精确文本检索复核：生产 Rust 中不存在 `NewExponential` 或 `astersql_util_backoff` 的调用，直接 Rust 调用仅来自 `backoff_test.rs` 与 `migration_aster_unit_test.rs`。Cargo 依赖声明代表可用接线，不等同于运行时调用。

Go 侧的直接调用可见于 `pkg/session/session.go`、`pkg/ddl/index.go`、`pkg/ddl/backfilling_dist_scheduler.go`、`pkg/dxf/importinto/scheduler.go`、`pkg/dxf/importinto/conflictedkv/deleter.go` 及 DXF framework 的 scheduler/taskexecutor 等文件。这些路径说明该通用策略在 Go 应用中的预期位置，但不是 Rust 已接线的证据。

## 错误处理与边界

API 返回 `Duration` 而非 `Result`，没有显式错误传播。调用者必须自行保证配置有业务意义：

- `retryCnt == 0` 返回 `baseBackoff`，不会与 `maxBackoff` 取最小值；因此若基础值大于上限，首个结果仍会超过上限，直到下一次非 0 调用才执行封顶。
- 任意非 0 重试序号都只增长一次；跳号、重复编号和乱序不会被拒绝。
- 零倍率会在非 0 调用时把状态降为零；小于 1 的正倍率会衰减，代码并未强制“指数增长”。
- Rust `Duration` 不能表达 Go `time.Duration` 可表达的负值，所以负基础时长或负上限没有直接对应输入。负数、NaN、无穷大倍率以及超过 `u64` 纳秒范围的结果没有契约测试，不应把其转换结果当作稳定业务语义。
- `as_nanos()` 返回 `u128`，随后转为 `f64`；很大时长可能损失整数精度，最终又收窄为 `u64`。当前测试只覆盖很小的纳秒值。

本实现无 I/O、无可恢复错误，也没有主动 panic 分支。安全扩展时应优先在构造边界增加经 Go 行为验证的约束，而不是静默改变现有序列。

## 并发与资源生命周期

`Exponential` 只拥有按值存储的 `Duration` 和 `f64`，没有文件、网络连接、锁、通道、后台任务或显式清理过程。实例随 Rust 所有权移动，并在离开作用域时普通释放。

`Backoff` 要求 `&mut self`，所以同一实例的状态推进天然需要独占可变访问；文件本身不提供跨线程共享或同步。若上层确需共享一个序列，应由上层选择锁等同步机制，并明确这种共享是否符合“一次需要重试的操作使用一个实例”的设计。通常应为每个独立操作创建实例，避免不同重试流程互相推进或复位 `nextBackoff`。

算法本身不会等待；返回的 `Duration` 何时被消费、等待能否取消、任务结束后是否继续复用，都由调用者管理。

## 与 Go 版本的对应关系

Rust 文件逐项对应 `pkg/util/backoff/backoff.go`：`Backoffer` 接口对应 trait，`Exponential` 四个字段和状态含义一致，`NewExponential` 初始化相同，`Backoff(0)` 复位以及后续“前值乘倍率再封顶”的顺序一致。Go 的 `var _ Backoffer = &Exponential{}` 编译期接口断言，在 Rust 中由 `impl Backoffer for Exponential` 表达。

两端仍有可见语言/API 差异：Go 字段未导出，而 Rust 字段全部公开；Go 构造函数返回 `*Exponential`，Rust 返回拥有所有权的值；Go 接收 `int`，Rust 接收不允许负数的 `usize`；Rust 额外提供固有 `Exponential::Backoff` 作为 trait 方法转发入口。Go 直接把 `time.Duration` 转 `float64` 后再转回，Rust 为避免 `Duration::from_secs_f64` 的舍入行为，显式按纳秒截断。

`backoff_test.go::TestExponential` 与 `backoff_test.rs::test_exponential` 覆盖相同三组序列。Rust 的 `migration_aster_unit_test.rs` 额外覆盖实例复位、1.5 倍纳秒截断和 `Box<dyn Backoffer>` 动态分发。未覆盖的极端浮点与超大时长行为不能仅凭常规序列推断为 Go 完全等价。

## 扩展指南

- 调整增长公式或封顶顺序时，修改 `impl Backoffer for Exponential::Backoff`，保留固有方法的纯转发，避免两套算法漂移。
- 新增构造校验、抖动或其他策略前，先确认 Go 对照是否有相同语义。抖动会改变确定性序列与可复现性，宜作为新类型/明确配置，而不是悄然改变 `Exponential`。
- 若要让 Rust 生产路径采用该策略，应在实际重试循环中显式消费返回的 `Duration`，同时实现错误分类、等待/取消与最大尝试次数；不要误认为本 crate 已提供这些能力。
- 修改公开字段可见性、trait 签名、返回所有权方式或浮点转换规则属于兼容性变化，应检查所有声明该 Cargo 依赖的 crate，并同步独立测试。
- 测试逻辑继续放在 `pkg/util/backoff/backoff_test.rs` 或 `migration_aster_unit_test.rs`，不要内嵌到生产源文件。至少同步 Go 的固定倍率/封顶用例，并为复位、跳号、`base > max`、小数倍率和大数精度等新增契约补回归。
- 性能上单次调用为常数时间且无分配；引入随机源、锁或异步 sleep 会改变这一性质，应由上层生命周期与性能需求驱动。

## 验证依据

- RustCodeGraph：`status` 显示索引覆盖 7,032 个 Rust 文件；`files --filter pkg/util/backoff` 找到目标、Go 对照、两个 Rust 测试及 crate 入口；`node --file pkg/util/backoff/backoff.rs` 读取完整 92 行和 7 个符号；同样读取了 `backoff.go`、`backoff_test.rs`、`backoff_test.go`、`lib.rs` 与 `migration_aster_unit_test.rs`。宽泛 `explore` 和同名 `callers` 查询存在歧义/超时，因此调用关系结论由下面的精确检索交叉验证。
- crate/装配：`pkg/util/backoff/Cargo.toml`、`pkg/util/backoff/lib.rs`、根 `Cargo.toml`，以及声明 `astersql-util-backoff` 的 DDL/DXF Cargo manifests。
- 实现与测试：`pkg/util/backoff/backoff.rs`、`pkg/util/backoff/backoff_test.rs`、`pkg/util/backoff/migration_aster_unit_test.rs`。
- Go 对照：`pkg/util/backoff/backoff.go`、`pkg/util/backoff/backoff_test.go`；生产调用通过 `rg` 在 session、DDL、DXF/importinto 及其 framework 路径中核对。
- Rust 接线核对：对全部 `*.rs` 精确检索 `NewExponential` 与 `astersql_util_backoff`，只命中目标实现和同 crate 测试；因此文中将生产接线状态标为“Cargo 已声明、当前未发现生产调用”。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务给定命令验证本文恰有 11 个固定二级章节，并人工复核定位、流程、边界、Go 差异和扩展入口均有上述路径或符号支撑。
