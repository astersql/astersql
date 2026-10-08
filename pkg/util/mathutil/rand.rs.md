# `pkg/util/mathutil/rand.rs`

## 文件定位

本文件实现 `astersql-util-mathutil` crate 中的 MySQL 兼容伪随机数生成器。模块本身在 [`lib.rs`](./lib.rs) 中以私有 `mod rand` 挂载，但 `MysqlRng`、`NewWithSeed` 和 `NewWithTime` 被 crate 根公开再导出，因此表达式等上层 crate 不需要感知内部模块路径。crate 边界、默认 `formal-crate` feature 以及 Go 包映射 `pkg/util/mathutil` 由 [`Cargo.toml`](./Cargo.toml) 声明；本文件没有条件编译项，也没有额外第三方依赖。

它不是通用密码学随机源。它存在的目的，是用确定性的双种子递推复现 Go/MySQL 的 `RAND()` 数值序列，并提供读取、恢复种子的接口，使随机状态可以随表达式或会话上下文继续传递。

## 核心职责

1. `NewWithSeed` 把一个有符号 64 位种子确定性地映射为两个受 `maxRandValue` 约束的 `u32` 内部种子。
2. `NewWithTime` 用 Unix 纪元相对纳秒数生成非确定性初始种子，然后复用 `NewWithSeed`。
3. `MysqlRng::Gen` 按 MySQL/Go 的既定先后顺序更新两个种子，并返回 `[0, 1)` 的 `f64`。
4. `GetSeed1`、`GetSeed2`、`SetSeed1`、`SetSeed2` 暴露状态快照与恢复所需的最小接口。
5. 所有种子读写共享同一个 `Mutex`，保证一次生成操作的双种子更新不会被其他生成或状态访问打断。

## 主要符号

- `const maxRandValue: u32 = 0x3FFF_FFFF`：模数和输出归一化分母，与 Go 的 `maxRandValue` 相同。它不是可公开配置项。
- `pub struct MysqlRng { seeds: Mutex<(u32, u32)> }`：唯一生产类型。两个种子放在同一个互斥区内，类型没有实现 `Clone`；上层需要共享时使用 `Arc<MysqlRng>`。
- `pub fn NewWithSeed(seed: i64) -> Box<MysqlRng>`：公开构造函数。使用 wrapping 算术模拟固定宽度整数溢出，再转换为 `u32` 并取模；返回 `Box` 与 Go 返回指针的所有权形态对应，也便于上层转成 `Arc`。
- `pub fn NewWithTime() -> Box<MysqlRng>`：公开时间种子构造函数。系统时间晚于纪元时取正纳秒数，早于纪元时取负偏移，最终委托 `NewWithSeed`。
- `pub fn Gen(&self) -> f64`：公开的有状态生成入口。先计算新 `seed1`，再用这个新值计算 `seed2`，最后返回 `seed1 / maxRandValue`。
- `SetSeed1` / `SetSeed2`：分别替换一个内部种子，供状态恢复使用；输入不会在 setter 中主动取模。
- `GetSeed1` / `GetSeed2`：分别读取当前种子，供状态序列化或上下文复制使用。

这些公开名称保留了 Go 风格大写命名；crate 根通过 `#![allow(non_snake_case, non_upper_case_globals)]` 接受这一迁移接口。

## 执行流程

固定种子构造流程如下：

1. `NewWithSeed(seed)` 计算 `seed1 = u32(seed * 0x10001 + 55_555_555) % maxRandValue`。
2. 同一输入计算 `seed2 = u32(seed * 0x1000_0001) % maxRandValue`。
3. 两个值组成元组放入同一个 `Mutex`，再把 `MysqlRng` 装箱返回。

时间种子路径先在 `NewWithTime` 中读取 `SystemTime::now()`。若 `duration_since(UNIX_EPOCH)` 成功，使用正的纳秒数；若系统时间早于纪元，使用错误对象携带时长的负值。此后所有初始化规则仍由 `NewWithSeed` 统一完成。

每次 `Gen` 在持锁期间按顺序执行：

1. `seed1 = (seed1 * 3 + seed2) % maxRandValue`；乘加使用 wrapping 算术。
2. `seed2 = (新 seed1 + 旧 seed2 + 33) % maxRandValue`；第二步必须使用刚更新的 `seed1`。
3. 把新 `seed1` 转为 `f64`，除以 `maxRandValue` 后返回。

