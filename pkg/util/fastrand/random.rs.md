# `pkg/util/fastrand/random.rs`

## 文件定位

`random.rs` 属于独立 crate `astersql-util-fastrand`，提供基于快速伪随机源的高层 API。crate 入口 `pkg/util/fastrand/lib.rs` 将本文件的公开项全部重导出；工作区根 `Cargo.toml` 以 `facade_util_fastrand` 引入该 crate，`pkg/lib.rs` 又在 `util::fastrand` 门面下重导出。因此调用者既可直接依赖 `astersql-util-fastrand`，也可经工作区门面访问这些接口。

本文件不是 SQL 请求主链中的状态管理组件，而是服务器握手随机盐、统计采样等场景可复用的底层工具。当前 Rust 源码搜索只确认了测试侧调用（例如 `pkg/util/misc_test.rs::TestBasicFuncRandomBuf`）；对应 Go 包的生产调用包括 `pkg/server/server.go` 中生成 20 字节 salt，以及 `pkg/statistics/sample.go` 中进行蓄水池采样。不能仅由这些 Go 调用推断对应 Rust 生产路径已经接线。

## 核心职责

- `wyrand`、`wyrand::Next` 与 `_wymix` 实现一个可显式持有种子的轻量 PRNG，用固定的 wrapping 状态推进和 128 位乘积混合生成 `u64`。
- `Buf` 从一次 `runtime::Uint32` 取得种子，然后在局部 `wyrand` 状态上批量生成指定长度的 ASCII 字节，排除 NUL 和 MySQL 密码散列格式所用的 `$` 分隔符。
- `Uint32N` 将底层随机 `u32` 通过乘法高半部缩放到 `[0,n)`；`Uint64N` 将两个 `u32` 拼成 `u64`，对二次幂上界使用位掩码，否则使用余数。
- 这些接口面向速度和 Go 行为兼容，不提供密码学随机性保证，也不保证无偏采样或可复现的全局序列。

## 主要符号

- `pub struct wyrand(pub u64)`：单字段公开 tuple struct，字段即当前 64 位状态。类型名保留 Go 命名；调用者可构造固定种子，也能直接读写状态。
- `pub fn _wymix(a: u64, b: u64) -> u64`：以 `u128` 计算完整乘积，将高、低 64 位异或；对应 Go 的 `bits.Mul64` 加 `hi ^ lo`。
- `pub fn wyrand::Next(&mut self) -> u64`：用 `wrapping_add(0xa0761d6478bd642f)` 推进状态，再混合状态与 `0xe7037ed1a0b428db` 异或后的值。必须取得可变引用，单个实例的调用顺序决定输出序列。
- `pub fn Buf(size: usize) -> Vec<u8>`：分配恰好 `size` 个字节并填充。每个候选值取 `Next` 的低 32 位，与 127 相乘后取乘积高 32 位；结果原本位于 `0..127`，其中 0 与 `$` 分别加一，最终仍小于 127。
- `pub fn Uint32N(n: u32) -> u32`：计算 `high32(Uint32() * n)`；`n == 0` 时乘积恒为零，因此返回 0。
- `pub fn Uint64N(n: u64) -> u64`：用两次 `Uint32` 组成 `v`。`n` 为二次幂时返回 `v & (n-1)`，否则返回 `v % n`；其 wrapping 写法令 `n == 0` 走掩码分支并返回 `v`，不会执行除零。

文件没有 trait、枚举、模块级常量或条件编译项。算法常数直接位于 `Next` 中，测试装配的 `#[cfg(test)]` 位于相邻的 `lib.rs`，不在本文件内。

## 执行流程

`Buf(size)` 的流程是：先分配输出 `Vec<u8>`；调用一次 `runtime::Uint32` 创建局部 `wyrand`；随后循环 `size` 次，每次由 `Next` 先推进状态再输出混合值，将其低 32 位乘 127 并取高半部映射到 ASCII 范围；若结果为 0 或 `$`，则加一；最后返回已填满的缓冲区。整个缓冲区只在开始时访问一次底层线程局部随机源，后续字节来自局部状态。

`Uint32N(n)` 每次只调用一次 `Uint32`，无重试或拒绝采样。`Uint64N(n)` 每次调用两次 `Uint32`，高位来自第一次、低位来自第二次；随后先计算 `mask = n.wrapping_sub(1)`。若 `(n & mask) == 0`，代码把 `n` 当作二次幂（该判定也包含零）并走掩码快速路径，否则走取模路径。

