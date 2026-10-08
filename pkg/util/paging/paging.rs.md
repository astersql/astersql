# `pkg/util/paging/paging.rs`

## 文件定位

`paging.rs` 是 `astersql-util-paging` crate 的算法实现文件，属于 `pkg/util` 下的分布式 SQL 分页工具。crate 根 [`pkg/util/paging/lib.rs`](lib.rs) 通过 `pub mod paging` 加载本文件，再用 `pub use paging::*` 对外再导出公开常量和函数。[`pkg/util/paging/Cargo.toml`](Cargo.toml) 声明的 crate 名为 `astersql-util-paging`，没有额外 feature 或直接依赖，并将 Go 对照包记为 `pkg/util/paging`。

分页策略的用意是将大范围扫描拆成多批请求：首批较小，未耗尽数据时逐步放大 page size，以更早交付首批结果。本文件只计算“下一页大小”和“预估 seek 次数”，不发送请求，也不保存扫描进度。

## 核心职责

- 定义与 Go 包对齐的分页默认值与策略阈值：`MinPagingSize` (128)、`MinAllowedMaxPagingSize` (50,000) 和 `Threshold` (960)。
- `GrowPagingSize` 把当前页大小加倍，同时把过小的调用方上限防御性抬高到 50,000，然后对增长结果封顶。
- `CalculateSeekCnt` 根据预期返回行数，估算几何增长分页会产生的 seek 次数；超过前 8 批的部分按 50,000 行一批向上取整。
- 保留 Go `uint64` 的整数算术形状，包括极值下的环绕，而不在 Rust debug 构建中因加法溢出 panic。

## 主要符号

| 符号 | 可见性 | 语义 |
| --- | --- | --- |
| `MinPagingSize: u64 = 128` | `pub` | 首批分页的默认最小行数。 |
| `maxPagingSizeShift: u32 = 7` | `pub(crate)` | 几何增长区间的最大移位参数；与初始页共同对应 8 次 seek。 |
| `pagingSizeGrow: u64 = 2` | `pub(crate)` | 页大小每次增长倍率。 |
| `MinAllowedMaxPagingSize: u64 = 50000` | `pub` | 允许的 max page size 下界，也是几何阶段后的固定分批尺寸。 |
| `pagingGrowingSum: u64` | `pub(crate)` | `((2 << 7) - 1) * 128 = 32640`，表示前 8 个几何分页项的总行数。 |
| `Threshold: u64 = 960` | `pub` | 分页策略的规划器阈值；本文件本身不解释或应用它。 |
| `GrowPagingSize(size, maxv) -> u64` | `pub` | 产生下一个分页大小：校正上限、加倍、封顶。 |
| `CalculateSeekCnt(expectCnt) -> f64` | `pub` | 用分段几何公式返回 seek 估值。虽然类型为 `f64`，当前分支均产生整数值。 |

文件没有 struct、enum、trait、`impl` 或条件编译项；`#![allow(non_snake_case, non_upper_case_globals)]` 是为保留 Go 导出 API 命名。

## 执行流程

`GrowPagingSize` 的流程如下：

1. 检查 `maxv`。若小于 `MinAllowedMaxPagingSize`，先把它抬高到 50,000；因此 `maxv = 0` 不会禁用增长。
2. 执行 `size <<= 1`，即按 `u64` 位模式左移一位。
3. 若移位后的 `size > maxv`，返回 `maxv`；否则返回加倍结果。

`CalculateSeekCnt` 按 `expectCnt` 分三段：

1. `expectCnt == 0` 时直接返回 `0.0`。
2. `0 < expectCnt <= MinPagingSize` 时返回 `1.0`。
3. `MinPagingSize < expectCnt <= pagingGrowingSum` 时，计算 `ratio = expectCnt / 128`（代码保留了 `(pagingSizeGrow - 1)` 因子），再返回 `1 + trunc(log_2(ratio))`。中间结果先转为 `u64` 实现截断，最后转回 `f64`。
4. `expectCnt > pagingGrowingSum` 时，计算超出部分的 `ceil(excess / 50000)`，并返回 `8 + 额外批数`。向上取整的加法使用 `wrapping_add`，保持 Go `uint64` 环绕语义。

