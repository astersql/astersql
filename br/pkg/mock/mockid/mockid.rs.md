# `br/pkg/mock/mockid/mockid.rs`

## 文件定位

[`mockid.rs`](mockid.rs) 是 workspace 成员 crate `astersql-br-pkg-mock-mockid` 的实现文件，crate 边界由 [`Cargo.toml`](Cargo.toml) 定义，并由 [`lib.rs`](lib.rs) 以 `pub mod mockid` 纳入、再通过 `pub use mockid::*` 平铺导出。它对齐 Go 包 [`mockid.go`](mockid.go)，为测试或脱离真实元数据服务的流程提供最小 ID 分配器，不接入 PD、TSO、存储或租约。

当前 Rust 接线需要明确区分：根 [`Cargo.toml`](../../../../Cargo.toml) 只将该 crate 列为 workspace 成员；全库 Rust 引用搜索未发现其他 crate 依赖它。`br/cmd/br/debug.rs` 中的 `mockid::NewIDAllocator` 实际解析到 `br/cmd/br/stubs.rs` 内的同名本地模块，不是本文件。因此，本实现当前已验证的 Rust 调用面是本 crate 的独立对等测试 [`parity_test.rs`](parity_test.rs)，不应把 Go 主链或 BR 本地桩的用途误记为已接入此 crate。

## 核心职责

- 用 `IDAllocator { base: AtomicU64 }` 维护每个分配器实例的独立计数状态。
- 用 `NewIDAllocator()` 创建从 `base = 0` 开始的实例，保证首次 `Alloc()` 返回 `1`。
- 用 `IDAllocator::Alloc()` 实现线程安全的“先加一、再返回”语义，包括 `u64::MAX -> 0` 的 Go 无符号整数回绕行为。
- 用 `IDAllocator::Rebase()` 保留 Go mock 的接口形状；该方法不读写 `base`，也不联系外部服务。

这是一个精确复制 Go mock 契约的小型实现，不是真实 ID 分配系统的简化替代品。

## 主要符号

- `pub struct IDAllocator { base: AtomicU64 }`：公开类型，但唯一字段 `base` 私有。`AtomicU64` 使 `Alloc(&self)` 能在共享引用上更新状态，无需 `&mut self` 或外部锁。
- `pub fn NewIDAllocator() -> IDAllocator`：公开构造函数，返回值类型而非 Go 的指针；调用者需共享时可自行放入 `Arc`。命名保留 Go API 的大写形式，`lib.rs` 对此允许非 snake case。
- `pub(crate) fn new_id_allocator_with_base(base: u64) -> IDAllocator`：仅在 `cfg(test)` 下编译的 crate 内辅助函数；它允许测试直接将计数器放到 `u64::MAX`，而不污染公开 API。
- `pub fn Alloc(&self) -> Result<u64, Infallible>`：用 `fetch_add(1, Ordering::SeqCst)` 原子取回旧值，再用 `wrapping_add(1)` 计算要返回的新值。`Infallible` 表示当前实现没有错误分支。
- `pub fn Rebase(&self) -> Result<(), Infallible>`：恒返回 `Ok(())` 的无操作接口。

本文件没有 trait、枚举、模块级常量、异步函数或除 `cfg(test)` 以外的条件编译项。

## 执行流程

1. 调用者调用 `NewIDAllocator()`，函数以 `AtomicU64::new(0)` 创建独立实例。
2. 每次调用 `Alloc()` 时，`fetch_add(1, SeqCst)` 在一个原子读-改-写操作中增加 `base`，并返回增加前的值。
3. 函数对旧值执行 `wrapping_add(1)`，得到与 Go `atomic.AddUint64` 相同的“增加后值”。正常时序列为 `1, 2, 3, ...`；旧值为 `u64::MAX` 时返回 `0`。
4. 结果包装为 `Ok(id)` 返回，没有 I/O、重试、日志或错误转换。
5. 调用 `Rebase()` 时直接返回 `Ok(())`；后续 `Alloc()` 从原计数继续。