固定种子的 `wyrand` 可复现：`pkg/util/fastrand/migration_aster_unit_test.rs::migration_wymix_and_wyrand_match_go_vectors` 证明种子 0 的前三个输出依次为 `0x111cb3a78f59a58e`、`0xceabd938ff4e856d`、`0x61fb51318f47d2a4`。

## 数据与状态

持久状态只有 `wyrand.0`，每次 `Next` 原地更新，所有算术按 `u64` 模数 wrapping。`_wymix` 无状态且无副作用。`Buf` 的状态和输出向量均为函数局部所有权，返回后不保留全局缓存；其初始种子只有 `Uint32` 提供的 32 位熵。

`Uint32N` 和 `Uint64N` 不保存状态，随机状态实际由 `pkg/util/fastrand/runtime.rs::Uint32` 背后的 `fastrand::u32(..)` 管理。由于实现不使用拒绝采样，非整除范围可能存在模缩放偏差；尤其 `Uint64N` 的普通分支直接取模。这与 Go 版本一致，是性能/兼容选择，不应把它描述为严格均匀或密码学安全。

`Buf` 的输出所有权交给调用者；空长度返回空向量。它只保证避开两个分隔字符并限制字节范围，不保证 UTF-8 可打印字符：例如控制字符 1 仍可能出现。

## 依赖与调用关系

下游唯一源码依赖是同 crate 的 `super::runtime::Uint32`；`runtime.rs` 再调用外部依赖 `fastrand = "2"`。本文件自身不直接调用标准随机库。`Cargo.toml` 的 `rand = "0.8"` 是开发依赖，仅供 `random_test.rs` 的对照路径使用。

内部调用边为：`Buf -> wyrand::Next -> _wymix`，`Buf -> Uint32`，`Uint32N -> Uint32`，`Uint64N -> Uint32`（两次）。RustCodeGraph 能索引这些符号和源码，但对 Go 风格大写函数执行精确 `callers/callees` 未返回可用跨 crate 边；因此上游接线另以源码搜索核验。

上游 Rust 边界由 `pkg/util/fastrand/lib.rs` 的 `pub use random::*` 暴露，并由 `pkg/lib.rs` 的 `util::fastrand` 门面继续重导出。已核实的直接 Rust 调用是 `pkg/util/misc_test.rs` 对 `astersql_util_fastrand::Buf(5)` 的约束测试；`pkg/server/Cargo.toml` 声明了该 crate 依赖，但源码搜索未发现服务器 Rust 代码直接调用本文件 API。Go 生产调用位于 `pkg/server/server.go` 和 `pkg/statistics/sample.go`，是迁移意图和应用位置证据，不是 Rust 接线证据。

## 错误处理与边界

所有 API 都直接返回值，没有 `Result` 或显式错误分支。整数计算显式或天然使用无符号 wrapping 语义，避免 debug/release 溢出差异。`Buf` 的分配仍可能因极大 `size` 导致容量溢出或内存分配失败并 panic/中止；接口不尝试恢复。

边界语义由迁移测试确认：`Buf(0)` 返回空向量；`Uint32N(0)` 返回 0；`Uint64N(1)` 返回 0；`Uint64N(0)` 不 panic，而是返回未约束的拼接随机值。最后一点来自零也满足当前“二次幂”位判定，扩展或重构时不可擅自改成取模零或新增 panic，否则会偏离现有 Go/Rust 契约。

`Buf` 只修正恰好为 0 或 `$` 的候选字节，不做重采样，所以 1 和 `%` 的概率会叠加相邻被替换值的概率。若调用场景需要均匀字符分布、限定可打印字符或安全随机盐，必须另建明确契约，而不是默默改变该函数。

## 并发与资源生命周期

本文件不创建线程、锁、通道、任务、文件句柄或事务。`Buf` 的 `wyrand` 为栈上局部可变状态，输出向量由单一调用拥有，不在调用间共享；`Uint32N`/`Uint64N` 只短暂调用底层随机源。`runtime.rs` 说明底层 `fastrand` 使用线程局部生成器，因此常见并发调用无需本文件持锁。