表达式层有两种直接使用方式：[`builtin_math.rs`](../../expression/builtin_math.rs) 的 `MysqlRand::generate` 推进共享 RNG，而 `rand_with_seed_first_gen` 为每个显式种子新建 RNG 并只取首值；[`builtin_math_vec.rs`](../../expression/builtin_math_vec.rs) 对无显式种子的向量化 `RAND()` 按行推进共享状态，对显式种子版本则每行构造后取首值。

## 数据与状态

持久状态只有 `Mutex<(u32, u32)>` 内的双种子。`Gen` 会同时改变二者；getter 不改变状态；setter 只改变指定分量。因为两个 setter 是两次独立加锁，连续调用 `SetSeed1`、`SetSeed2` 并不是一个整体原子恢复操作：若同一实例正在被其他线程并发使用，其他操作可能观察到只恢复了一半的中间状态。当前静态表达式上下文的复制路径先创建未共享的新实例，再连续设置两项，因此不会触发这一问题，见 [`exprstatic/exprctx.rs`](../../expression/exprstatic/exprctx.rs) 的 `MakeExprContextStatic`。

构造阶段把初始种子限制在 `[0, maxRandValue)`；正常 `Gen` 也在每步取模。setter 则忠实保存任意 `u32`，但下一次 `Gen` 会重新取模。输出使用更新后的 `seed1`，所以其范围为 `[0, 1)`，不会等于 `1.0`。

## 依赖与调用关系

下游依赖仅来自标准库：`std::sync::Mutex` 负责同步，`std::time::{SystemTime, UNIX_EPOCH}` 负责时间种子。生成算法不访问 I/O、全局变量或外部随机源。

主要上游关系由 RustCodeGraph 的文件索引和限定符号搜索确认：

- [`lib.rs`](./lib.rs) 再导出全部三个公开入口，使包外调用通过 `mathutil::{MysqlRng, NewWithSeed, NewWithTime}` 完成。
- [`builtin_math.rs`](../../expression/builtin_math.rs) 的 `MysqlRand` 以 `Arc<MysqlRng>` 包装共享实例；`generate` 调用 `Gen`，`rand_with_seed_first_gen` 调用 `NewWithSeed(...).Gen()`。
- [`builtin_math_vec.rs`](../../expression/builtin_math_vec.rs) 的 `builtinRandSig::vecEvalReal` 每行调用共享实例的 `Gen`；`builtinRandWithSeedFirstGenSig::vecEvalReal` 每行用输入种子调用 `NewWithSeed` 后生成首值。
- [`legacy_vectorized_runtime.rs`](../../expression/legacy_vectorized_runtime.rs) 的 `MathBase` 持有 `Arc<MysqlRng>`，`with_seed` 将 `NewWithSeed` 的 `Box` 转为 `Arc`。
- [`exprstatic/exprctx.rs`](../../expression/exprstatic/exprctx.rs) 的 `NewExprContext` 用 `NewWithTime` 建立默认上下文随机状态；`MakeExprContextStatic` 在无法复用原 `Arc` 时，用四个 Get/Set 方法复制双种子。

因此该文件位于 SQL 表达式执行的基础工具层：它不解析 SQL，也不决定 `RAND()` 的 NULL/逐行语义；表达式签名决定调用模式，本文件只保证兼容序列和状态同步。

## 错误处理与边界

所有公开函数都不返回 `Result`。`SystemTime` 早于 Unix 纪元不是错误返回，而是转换成负种子。整数计算显式使用 `wrapping_mul`、`wrapping_add`，避免 Rust 构建模式改变溢出行为，并对齐 Go 固定宽度整数的回绕语义；负种子和 `i64::MAX` 的黄金值由测试覆盖。

每次加锁都使用 `expect("MysqlRng mutex poisoned")`。若某个持锁线程 panic 导致互斥锁中毒，后续 `Gen` 和所有 getter/setter 会继续 panic，而不是恢复状态或传播可处理错误。调用方也不能从接口区分时钟异常或锁中毒。

该实现只承诺算法序列，不承诺密码学不可预测性。`NewWithTime` 的种子取决于系统时钟，短时间内重复构造理论上可能得到相同种子；测试用至少 1 ms 的等待降低这一概率，但这不是 API 的唯一性保证。

## 并发与资源生命周期

`MysqlRng` 自身拥有互斥锁和种子，没有后台任务、通道、文件句柄或显式清理过程；最后一个 `Box`/`Arc` 被释放时资源随 Rust 所有权自动销毁。由于 `Mutex<(u32, u32)>` 同时保护两个种子，一次 `Gen` 对其他访问者表现为原子状态迁移，多线程共享同一 `Arc<MysqlRng>` 不会发生数据竞争。

