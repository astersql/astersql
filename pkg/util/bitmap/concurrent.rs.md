# `pkg/util/bitmap/concurrent.rs`

## 文件定位

该文件实现 `astersql-util-bitmap` crate 的定长并发位图。crate 入口 `pkg/util/bitmap/lib.rs` 通过 `pub mod concurrent` 声明模块并用 `pub use concurrent::*` 重导出全部公开项；根门面 `pkg/lib.rs` 又在 `util::bitmap` 下重导出该 crate。`pkg/util/bitmap/Cargo.toml` 将库入口指定为 `lib.rs`，并用 `package.metadata.porting.go-package = "pkg/util/bitmap"` 标记其 Go 对照包。

它是一个底层工具实现，不直接处理 SQL、事务或存储。workspace 和 `pkg/executor/join/Cargo.toml` 已声明该 crate，但当前 Rust 生产源码中能检索到的 Join 使用仍是 `pkg/executor/join/hash_table_v1.rs`、`hash_join_v1.rs` 内的注释化迁移草稿；因此现有直接可验证的运行使用主要来自独立 Rust 测试，不能据此声称它已经进入完整 SQL 执行主链。

## 核心职责

- 用 `Vec<AtomicU32>` 把逻辑位空间切成 32-bit segment，并按需向上取整分配（`NewConcurrentBitmap`）。
- 用顺序一致的 CAS 循环实现并发 `0 -> 1` 置位，并让唯一成功者得到 `true`（`ConcurrentBitmap::Set`）。
- 提供需要调用方独占可变借用的快速置位、轻量读取、深拷贝、容量复用式重置和内存估算（`UnsafeSet`、`UnsafeIsSet`、`Clone`、`Reset`、`BytesConsumed`）。
- 保持与 `pkg/util/bitmap/concurrent.go` 的位序、分段、越界忽略和唯一 setter 语义一致。

此类型只支持置位，不提供一般性的并发清位、动态增长或位图集合运算。长度在构造或独占 `Reset` 时确定。

## 主要符号

- `segmentWidth: usize = 32`：每个原子 segment 的位数。
- `segmentWidthPower: usize = 5`：以右移 5 位代替除以 32，得到 segment 下标。
- `bitMask: u32 = 0x80000000`：segment 内 bit 0 从最高位开始；目标掩码为 `bitMask >> (bitIndex % 32)`。
- `bytesConcurrentBitmap: i64`：`ConcurrentBitmap` 结构体本身的 `size_of`，不包含 `Vec` 指向的堆存储。
- `ConcurrentBitmap { pub segments: Vec<AtomicU32>, pub bitLen: i32 }`：位图状态。两个字段目前公开，外部代码理论上可破坏“`segments` 足以覆盖 `bitLen`”的不变量，扩展时应避免依赖不受控的字段写入。
- `NewConcurrentBitmap(bitLen: i32) -> ConcurrentBitmap`：按 `(bitLen + 31) >> 5` 创建清零 segments。
- `Set(&self, bitIndex: i32) -> bool`：并发安全置位；仅实际完成 `0 -> 1` 的调用返回 `true`。
- `UnsafeSet(&mut self, bitIndex: i32)`：借助独占可变借用直接修改目标 `AtomicU32` 的内部值，不报告是否首次置位。
- `UnsafeIsSet(&self, bitIndex: i32) -> bool`：用 `Relaxed` 原子 load 检查目标位。名称沿用 Go API；Rust 实现仍执行原子读取，但不建立跨变量同步关系。
- `Clone(&self) -> ConcurrentBitmap` 及标准 `Clone::clone`：逐 segment 以 `SeqCst` load/store 复制到独立分配。
- `Reset(&mut self, bitLen: i32)`：已有 segment 数足够时全部清零并复用，否则重新分配。
- `BytesConsumed(&self) -> i64`：返回结构体大小加 `segments.capacity() * 4` 的估算值。

## 执行流程

构造时，`NewConcurrentBitmap` 将逻辑长度向上取整为 segment 数，为每段创建值为 0 的 `AtomicU32`，并保存 `bitLen`。例如长度 33 需要两个 segment；这一行为由 `migration_constructor_rounds_segments_and_ignores_out_of_range_bits` 覆盖。

`Set` 先拒绝负下标和 `bitIndex >= bitLen`。合法下标通过右移 5 位定位 segment，通过对 `0x80000000` 右移段内偏移定位 bit。随后循环：以 `SeqCst` 读取旧值；若 bit 已为 1，立即返回 `false`；否则计算 `old | mask` 并执行 `compare_exchange`。CAS 成功的唯一调用返回 `true`，失败表示该 segment 在观察后被其他线程改变，于是重新读取并重试。

