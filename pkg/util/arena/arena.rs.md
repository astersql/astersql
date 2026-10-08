# `pkg/util/arena/arena.rs`

## 文件定位

本文件实现 `astersql-util-arena` crate 的核心逻辑：一个面向短生命周期字节缓冲的线性 arena 分配器，以及在 arena 容量不足时退回普通堆分配的兼容路径。crate 边界由 `pkg/util/arena/Cargo.toml` 定义，`pkg/util/arena/lib.rs` 公开 `arena` 模块并再导出本文件全部公开项；根 workspace 的 `Cargo.toml` 将它登记为成员并以 `facade_util_arena` 引入，`pkg/lib.rs::util::arena` 再做统一门面导出。

当前 Rust 接线状态需要与 Go 生产链区分：`pkg/server/Cargo.toml` 已声明 `astersql-util-arena` 依赖，但仓库内 Rust server 源码尚未检出对本 crate API 的调用；可确认的 Rust 调用者位于同 crate 的独立测试。Go 对照实现则由 `pkg/server/conn.go::clientConn.alloc` 实际用于连接级协议包缓冲，初始化容量为 32 KiB，并在命令处理循环中复位。

## 核心职责

- `Allocator` 统一描述三项操作：分配零可见长度缓冲、分配指定可见长度缓冲、复位复用位置。
- `SimpleAllocator` 预先持有一块连续 `Vec<u8>`，用 `off` 线性推进并返回互不重叠的借用区间，以减少重复堆分配。
- `stdAllocator` 提供不复用 arena 的基线实现，每次直接构造自有 `Vec`。
- `ArenaBuffer` 封装“可见长度”和“容量”，使 Rust 返回值接近 Go `[]byte` 的 `len/cap` 语义；arena 缓冲追加越界时转成自有 `Vec`，避免写出借用区间。
- `Reset` 只允许复用整块 arena，不负责清零，也不逐项释放。

以上职责来自 `Allocator::{Alloc, AllocWithLen, Reset}`、`SimpleAllocator`、`stdAllocator`、`ArenaBuffer` 及 `BufferStorage`；本文件没有事务、I/O 或数据库状态管理。

## 主要符号

- `enum BufferStorage<'a>`：内部存储判别。`Arena(&'a mut [u8])` 借用分配器的子切片，`Owned(Vec<u8>)` 持有独立堆缓冲；该类型不公开。
- `pub struct ArenaBuffer<'a>`：公开缓冲包装，保存 `storage`、可见 `len` 和逻辑 `capacity`。`from_arena`、`owned`、`set_len` 是内部构造/整形方法；`len`、`is_empty`、`capacity`、`is_arena_backed`、`as_ptr`、`as_mut_ptr` 提供观察接口；`push`、`extend_from_slice` 提供追加；`Deref<Target=[u8]>` 与 `DerefMut` 只暴露可见区间。
- `pub trait Allocator`：公开分配器协议。方法名保留 Go 风格：`Alloc(&mut self, capacity)`、`AllocWithLen(&mut self, length, capacity)`、`Reset(&mut self)`。
- `pub struct SimpleAllocator`：生产实现，公开字段 `arena: Vec<u8>` 与 `off: usize`；`NewAllocator` 以全零、固定长度的 `Vec` 初始化它。
- `pub struct stdAllocator`：类型公开但命名保持 Go 风格；`StdAllocator()` 每次返回一个无状态实例。
- `pub fn NewAllocator(capacity)`：创建 `SimpleAllocator`，`arena` 的长度和容量至少为请求值，`off` 为零。

文件没有常量、条件编译项或错误枚举。所有条件编译仅在 `lib.rs` 中用于挂载独立测试文件。

## 执行流程

1. 调用者通过 `NewAllocator(capacity)` 获得预分配器，或通过 `StdAllocator()` 获得始终走堆的实现。
2. `SimpleAllocator::Alloc` 使用 `checked_add` 计算 `off + capacity`。只有结果存在且严格小于 `arena.len()` 时，才借用 `arena[off..off+capacity]`，随后把 `off` 增加 `capacity`；等于末端、超出末端或整数溢出都返回 `ArenaBuffer::owned(capacity)`，并保持 `off` 不变。
3. `AllocWithLen` 先执行 `Alloc(capacity)`，再经 `ArenaBuffer::set_len(length)` 设置可见长度。自有存储会 `resize` 并以零填充新增可见字节；arena 存储只是扩大可见范围，因为 `NewAllocator` 初始已将整块 arena 填零，但 `Reset` 后再次分配可能看到旧内容。
4. 容量内 `ArenaBuffer::push` 原地写入 arena，或向尚未满的 owned `Vec` 追加，并推进 `len`。容量已满时，owned 路径交给 `Vec` 扩容；arena 路径创建容量至少为旧容量两倍（零容量时至少为一）的 `Vec`，复制可见字节、追加新字节，再切换为 `Owned`。
5. `extend_from_slice` 逐字节调用 `push`，因此保留相同迁移规则，但没有一次性预留优化。
6. 调用者确认此前返回的借用缓冲均不再使用后，可调用 `SimpleAllocator::Reset` 将 `off` 归零，让后续分配覆盖并复用旧区域。