Go 的已知业务用法见 `br/cmd/br/debug.go`：为表、索引和分区生成模拟新 ID，用于构建 rewrite rules。Rust 对应的 `br/cmd/br/debug.rs` 目前走同样流程，但其分配器来自 `br/cmd/br/stubs.rs`，不构成对本文件的调用边。

## 数据与状态

`base` 是全部持久状态，作用域限于单个 `IDAllocator` 实例。新建两个实例会得到两个从 `0` 开始的原子计数器，因此它们可以各自返回 ID `1`；实例之间不保证全局唯一。

计数以 `u64` 保存，没有保留值或上界拒绝逻辑。达到 `u64::MAX` 后下一次分配会返回 `0`，再从 `1` 继续。这是经 [`parity_test.rs`](parity_test.rs) 验证的 Go 对等契约，不是溢出错误。

`IDAllocator` 没有自定义 `Clone`、`Default` 或序列化实现。若要在线程间共享同一计数状态，应共享同一实例（现有测试使用 `Arc<IDAllocator>`），而不是构造新实例。

## 依赖与调用关系

- 下游依赖只有 Rust 标准库：`std::sync::atomic::{AtomicU64, Ordering}` 以及返回类型中的 `std::convert::Infallible`。[`Cargo.toml`](Cargo.toml) 没有 `[dependencies]` 或 feature 声明。
- crate 内上游是 [`lib.rs`](lib.rs)：它声明 `#[path = "mockid.rs"] pub mod mockid` 并再导出公开符号。
- 已验证的 Rust 调用者是 [`parity_test.rs`](parity_test.rs)：`go_rust_public_contract_matches` 调用 `NewIDAllocator`、`Alloc` 和 `Rebase`，`alloc_wraps_after_u64_max_like_go_atomic_add` 额外调用测试专用构造函数。
- RustCodeGraph 将目标文件索引为 6 个符号，并将 `new_id_allocator_with_base -> alloc_wraps_after_u64_max_like_go_atomic_add` 标识为调用边。因 `IDAllocator`/`Alloc` 在全库有多个同名符号，全库引用范围又用限定 Rust 文件搜索核对。
- 与应用主链的边界：目标 crate 目前只是 workspace 成员，未见其他 Cargo manifest 引用 `astersql-br-pkg-mock-mockid`；`br/cmd/br/debug.rs` 依赖同名 stubs 模块。

## 错误处理与边界

`Alloc()` 和 `Rebase()` 用 `Result<_, Infallible>` 保留“可返回错误”的 API 形状，但当前不存在 `Err` 构造路径。这对应 Go 实现中 `Alloc` 恒返回 `nil` error、`Rebase` 直接返回 `nil`。调用者可以用 `?` 或 `unwrap()` 处理结果，但不应依赖某个尚不存在的错误种类。

主要边界如下：

- 首次分配返回 `1`，而非内部初值 `0`。
- `Rebase()` 不重置、不前移也不验证计数器。
- `u64` 溢出是显式回绕，不会因 debug/release 编译模式不同而 panic。
- 唯一性只在“同一实例、回绕前且不重用已发放值”的边界内成立；新实例和整数回绕都可产生重复 ID。
- 没有输入参数、取消、超时、持久化失败或远程错误路径。

## 并发与资源生命周期

`Alloc(&self)` 使用 `AtomicU64::fetch_add` 实现无锁计数，`Ordering::SeqCst` 给出顺序一致的全局原子操作顺序。对同一实例的并发调用不会丢失更新；[`parity_test.rs`](parity_test.rs) 用 `Arc` 在 8 个线程中各分配 100 次，并验证 800 个结果去重后数量不变。

类型未实现自定义 `Drop`，也不拥有线程、任务、通道、锁守卫、文件、网络连接或事务。实例被丢弃时只需正常回收其 `AtomicU64`；装入 `Arc` 时由最后一个强引用负责销毁。`Rebase()` 不创建新资源也不改变生命周期。

## 与 Go 版本的对应关系

Rust 的 `IDAllocator` 、`NewIDAllocator`、`Alloc` 和 `Rebase` 逐一对应 [`mockid.go`](mockid.go) 中的同名符号。关键一致点是：初始 base 为 0，`Alloc` 原子加一并返回新值，无符号溢出按模 $2^{64}$ 回绕，两个方法均不返回实际错误，`Rebase` 为空操作。