`UnsafeSet` 使用相同映射规则，但要求 `&mut self`，通过 `AtomicU32::get_mut` 在独占访问下直接 OR 掩码。`UnsafeIsSet` 检查相同的上下界和掩码，以 `Relaxed` load 后测试掩码。二者都不会扩容。

`Clone` 先按原 `bitLen` 构造新位图，再逐段复制观察值；标准 `Clone` trait 委托给同名 Go 风格方法。`Reset` 计算新长度所需段数：若现有 `segments.len()` 足够，会清零所有已分配段而不缩短 vector；否则用恰好所需的新 vector 替换旧存储。最后更新逻辑长度。

## 数据与状态

逻辑位索引从 0 开始，segment 内采用高位优先映射：0 对应首段 `0x80000000`，31 对应 `0x00000001`，32 对应下一段最高位。`migration_set_maps_bits_across_segments_and_has_one_setter` 覆盖 0、31、32、63、64 的跨段边界。

核心不变量是 `segments.len() >= ceil(bitLen / 32)`，且有效索引范围为 `[0, bitLen)`。构造、`Reset` 和所有访问方法都按此关系计算；不过字段公开意味着 crate 使用者可绕开这些方法制造不一致状态。

`Reset` 缩短长度时保留原 vector 长度和容量，但将所有 segment（包括新逻辑长度之外的段）清零。`BytesConsumed` 按容量而不是长度计费，因此能反映保留的可复用堆内存；它是估算值，不包括分配器元数据或对齐外的额外开销。`migration_clone_is_independent_and_reset_reuses_or_grows_storage` 与 `migration_bytes_consumed_tracks_allocated_segment_capacity` 分别验证复用/扩容和计费关系。

## 依赖与调用关系

下游依赖仅来自 Rust 标准库：`std::sync::atomic::{AtomicU32, Ordering}` 提供原子读写与 CAS，`std::mem` 提供结构体静态大小。目标 crate 的 Cargo manifest 没有声明第三方依赖或 feature。

模块接线为 `pkg/util/bitmap/lib.rs -> concurrent -> 公开重导出`，根 crate 再通过 `pkg/lib.rs::util::bitmap` 暴露门面。RustCodeGraph 对 `NewConcurrentBitmap` 的精确 caller 查询确认 `ConcurrentBitmap::Clone` 是目标生产文件内的直接调用者；测试调用则见 `concurrent_test.rs` 与 `migration_aster_unit_test.rs`。`Set` 的直接下游是目标 segment 的 `load`、位或运算和 `compare_exchange`，没有 I/O、锁、任务或外部错误类型。

`pkg/executor/join/Cargo.toml` 声明 `astersql-util-bitmap` 路径依赖；相邻 Join Rust 文件包含与 Go hash join 对应的 `ConcurrentBitmap`/`NewConcurrentBitmap` 草稿，但相关段落目前为注释，故只能作为预期迁移入口，不能作为已运行调用边。RustCodeGraph 的宽泛文件使用摘要包含大量名称相似项，文档不将这些歧义结果视为真实调用者。

## 错误处理与边界

API 不返回 `Result`。位下标为负数或不小于 `bitLen` 时，`Set` 返回 `false`，`UnsafeSet` 静默不操作，`UnsafeIsSet` 返回 `false`；相关负数和上界行为由迁移测试覆盖。

`bitLen` 的类型是 `i32`，但构造和重置没有显式拒绝负长度。计算结果最终转换成 `usize`：小范围负值可能算得 0，更小的负值可能在转换后形成极大的 segment 数并导致分配失败；正值加 31 也受 `i32` 算术边界约束。当前 Go 版本同样没有业务级错误返回并依赖调用方提供有效长度。安全扩展时应把“非负且加法不溢出”视为前置条件，若要验证长度，应同步决定 Go/Rust 兼容策略并补独立测试。

公开字段允许构造 `bitLen` 与 `segments` 不匹配的值；在这种非规范状态下，合法逻辑下标仍可能触发 vector 越界 panic。正常构造和 `Reset` 不会产生该状态。

## 并发与资源生命周期

`Set(&self, ...)` 可由共享引用并发调用；每个 segment 使用 `AtomicU32`，CAS 将竞争限制在 32 个 bit 的范围内。成功和失败 ordering 都是 `SeqCst`，因此所有 `Set` 原子操作位于全局顺序中。`TestConcurrentBitmapUniqueSetter` 和 `migration_concurrent_set_reports_exactly_one_winner` 验证同一 bit 的竞争恰有一个成功者。

`UnsafeSet` 和 `Reset` 需要 `&mut self`，安全 Rust 会阻止它们与其他借用并发执行。`UnsafeIsSet` 虽使用 `Relaxed` 原子 load，但只可靠地报告该次 load 观察到的 bit 值，不为其他共享状态提供 happens-before 保证。测试在工作线程结束或 barrier/join 后读取，因此同步由线程生命周期建立。