Rust 借用规则使一个 arena-backed `ArenaBuffer` 存活期间不能再次可变借用同一 `SimpleAllocator`；测试通过显式结束作用域或 `drop` 后再分配体现了这一点。

## 数据与状态

`SimpleAllocator` 的持久状态只有连续字节池 `arena` 和下一个分配起点 `off`。成功的 arena 分配满足区间为 `[旧 off, 旧 off + capacity)`，新 `off` 等于区间末端；回退分配不改变二者。严格的 `< arena.len()` 判断会永久保留最后一个边界位置：请求恰好到达 arena 末端也不使用 arena，这是对 `arena.go::SimpleAllocator.Alloc` 中 `s.off+capacity < cap(s.arena)` 的刻意复刻。

`ArenaBuffer` 将存储所有权与切片形状分开记录。对 arena 存储，底层切片长度就是分配容量，而 `len` 控制 `Deref` 可见范围；对 owned 存储，`Vec::len()` 与 `len` 同步，`capacity` 在追加扩容后同步为 `Vec::capacity()`。从 arena 迁移到 owned 后，不再引用原 arena，但 `SimpleAllocator.off` 不回退，原区间只会在 `Reset` 后整体复用。

零初始化并非跨复位不变量：首次 `NewAllocator` 和新建 owned 缓冲的可见扩展为零；arena 区域被写入、缓冲释放、再 `Reset` 后，`AllocWithLen` 会重新暴露旧字节。`migration_aster_unit_test.rs::simple_allocator_reuses_the_preallocated_arena` 明确验证了这一行为。

## 依赖与调用关系

下游依赖仅为 Rust 标准库：`std::ops::{Deref, DerefMut}` 和 `Vec`。`pkg/util/arena/Cargo.toml` 没有运行时第三方依赖；唯一 dev-dependency `astersql-testkit-testsetup` 由 `main_test.rs` 使用，而非本文件使用。

RustCodeGraph 将本文件识别为 30 个符号，并能定位 `Allocator`、`SimpleAllocator`、`ArenaBuffer`、`NewAllocator`、`StdAllocator` 及各实现。对这些重名/trait 方法执行精确 `callers`、`callees` 查询没有产生调用边，因此调用关系进一步由直接仓库搜索核验：

- `pkg/util/arena/lib.rs` 公开并再导出本模块。
- 根 `Cargo.toml` 的 workspace/member 与 `facade_util_arena` 依赖、`pkg/lib.rs::util::arena` 构成上层门面。
- `pkg/server/Cargo.toml` 声明此 crate，但当前 Rust server 源码没有检出符号调用，不能据此宣称生产 Rust 主链已接线。
- Rust 直接行为调用者是 `arena_test.rs` 和 `migration_aster_unit_test.rs`。
- Go 生产调用链是 `pkg/server/conn.go::newClientConn -> arena.NewAllocator`，之后多处协议编码调用 `cc.alloc.AllocWithLen`，命令循环调用 `cc.alloc.Reset`。

## 错误处理与边界

该 API 不返回 `Result`。可恢复的容量不足通过 `Owned(Vec<u8>)` 回退，不视为错误；`off + capacity` 的整数溢出由 `checked_add` 转入同一回退路径，因此不会因加法溢出访问 arena。

`ArenaBuffer::set_len` 要求 `length <= capacity`，否则以包含长度和容量的消息触发 panic。因而 `AllocWithLen(length, capacity)` 也具有相同前置条件。内存申请失败仍遵循标准 `Vec` 的分配失败行为，本文件没有捕获机制。

边界 `off + capacity == arena.len()` 明确回退 owned，且不推进 `off`；`capacity == 0` 在非满 arena 时可能产生零长 arena-backed 缓冲且不推进 `off`。`push` 对满的零容量 arena 缓冲会分配至少容量一的 owned `Vec`。原始指针访问器只返回指针而不执行解引用；安全切片访问仍受 `Deref/DerefMut` 的可见长度限制。

## 并发与资源生命周期

`Allocator` 的方法都要求 `&mut self`，注释也明确分配器非线程安全；本文件没有锁、原子、通道、异步任务或后台资源。若跨线程共享，调用者必须自行建立互斥与生命周期边界。

arena-backed `ArenaBuffer<'a>` 的生命周期绑定到对 `SimpleAllocator` 的可变借用，这在类型层面阻止缓冲仍存活时调用 `Reset` 或再次分配，从而强化 Go 注释“确保已分配内存不再使用”的约束。缓冲析构不调整 `off`；只有显式 `Reset` 才整体回收。owned 缓冲的字节存储按普通 `Vec` 独立释放，但公开方法统一返回 `ArenaBuffer<'_>`，所以即使运行时已迁移为 owned，静态生命周期仍不会被重新放宽为可越过创建它的分配器。