已验证的形式差异包括：

- Go 字段是 `uint64`，并在方法中对其执行 `atomic.AddUint64`；Rust 直接将字段定义为 `AtomicU64`。
- Go 构造函数返回 `*IDAllocator`；Rust 返回拥有权值 `IDAllocator`，共享时由调用者包装 `Arc`。
- Go `Alloc` 返回 `(uint64, error)`；Rust 用 `Result<u64, Infallible>` 表达相同的“值加不可达错误”契约。
- Go 溢出由无符号算术自然回绕；Rust 必须对 `fetch_add` 返回的旧值显式调用 `wrapping_add(1)`，以避免普通加法的溢出检查差异。
- Rust 额外提供仅测试可见的 `new_id_allocator_with_base`，用于以常数时间覆盖 Go 回绕边界；它不是 Go 包或 Rust 公开 API 的扩展。

Go 同目录未发现独立 `mockid_test.go`；Rust 对等覆盖集中在独立文件 [`parity_test.rs`](parity_test.rs)。

## 扩展指南

- 修改起始值、递增步长或回绕策略时，接入点是 `NewIDAllocator()` 与 `IDAllocator::Alloc()`；必须先核对 Go `mockid.go` 的契约，并同步更新 [`parity_test.rs`](parity_test.rs) 的顺序、多实例、回绕和并发用例。
- 若要让 `Rebase` 真正改变状态，需先定义它与并发 `Alloc` 的线性化顺序、新 base 来源和错误契约；当前签名没有 base 参数，不应在缺少 Go 对照的情况下自行扩展。
- 若引入可失败的下游资源，`Infallible` 必须替换为具体错误类型，并在独立测试文件中覆盖错误传播和资源释放；不要把测试内嵌到 `mockid.rs`。
- 若目标是将这个 crate 接入 Rust BR 调试主链，需在消费 crate 的 Cargo manifest 中增加对 `astersql-br-pkg-mock-mockid` 的依赖，替换 `br/cmd/br/stubs.rs` 中的同名实现，并对 `Alloc` 的不同返回形状做必要的局部接线。这是未完成的后续工作，不是当前状态。
- 性能变更需关注 `SeqCst` 的竞争成本，但不应未经并发契约证明就放宽内存序。兼容风险集中在 Go/Rust 返回形状、回绕行为和命名形状。

## 验证依据

- RustCodeGraph `status`：索引包含 7032 个 Rust 文件；`files --filter br/pkg/mock/mockid` 列出 `lib.rs`、`mockid.rs`、`mockid.go` 和 `parity_test.rs`。
- RustCodeGraph `explore "br/pkg/mock/mockid/mockid.rs MockID NewMockID NewMockIDGenerator"`：返回目标源码、Go 对照、crate 入口和独立测试，并确认 `new_id_allocator_with_base` 由 `alloc_wraps_after_u64_max_like_go_atomic_add` 调用。
- RustCodeGraph `query IDAllocator --kind struct --limit 20` 与 `query NewIDAllocator --kind function --limit 20`：确认目标 Go/Rust 符号，同时暴露 `br/cmd/br/stubs.rs` 的同名实现，因此后续以文件限定搜索排除歧义。
- 逐文件阅读：[`mockid.rs`](mockid.rs)、[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`mockid.go`](mockid.go)、[`parity_test.rs`](parity_test.rs)、`br/cmd/br/debug.go`、`br/cmd/br/debug.rs` 和 `br/cmd/br/stubs.rs`。同目录没有 `doc.go` 或 Go 独立测试文件。
- `rg` 核对：全库 Cargo manifest 中只有根 workspace 成员列表和本 crate 自身出现包名；Rust 限定搜索显示目标公开符号只被 `lib.rs` 再导出、被 `parity_test.rs` 调用。
- 结构验证要求：文档必须存在，且上述十一个固定二级标题各出现一次。本任务是纯文档分析，按计划不运行 Cargo。