## 数据与状态

所有数据都是按值传入的 `u64`，返回值为 `u64` 或 `f64`。模块没有全局可变状态、缓存、计数器或外部资源。

关键不变量是：常规输入下 `GrowPagingSize` 的返回值不超过校正后的 `max(maxv, 50000)`；`CalculateSeekCnt(0) == 0`，非零且不超过 128 行时为 1，32,640 行时为 8，32,641 行时进入固定分批阶段并返回 9。

需注意，`GrowPagingSize` 的“不超过 max”描述以不发生 `u64` 位环绕的常规页大小为前提。极值 `u64::MAX` 左移一位得到 `u64::MAX - 1`，这是测试明确锁定的 Go 兼容行为，不是饱和乘法。

## 依赖与调用关系

下游方面，本文件只使用 Rust 原生整数、浮点数运算和 `f64::ln`，不调用仓库其他模块。`Cargo.toml` 也没有 `[dependencies]`。

上游方面，`lib.rs` 是确定的 crate 公开入口；工作区 `pkg/lib.rs` 还通过 `facade_util_paging` 将其纳入顶层门面的 `util::paging` 再导出。工作区的 `pkg/distsql/Cargo.toml`、`pkg/executor/Cargo.toml` 和 `pkg/store/copr/Cargo.toml` 声明了对该 crate 的路径依赖（`store/copr` 中为 optional），但对 Rust 生产源码的全局搜索只找到门面再导出，没有找到 `GrowPagingSize` 或 `CalculateSeekCnt` 的实际调用。因此，当前可验证的 Rust 现状是“已实现、已通过 crate 与顶层门面再导出、已被若干 manifest 接入，但未在 Rust 生产代码中调用”；不应把 Go 调用链误写成 Rust 已接线。

Go 对照主链提供了该算法在完整应用中的位置证据：`pkg/store/copr/coprocessor.go` 在分页 coprocessor 任务继续扫描时调用 `GrowPagingSize`；`pkg/planner/core/task.go` 用 `CalculateSeekCnt` 参与代价估算；`pkg/planner/core/plan_cost_ver1.go` 和 `plan_cost_ver2.go` 用 `Threshold` 决定分页条件；`pkg/sessionctx/vardef/tidb_vars.go` 用两个公开尺寸常量设置会话变量默认值。

## 错误处理与边界

两个函数都是总函数形式，签名不返回 `Result`，也没有 I/O 错误。防御与边界策略体现在数值规则中：

- `GrowPagingSize(_, maxv < 50000)` 将上限抬高到 50,000，用来吸收会话变量或请求配置中的异常小值。
- 当加倍值超过上限时返回上限，而不是报错。
- `CalculateSeekCnt(0)` 不进入对数运算，避免 `ln(0)` 并表示无 seek。
- 超过 `pagingGrowingSum` 的加法使用 `wrapping_add`。因此 `CalculateSeekCnt(u64::MAX) == 8.0`：这一结果看似非单调，却是 Go 无符号整数环绕的有意兼容语义，已由 `migration_aster_unit_test.rs` 回归。
- 对数分支通过浮点运算后截断，边界附近的结果必须与 Go `int(math.Log(...))` 保持一致；修改为四舍五入或 `ceil` 会改变代价估算。

## 并发与资源生命周期

本模块无共享可变状态，无锁、原子量、通道、异步任务、线程、事务或资源句柄。每次调用只在栈上处理标量值，不分配堆内存，没有需要关闭或回滚的生命周期。因而函数本身可被多线程并发调用，线程安全性不依赖额外同步。

分页请求的实际网络并发、范围续扫和取消生命周期不在本文件中；Go 端相关状态位于 `pkg/store/copr/coprocessor.go`。

