# `pkg/lightning/manual/allocator.rs`

## 文件定位

本文件属于 `astersql-lightning-manual` crate；crate 根在 `pkg/lightning/manual/lib.rs`，其中声明 `allocator` 模块并以 `pub use allocator::Allocator` 将本文件的类型提升为 crate 级 API。`pkg/lightning/manual/Cargo.toml` 指定库入口为 `lib.rs`、Go 包映射为 `pkg/lightning/manual`，并声明空的默认 feature 与 `cgo` feature；本文件自身没有条件编译分支，实际分配函数固定通过 `super::{New, Free}` 取得 crate 根再导出的 `manual.rs` 实现。

它是 Go `pkg/lightning/manual/allocator.go` 的 Rust 对照文件，提供“字节缓冲分配/释放 + 可选未释放计数”的小型门面。当前 Rust 生产代码中未检索到 `Allocator`、`Alloc`、`Free` 或 `CheckRefCnt` 的直接调用；已确认的使用者是同 crate 的独立测试 `allocator_test.rs` 与 `migration_aster_unit_test.rs`。因此它目前是已实现、已测试、已公开，但尚未按 Go 主链接入 Rust ingest 控制面的迁移边界，而不是 Lightning 导入主链已经在使用的分配器。

## 核心职责

- `Allocator::Alloc` 在每次分配前可选地把共享计数加一，再调用 `manual::New` 创建指定长度、内容清零的 `Vec<u8>`。
- `Allocator::Free` 在每次释放前可选地把共享计数减一，再把 `Vec<u8>` 的所有权交给 `manual::Free`；后者通过 `drop` 释放缓冲区。
- `Allocator::CheckRefCnt` 把非零计数转换为稳定的泄漏诊断字符串；未启用计数或计数为零时返回成功。
- `Allocator` 的默认值关闭计数，克隆则共享同一个 `AtomicI64`。这分别对应 Go 结构体零值中 `RefCnt == nil` 和按值复制结构体但保留同一计数器指针的语义。

本类型只记录“调用了多少次 `Alloc` 尚未被对应的 `Free` 抵消”，不记录字节数、缓冲区身份、容量或所有权来源，也不实现池化、复用或双重释放检测。

## 主要符号

- `pub struct Allocator { pub RefCnt: Option<Arc<AtomicI64>> }`：文件唯一类型，也是唯一公开状态。`Option::None` 表示关闭统计；`Some` 允许多个 `Allocator` 克隆共享计数。字段公开，调用方可以自行安装、读取或共享计数器。
- `#[derive(Clone, Default)]`：`Default` 生成 `RefCnt: None`；`Clone` 克隆 `Arc` 而不是复制当前数值，因此克隆实例属于同一个计数域。
- `pub fn Alloc(&self, n: isize) -> Vec<u8>`：先以 `fetch_add(1, Ordering::SeqCst)` 记账，再调用 `New(n)`。参数类型为 `isize`，负值及超过 `MaxArrayLen` 的值由 `manual.rs::New` 拒绝。
- `pub fn Free(&self, bytes: Vec<u8>)`：先以 `fetch_sub(1, Ordering::SeqCst)` 记账，再调用 `Free(bytes)`。按值接收缓冲区，使同一 `Vec` 无法在安全 Rust 中再次传入。
- `pub fn CheckRefCnt(&self) -> Result<(), String>`：对已启用的计数器先判断一次；非零时再次读取当前值并返回 `Err("memory leak detected, refCnt: {count}")`，对应 Go 判断后再次 `Load` 再格式化错误的实现。

本文件没有模块级常量、trait、自由函数、泛型、宏、异步函数或条件编译项。

## 执行流程

分配流程如下：调用方持有 `Allocator`；若 `RefCnt` 为 `Some`，`Alloc` 先顺序一致地增加共享计数；随后 `manual::New` 将 `isize` 转换为 `usize`、检查 `MaxArrayLen`，并返回长度为 `n` 的清零 `Vec<u8>`。零长度直接返回空 `Vec`。

释放流程如下：调用方把 `Vec<u8>` 的所有权移入 `Allocator::Free`；若启用了计数，方法先顺序一致地减一；随后 `manual::Free` 调用 `drop`。计数变化描述的是 API 调用配对，不等待析构完成，也不依据缓冲区容量调整。

检查流程如下：`CheckRefCnt` 对 `None` 直接成功；对 `Some` 读取当前值，零值成功，非零值则再次读取并构造错误。由于两次读取之间可能有并发分配或释放，错误文本中的数值是第二次读取时的快照，不保证等于触发非零分支时的数值。

## 数据与状态