性能上，arena 分配为常数时间切片和偏移推进；回退需要单独堆分配；arena 缓冲首次越界追加需要复制全部可见字节。`extend_from_slice` 的逐字节实现可能多次检查/扩容，批量场景若优化必须维持迁移和 `len/cap` 不变量。

## 与 Go 版本的对应关系

`pkg/util/arena/arena.go` 是直接语义基线：Rust `Allocator` 对应 Go 接口，`SimpleAllocator`/`stdAllocator`、`NewAllocator`、三项 trait 方法分别对应同名 Go 定义。严格小于容量才走 arena、回退不推进 `off`、`Reset` 只归零偏移均保持一致。

关键表示差异如下：

- Go 以三索引切片 `arena[off:off:off+capacity]` 表达长度零、固定容量；Rust 不能直接返回同形态的裸切片，因此以 `ArenaBuffer { len, capacity, storage }` 模拟。
- Go 的 `append` 可自然迁移到底层新数组；Rust 由 `ArenaBuffer::push` 和 `extend_from_slice` 显式完成 arena 到 owned 的复制迁移。
- Go `StdAllocator` 是包级单例变量；Rust 是构造函数 `StdAllocator()`，返回零状态 `stdAllocator`。行为等价，但实例身份不同。
- Go 的错误长度切片会运行时 panic；Rust `set_len` 用显式断言实现同类前置条件。
- Go 可在旧切片仍被引用时误用 `Reset`；Rust 的 `&mut` 借用和 `ArenaBuffer<'a>` 通常在编译期阻止该用法。

`arena_test.go` 与 `arena_test.rs` 覆盖基础形状和偏移语义；Rust 额外的 `migration_aster_unit_test.rs` 覆盖指针连续性、严格边界回退、复位后旧字节复用、追加迁移和 owned 零填充。

## 扩展指南

- 修改分配判定或偏移策略时，入口是 `SimpleAllocator::Alloc`。必须同步 `arena_test.rs::test_simple_arena_allocator` 与 `migration_aster_unit_test.rs::simple_allocator_matches_go_fallback_boundaries`，并确认是否有意偏离 Go 的严格 `<` 边界。
- 修改可见长度、追加或扩容策略时，入口是 `ArenaBuffer::{set_len, push, extend_from_slice}` 及 `Deref/DerefMut`。应扩展独立测试，覆盖零容量、恰好满容量、迁移后数据保持、容量增长和多次追加；测试逻辑不得内嵌回生产文件。
- 修改复用/清零政策时，入口是 `SimpleAllocator::Reset` 和 `NewAllocator`。必须考虑 Go 兼容性、复位后旧字节是否可见，以及在 server 协议包场景中额外清零的性能成本。
- 新增线程共享能力不能只增加 `Send/Sync` 声明；需要重新设计可变借用、偏移同步和返回缓冲生命周期，并评估锁竞争。当前单连接、顺序复用模型不提供并发承诺。
- 将 Rust server 真正接入此 crate 时，应从 Go 的 `clientConn.alloc` 生命周期核对：连接创建、每个协议编码点、写出完成与 `Reset` 时机；仅保留 Cargo 依赖不能视为完成接线。
- API 命名若 Rust 化会影响 Go 对齐和门面用户；变更前需搜索 `pkg/lib.rs::util::arena` 的外部消费者，并保留兼容层或明确迁移计划。

## 验证依据

- 源码：`pkg/util/arena/arena.rs`，RustCodeGraph `node --file ... --offset 1/238` 覆盖全部 270 行；索引报告该文件有 30 个符号。
- 图查询：RustCodeGraph `explore`、对 `Arena`/`NewAllocator`/`StdAllocator`/`AllocWithLen`/`Reset` 的 `query`，以及关键符号的 `callers/callees`；后两类调用边查询无输出，故未把图中“used by 318 files”的粗粒度文件关系解释为真实 API 调用。
- crate/装配：`pkg/util/arena/Cargo.toml`、`pkg/util/arena/lib.rs`、根 `Cargo.toml`、`pkg/lib.rs::util::arena`、`pkg/server/Cargo.toml`。
- Go 对照与生产调用：`pkg/util/arena/arena.go`、`pkg/server/conn.go::{newClientConn, clientConn}` 以及其中 `cc.alloc.AllocWithLen`/`Reset` 调用点。
- 独立测试：`pkg/util/arena/arena_test.rs`、`pkg/util/arena/migration_aster_unit_test.rs`、`pkg/util/arena/arena_test.go`；`main_test.rs` 仅负责测试初始化，不是分配逻辑证据。
- 人工复核结论：本文件存在是为了为短生命周期字节缓冲提供连接/请求级线性复用模型；运行时在 arena 借用与 owned 回退间切换；安全扩展必须维护长度不超过容量、成功分配区间不重叠、回退不推进偏移、复位前旧借用已结束四项约束。