并发调用的结果集合仍来自同一条确定性序列，但哪个线程获得哪一个值取决于锁获取顺序，不具备线程间确定次序。单个 getter 读取和单个 setter 写入各自原子；两次 getter 组成的快照、或两个 setter 组成的恢复，并非跨调用原子操作。若将来要求并发下无中间态的保存/恢复，应增加一次持锁读取/写入双种子的成对 API，并在独立测试文件中验证，而不应让调用方自行拼接两个方法。

## 与 Go 版本的对应关系

直接对照文件是 [`rand.go`](./rand.go)，对应测试为 [`rand_test.go`](./rand_test.go)。Rust 保留了 Go 的 `maxRandValue`、两个初始化公式、`Gen` 更新顺序、四个状态访问器和大写 API 名称。Go 使用 `*sync.Mutex` 加两个独立字段；Rust 将 `(seed1, seed2)` 一并置于 `Mutex` 中，保护范围等价且更直接表达共同不变量。Go 返回 `*MysqlRng`，Rust 返回 `Box<MysqlRng>`；上层需要共享时显式转换成 `Arc`。

溢出兼容是移植重点：Rust 构造和递推显式使用 wrapping 算术，再按 Go 相同位置转换为 `u32`、取模。Rust [`rand_test.rs`](./rand_test.rs) 与 Go 测试共享种子 `0`、`1`、`-1`、`i64::MAX` 的前两个输出黄金值，也共享手工恢复 `10_000_000`/`1_000_000` 后的三个输出和最终双种子。Rust 还在 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 重复核对迁移语义。

时间异常处理形式略有差异：Go 的 `time.Now().UnixNano()` 直接给出有符号纳秒数；Rust 根据 `duration_since(UNIX_EPOCH)` 的成功或失败显式构造正、负值。两边都不保证时间种子的唯一性。

## 扩展指南

- 修改递推公式、常量、转换顺序或初始种子映射时，必须先确认 MySQL 和 [`rand.go`](./rand.go) 的兼容要求；即使输出仍位于 `[0,1)`，序列变化也会破坏 SQL 兼容和可复现测试。
- 增加公开构造或状态 API 时，应在 [`lib.rs`](./lib.rs) 决定是否再导出，并在独立的 [`rand_test.rs`](./rand_test.rs) 中补测试；不要把测试嵌入本生产文件。
- 增加双种子快照/恢复能力时，优先设计一次加锁的成对接口，避免现有两个 getter/setter 在并发组合时产生非原子快照或中间态；同时检查 `MakeExprContextStatic` 的接线是否应迁移到新接口。
- 改变返回所有权（`Box`/`Arc`）会影响表达式上下文、`MysqlRand` 和 `MathBase` 的共享方式，需检查上述直接调用点，而不是只修改本 crate。
- 性能优化应保持“一次 `Gen` 一次锁”的状态原子性。该锁位于向量化 `RAND()` 的逐行热路径；若改为批量生成，需要验证输出顺序与逐次调用完全一致，并增加并发及批量等价测试。
- 新增密码学或统计性质不同的 RNG 应建立独立类型，不应复用 `MysqlRng` 名义或悄然替换其兼容算法。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 `pkg/util/mathutil/rand.rs`；`explore` 与 `node --file pkg/util/mathutil/rand.rs` 核对文件的常量、类型、函数和方法；`query MysqlRng`、`query NewWithSeed`、`query NewWithTime` 定位 Rust/Go 定义及表达式上下文关联。精确 `callers` 查询在等待约 90 秒后未返回并被中止，因此调用边另用限定 Rust 搜索及 RustCodeGraph 对上游文件的 `node --file` 结果交叉核验，未把不完整 callers 输出当作证据。
- 生产源码：[`rand.rs`](./rand.rs)、[`lib.rs`](./lib.rs)、[`Cargo.toml`](./Cargo.toml)、[`builtin_math.rs`](../../expression/builtin_math.rs)、[`builtin_math_vec.rs`](../../expression/builtin_math_vec.rs)、[`legacy_vectorized_runtime.rs`](../../expression/legacy_vectorized_runtime.rs)、[`exprstatic/exprctx.rs`](../../expression/exprstatic/exprctx.rs)。
- Go 对照：[`rand.go`](./rand.go)。
- 独立测试：[`rand_test.rs`](./rand_test.rs)、[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)、[`rand_test.go`](./rand_test.go)。这些测试证明固定种子序列、负数与最大种子回绕、输出范围、时间种子基本差异以及双种子恢复结果；它们没有证明密码学安全、时间种子绝对唯一，也没有覆盖锁中毒或成对 getter/setter 的并发原子性。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证只检查固定章节结构、路径和人工事实一致性。