唯一持久状态是 `Option<Arc<AtomicI64>>`。计数的单位是尚未配对的分配调用次数，而非活跃字节数。正常约定是不启用计数时始终保持 `None`；测试或诊断场景用从零开始的 `AtomicI64`。`allocator_test.rs::allocator_preserves_go_zero_value_and_value_copy_contract` 证明默认实例不计数，并证明原实例分配后可由克隆实例释放、共享计数最终归零。

实现没有阻止计数变负：在未先调用 `Alloc` 的情况下调用 `Free` 会执行 `fetch_sub`。`CheckRefCnt` 将任何非零值（包括负数）都报告为泄漏。这是与 Go 文件相同的计数器契约，而不是缓冲区所有权验证器。

`Alloc` 的计数先于真实分配发生。因此如果 `New(n)` 因负长度、长度越界或分配失败而 panic，计数已经增加且不会由本方法回滚。类似地，`Free` 在实际 `drop` 前减计数；安全 Rust 中 `drop(Vec<u8>)` 不返回错误。扩展代码不能把该计数误当作事务性资源账本。

## 依赖与调用关系

下游只有标准库同步原语和同 crate 分配函数：`Arc` 负责共享所有权，`AtomicI64` 负责跨线程计数，`Ordering::SeqCst` 保持 Go 原子操作的顺序一致语义，`super::New`/`super::Free` 最终落到 `pkg/lightning/manual/manual.rs`。`New` 负责范围检查及清零分配，`Free` 负责取得并释放 `Vec` 所有权。

模块入口 `pkg/lightning/manual/lib.rs` 同时公开 `Allocator`、`New`、`Free` 和 `MaxArrayLen`。工作区根 `Cargo.toml` 将该 crate 列为成员并提供 `facade_lightning_manual` 别名；`pkg/ingestor/ingestctrl/Cargo.toml` 仅在 Windows 目标依赖区声明它。然而当前 `pkg/ingestor/ingestctrl/engine_mgr.rs` 没有使用该依赖或 `Allocator`。

Go 对照主链更完整：`pkg/ingestor/ingestctrl/engine_mgr.go::newEngineManager` 构造 `manual.Allocator`，测试模式安装原子计数器并保存到 `LastAlloc`，非内存测试模式通过 `membuf.WithAllocator` 注入缓冲池。当前 Rust `newEngineManager` 只构造引擎注册表、重复数据缓冲等状态，没有等价的 allocator/buffer-pool 接线；因此该 Go 调用边只能作为迁移目标证据，不能视为 Rust 当前调用边。

## 错误处理与边界

`CheckRefCnt` 是本文件唯一显式返回错误的 API，错误类型是 `String`，消息格式与 Go 保持一致。它不清零计数、不释放资源，也不区分“真正泄漏”、漏调 `Free`、多调 `Free` 或检查时仍有合法并发使用者。

长度边界由 `manual.rs::New` 管理：`n == 0` 成功；负数在 `usize::try_from` 处以 `"makeslice: len out of range"` panic；大于 `MaxArrayLen` 的长度由断言以同一消息 panic。因为 `Alloc` 先加计数，捕获 panic 后继续使用分配器会看到非零计数。极端内存不足仍遵循 Rust 分配器的失败行为，本文件不提供可恢复错误。

`Free` 接受任何 `Vec<u8>`，无法证明该缓冲区来自同一个 `Allocator`；用 A 分配、用 B 释放会分别改变不同计数域。传入外部构造的 `Vec` 也会减计数。调用者必须自行维持“同一计数域、一次分配对应一次释放”的不变量。

## 并发与资源生命周期

`Allocator` 可因 `Arc<AtomicI64>` 而安全地在多个线程间克隆和共享计数；三类原子操作均使用 `Ordering::SeqCst`。`allocator_test.rs::allocator_uses_go_sequentially_consistent_atomic_operations` 通过源码契约检查禁止退化为 `Relaxed`，并核对一次加、一次减和两次读取。

原子计数只保证单次计数操作的同步，不把“分配—使用—释放—检查”组合成临界区。并发调用 `CheckRefCnt` 只能获得瞬时观察：第一次读取决定分支，第二次读取决定消息内容；其他线程可以在两者之间改变计数。需要稳定的最终泄漏结论时，上层必须先停止新分配并等待所有缓冲使用者完成，再调用检查。

缓冲区生命周期由 `Vec<u8>` 所有权控制：`Alloc` 返回所有权，`Free` 消耗所有权并立即 `drop`。如果调用方直接丢弃 `Vec` 而不经 `Allocator::Free`，内存仍会由 Rust 正常释放，但可选计数不会减少，随后检查会报告泄漏；这正是该诊断层要捕获的 API 配对遗漏。