## 与 Go 版本的对应关系

Rust 的直接对照文件是 [`pkg/util/paging/paging.go`](paging.go)。常量名称与数值、两个公开函数的分支顺序、几何公式及返回类型一一对应。Rust 用 `#![allow(...)]` 保留 `GrowPagingSize` 等 Go 风格名称，避免迁移后 API 语义被 snake_case 改名掩盖。

具体语言映射为：Go `size <<= 1` 直接映射为 Rust `u64` 左移；Go 的 `float64(int(math.Log(...)/math.Log(...)))` 映射为 Rust 的 `ln` 比值、`as u64` 截断再转 `f64`；Go 超额分支的无符号加法环绕则用 Rust `wrapping_add` 显式表达。

[`pkg/util/paging/paging_test.rs`](paging_test.rs) 复刻 Go [`paging_test.go`](paging_test.go) 的基本增长、封顶和 seek 边界用例，并保留 `0.1` 的浮点误差窗口。[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 进一步覆盖 `maxv = 0`、`u64::MAX` 左移、增长区间边界以及 `CalculateSeekCnt(u64::MAX)` 的环绕行为。

## 扩展指南

- 调整首页、最大页、增长倍率或增长次数时，需联动检查 `MinPagingSize`、`MinAllowedMaxPagingSize`、`pagingSizeGrow`、`maxPagingSizeShift` 和 `pagingGrowingSum`，否则页大小序列与 seek 代价会不一致。
- 改变 `GrowPagingSize` 时，优先扩展独立测试 `paging_test.rs` 和 `migration_aster_unit_test.rs`，覆盖正常加倍、恰好达到上限、越过上限、过小 `maxv` 和 `u64` 极值。不要把测试内嵌到生产文件。
- 改变 `CalculateSeekCnt` 时，应在两个区间分界（0、128、32,640、32,641）和每个 50,000 增量附近增加用例，并显式决定是否继续兼容 Go 的极值环绕。
- 修改任一公开常量或函数前，需同步核对 `paging.go`、`paging_test.go` 以及 Go 主链中 planner 代价、coprocessor 续扫、会话变量默认值的兼容性。风险主要是规划选择变化、请求批次/延迟变化和 Go/Rust 行为漂移。
- 若要让 Rust 主链真正使用该 crate，应在已声明依赖的 planner/distsql/copr 边界接入公开 API，并为实际请求续扫或代价计算增加跨 crate 测试；仅有 Cargo 依赖不能证明运行时已接线。

## 验证依据

- RustCodeGraph `status` 确认索引可用；`files --filter pkg/util/paging` 确认本 crate 的 Rust/Go 源与独立测试集合；`node --file pkg/util/paging/paging.rs --offset 1 --limit 260` 读取了目标文件全部 99 行。
- RustCodeGraph `query GrowPagingSize --kind function` 和 `query CalculateSeekCnt --kind function` 分别定位了 Rust 实现、Go 实现与 Go 测试。`callers/callees` 对这两个 Rust 符号的精确查询在本地持续无输出，因此调用边改用局部 `rg` 核对，没有将该无输出解读为“无调用”。
- 已读取的直接证据：`pkg/util/paging/Cargo.toml`、`lib.rs`、`paging.go`、`paging_test.rs`、`paging_test.go` 和 `migration_aster_unit_test.rs`。
- `rg` 核对了 Go 调用点 `pkg/store/copr/coprocessor.go`、`pkg/planner/core/task.go`、`plan_cost_ver1.go`、`plan_cost_ver2.go` 与 `pkg/sessionctx/vardef/tidb_vars.go`，也核对了 Rust 依赖声明、`pkg/lib.rs` 的门面再导出，以及 Rust 生产源中尚无该函数实际调用的现状。
- 按任务约束，本次为纯文档分析，未运行 Cargo；结构验证只检查文档存在且恰有 11 个规定二级标题。