`wyrand` 本身没有显式 `Sync` 协议：并发共享同一实例并调用 `Next` 需要调用者通过 Rust 的可变借用或外部同步保证独占访问。独立实例互不影响。`random_test.rs::run_parallel` 会在多个 scoped 线程中反复调用公开无状态入口，用于覆盖并发调用路径，但它不是正式性能基准，也不证明密码学或统计质量。

## 与 Go 版本的对应关系

Rust 文件逐项对应 `pkg/util/fastrand/random.go`：Go `type wyrand uint64` 对应公开 tuple struct；Go `bits.Mul64` 对应 Rust `u128` 乘法拆分；`Next` 的两个常数、先推进后混合的顺序完全一致；`Buf` 的低 32 位截断、乘 127、右移 32 位和分隔符修正规则一致；两个有界函数的缩放、拼接、二次幂快速路径也保持一致。

有两处语言层差异需要注意。Rust `Buf` 接受 `usize`，不存在 Go `int` 的负数输入；Go 负长度会在分配时失败，而 Rust 调用者必须在调用前把有符号长度安全转换为 `usize`。Rust 在 `Next` 和 `Uint64N` 中显式使用 `wrapping_add`/`wrapping_sub`，把 Go 无符号整数天然回绕语义固定下来。

`random_test.rs::test_rand` 对应 Go `TestRand`，验证有界结果并抽样检查 256 个桶的覆盖；同文件四个 `benchmark_*` 被实现为 `#[test]` 的并行工作负载，而非 Rust benchmark harness。额外的 `migration_aster_unit_test.rs` 用固定向量和边界断言补强 Go 对齐证据。测试逻辑保持在独立文件，由 `lib.rs` 的 `#[path]` 测试模块接入，没有内嵌进生产源文件。

## 扩展指南

新增算法级能力时，优先在本文件增加独立公开函数或 `wyrand` 方法，并在 `pkg/util/fastrand/random_test.rs` 增加一般行为/并发覆盖，在 `pkg/util/fastrand/migration_aster_unit_test.rs` 增加固定 Go 向量和边界对照；不要把测试写入 `random.rs`。若修改 `Uint32` 来源，则应在 `runtime.rs` 及其独立测试中处理，并重新评估本文件所有入口的并发和确定性。

修改 `_wymix`、推进常数、位截断顺序或 `Uint64N(0)` 分支会改变 Go 兼容序列或边界契约；修改 `Buf` 字符映射会影响服务器 salt 等 Go 端既有语义。此类变化应先同步核对 `random.go`，保留固定向量，并检查 `pkg/server/server.go`、`pkg/statistics/sample.go` 的兼容需求。若需求是密码学安全、严格无偏或稳定跨线程复现，应提供命名清楚的新 API，不能在无迁移计划时替换现有快速接口。

接入新的 Rust 生产调用者时，应在调用 crate 的 `Cargo.toml` 声明依赖，或使用现有 facade；同时增加调用场景自己的独立测试。特别要明确上界是否可能为零、是否接受取模偏差，以及随机值是否会进入安全边界或持久化格式。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `pkg/util/fastrand`；`files --filter pkg/util/fastrand` 列出目标、入口、运行时、Go 对照和测试；`node --file` 核读了 `random.rs`、`lib.rs`、`runtime.rs`、`random_test.rs`、`migration_aster_unit_test.rs`、`random.go`、`random_test.go`。对 `Buf`、`Uint32N`、`Uint64N` 的精确 callers/callees 查询未产生可用输出，已用仓库搜索补齐上游证据。
- crate/装配：`pkg/util/fastrand/Cargo.toml` 确认 crate 名、`fastrand = "2"`、仅测试使用的 `rand = "0.8"` 及 Go 包映射；根 `Cargo.toml`、`pkg/lib.rs`、`pkg/util/Cargo.toml`、`pkg/server/Cargo.toml` 用于核对 workspace、facade、测试依赖和服务器依赖边界。
- Go 对照与生产位置：`pkg/util/fastrand/random.go`、`pkg/util/fastrand/random_test.go`、`pkg/server/server.go`、`pkg/statistics/sample.go`。
- Rust 测试：`pkg/util/fastrand/random_test.rs`、`pkg/util/fastrand/migration_aster_unit_test.rs`、`pkg/util/misc_test.rs`。未运行 Cargo，符合本纯文档任务约束；结论来自源码、索引和结构检查，不声称执行了测试。