## 与 Go 版本的对应关系

Rust `Allocator` 对应 `pkg/lightning/manual/allocator.go::Allocator`；`Option<Arc<AtomicI64>>` 对应可为空的 `*atomic.Int64`。`Alloc`、`Free` 和 `CheckRefCnt` 保留了 Go 的操作顺序、零值行为、共享计数器以及错误文本。Rust 使用 `Result<(), String>` 表示 Go 的 `error`，使用 `Vec<u8>` 所有权替代 Go `[]byte` 的显式手动释放约定。

主要语言差异是：Rust `Free(Vec<u8>)` 消耗值，能在编译期阻止对同一变量的安全重复释放；Go 切片可复制，API 本身无法阻止重复调用。反过来，两种实现都无法验证释放操作使用了正确的 allocator。Rust 的 `Alloc` 参数为 `isize`，Go 为 `int`，两者均采用本机字长的有符号整数；真实 Rust 长度上限还由 `manual::MaxArrayLen` 固定为 `(1 << 31) - 1`。

迁移状态并非完整主链接线：Go `engine_mgr.go` 将此分配器注入 `membuf` 并在测试模式暴露最后一个计数器；Rust 对照的 `engine_mgr.rs` 尚无这些字段与调用。文档只确认本文件内部语义与独立测试对齐，不声称 Rust Lightning 导入过程已经通过该类型管理缓冲。

## 扩展指南

若增加字节统计、峰值统计或分配标签，应优先扩展 `Allocator` 的共享状态及 `Alloc`/`Free` 的成对更新，并明确克隆实例是否共享同一统计域；同步更新独立的 `pkg/lightning/manual/allocator_test.rs`，需要覆盖零值、克隆、失败分配、错误消息和并发检查边界。不要把测试内嵌回生产源文件。

若要完成 Go 主链接线，应在独立迁移任务中核对 `pkg/ingestor/ingestctrl/engine_mgr.go::newEngineManager`、Rust `engine_mgr.rs`、`astersql-lightning-membuf` 的 allocator 接口以及 `LastAlloc` 对应测试需求；不能只因 Cargo 中已有依赖就假定接口兼容。接线还需决定非 Windows 依赖范围，因为当前 ingestctrl 对 manual crate 的声明位于 Windows target 段。

若改变原子排序，必须先证明不再需要 Go `go.uber.org/atomic` 的顺序一致契约，并修改 `allocator_uses_go_sequentially_consistent_atomic_operations`；仅为性能把它降为 `Relaxed` 会破坏现有迁移契约。若让 `Alloc` 返回可恢复错误，还需处理“先计数、后分配”的回滚，并评估与 Go panic 行为及现有调用签名的兼容性。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标目录的 `allocator.rs`、`allocator_test.rs`、`lib.rs`、`manual.rs` 及 Go 对照均已索引。
- RustCodeGraph `node --file pkg/lightning/manual/allocator.rs`：确认文件共 67 行，唯一类型为 `Allocator`，方法为 `Alloc`、`Free`、`CheckRefCnt`，并确认其原子操作和对 `New`/`Free` 的委托。
- RustCodeGraph `node`：读取 `pkg/lightning/manual/lib.rs`、`manual.rs`、`allocator_test.rs`、`migration_aster_unit_test.rs` 与 `allocator.go`，核对公开边界、分配语义、零值/克隆/顺序一致性/泄漏错误测试及 Go 实现。精确 `callers`/`callees` 查询受同名符号消歧限制，没有得到可信的方法级调用边，因此未把其模糊结果用作结论。
- Cargo 与入口：读取 `pkg/lightning/manual/Cargo.toml`、工作区根 `Cargo.toml` 和 `pkg/ingestor/ingestctrl/Cargo.toml`，确认 crate 身份、workspace/facade 登记、feature，以及 Windows 目标依赖声明。
- Go/Rust 接线对照：读取 `pkg/ingestor/ingestctrl/engine_mgr.go::newEngineManager` 和 Rust `engine_mgr.rs::newEngineManager`，并用仓库文本搜索确认 Rust 生产文件当前没有 `Allocator` 直接引用，从而界定“已实现但未接入对应主链”的迁移状态。
- 独立测试证据：`allocator_test.rs` 覆盖默认零值、克隆共享计数和 `SeqCst` 源码契约；`migration_aster_unit_test.rs` 覆盖分配加计数、泄漏错误、释放减计数、禁用计数和清零分配。按任务要求这是纯文档分析，未运行 Cargo。