`Clone` 的逐段读取是原子的，但不是整个 bitmap 的事务性快照：若其他线程同时在不同段置位，副本可能观察到不同时间点的组合。副本拥有独立 vector，之后的修改互不影响。内存由 `Vec` 和 `ConcurrentBitmap` 的普通 RAII 生命周期管理，无显式关闭、后台任务或通道。

## 与 Go 版本的对应关系

`pkg/util/bitmap/concurrent.go` 是直接语义基准。常量、MSB-first 位序、32-bit 分段、向上取整、越界忽略、CAS 重试、重置复用和容量计费均一一对应。Rust 用 `AtomicU32` 代替 Go 的 `[]uint32` 加 `sync/atomic` 函数；Go 的指针返回变成按值返回，所有权随后由调用方或 `Arc` 管理。

Go `Set` 使用 `atomic.LoadUint32`/`CompareAndSwapUint32`，Rust 使用 `SeqCst` load/`compare_exchange` 保留强顺序与唯一 setter 语义。Go `UnsafeSet`/`UnsafeIsSet` 是普通非原子内存访问；Rust 的 `UnsafeSet` 借助 `&mut self` 和 `get_mut`，`UnsafeIsSet` 则因为存储类型固定为 `AtomicU32` 而采用 `Relaxed` 原子 load，因此它并非字面上的非原子读取。

Go `Clone` 在并发写入期间直接读取普通 `uint32`，Rust 对每段执行 `SeqCst` load；两者都不承诺全位图一致快照。Rust 额外实现标准 `Clone` trait。Rust 独立测试 `concurrent_test.rs` 对齐 Go `concurrent_test.go` 的并发置位、唯一 setter 和重置场景；`migration_aster_unit_test.rs` 进一步覆盖 Go 测试未显式覆盖的跨段、越界、克隆独立性和内存估算。

## 扩展指南

- 新增位操作时，应复用 `segmentWidthPower` 与 `bitMask` 的 MSB-first 映射，并在 `pkg/util/bitmap/concurrent_test.rs` 或 `migration_aster_unit_test.rs` 增加独立测试，至少覆盖 31/32 等 segment 边界、重复调用和越界。
- 新增并发清位、测试并置位或批量操作时，应明确返回值的线性化点与 atomic ordering；若一次操作跨多个 segment，必须说明是否允许非一致快照，不能把逐段原子误写成整体事务性。
- 修改 `Reset` 或长度模型时，应保持独占访问，检查缩短复用、扩容清零、`BytesConsumed` 容量计费和负/极大长度策略，并同步核对 Go 实现。
- 若封装公开字段，应先检索 `segments` 与 `bitLen` 的直接测试/调用；现有测试会直接检查字段并用 CAS 清位，需要同时迁移测试辅助接口。
- 将该位图真正接入 Join 时，最可能的入口是 `HashContext` 的 NULL 标记和 outer build matched status 草稿；应先把相应注释化模块变成真实实现并建立可编译调用边，不能仅凭 Cargo 依赖判断功能已启用。
- Rust 测试逻辑应继续放在独立的 `concurrent_test.rs` 或迁移测试文件中，不要内嵌回生产源文件。

## 验证依据

- RustCodeGraph：`status` 确认索引包含目标项目；`files --filter pkg/util/bitmap` 确认 7 个相邻实现/测试文件；`node --file pkg/util/bitmap/concurrent.rs` 读取 171 行目标文件及 10 个符号；`query NewConcurrentBitmap --kind function --json` 区分 Go/Rust 定义；`callers NewConcurrentBitmap --file pkg/util/bitmap/concurrent.rs --json` 确认生产文件内 `Clone -> NewConcurrentBitmap`。对常见方法名的宽泛 callers 查询存在歧义，因此未将其结果当作调用事实。
- 源码与模块：`pkg/util/bitmap/concurrent.rs`、`pkg/util/bitmap/lib.rs`、`pkg/lib.rs`。
- crate 与接线：`pkg/util/bitmap/Cargo.toml`、根 `Cargo.toml`、`pkg/executor/join/Cargo.toml`。
- Go 对照：`pkg/util/bitmap/concurrent.go`、`pkg/util/bitmap/concurrent_test.go`、`pkg/util/bitmap/main_test.go`。
- Rust 独立测试：`pkg/util/bitmap/concurrent_test.rs`、`pkg/util/bitmap/migration_aster_unit_test.rs`。
- 生产调用检索：Rust 源码精确检索只发现根门面重导出以及 `pkg/executor/join/hash_table_v1.rs`、`hash_join_v1.rs` 中的注释化迁移草稿，未验证到活动的 SQL 主链调用。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前仅执行任务指定的 11 章节结构检查，并人工复核上述符号、边界和未接线声明。
