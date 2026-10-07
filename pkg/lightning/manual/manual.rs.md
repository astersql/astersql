# `pkg/lightning/manual/manual.rs`

源文件：[`manual.rs`](./manual.rs)

## 文件定位

本文件属于 `astersql-lightning-manual` crate（`pkg/lightning/manual/Cargo.toml`），实现手动字节缓冲 API 的默认 Rust 版本。crate 根 `pkg/lightning/manual/lib.rs` 通过 `pub mod manual` 装载本模块，并以 `pub use manual::{Free, MaxArrayLen, New}` 将三个公开符号提升到 crate 根。它是 Lightning 内存缓冲基础设施的一层小型兼容接口，不负责缓存、复用或引用计数；这些策略由相邻的 `allocator.rs` 和上层缓冲池实现。

Cargo 清单声明了空的 `default` feature 和 `cgo` feature，但当前 `lib.rs`、`manual.rs` 均没有 `#[cfg(feature = "cgo")]` 分支。`manual_nocgo.rs` 也始终被声明为另一个公开模块，而不是在编译期替换本文件。因此，不能把 Go 的 cgo/非 cgo 文件选择机制视为当前 Rust crate 已接线的行为。

## 核心职责

- `MaxArrayLen` 给分配入口设定与 Go 常量相同的上限：`2^31 - 1`。
- `New` 将有符号长度转换为 `usize`，拒绝负数和超过上限的长度，并创建内容全为零的 `Vec<u8>`。
- `Free` 消费 `Vec<u8>` 的所有权并立即 `drop`，把显式释放点保留在迁移后的调用形状中。

本文件只提供“创建缓冲区/结束缓冲区生命周期”的原语。它不直接使用 C 分配器、不持有全局状态，也不记录泄漏计数；`Allocator` 的计数逻辑位于 `pkg/lightning/manual/allocator.rs`。

## 主要符号

- `pub const MaxArrayLen: usize = (1usize << 31) - 1`：公开长度上限。它与 `pkg/lightning/manual/manual.go` 的 `MaxArrayLen` 数值一致，但 Rust 类型为 `usize`。
- `pub fn New(n: isize) -> Vec<u8>`：公开分配入口。`n == 0` 时显式返回 `Vec::new()`；其他值先用 `usize::try_from` 检查负数，再以 `assert!` 检查 `MaxArrayLen`，最后执行 `vec![0; len]`。
- `pub fn Free(bytes: Vec<u8>)`：公开释放入口。参数按值传入，调用 `drop(bytes)` 后底层分配在没有其他所有者的正常 `Vec` 所有权模型下释放。

名称保留 Go 风格的大写形式；`pkg/lightning/manual/lib.rs` 在 crate 级允许 `non_snake_case` 和 `non_upper_case_globals`，所以这些符号无需局部 lint 抑制。

## 执行流程

`New` 的控制流如下：

1. 收到 `isize` 长度；若为零，直接返回长度和容量均为零的空 `Vec`，不进入范围转换。
2. 对非零值执行 `usize::try_from(n)`。负数无法转换，会以 `makeslice: len out of range` 为期望消息触发 panic。
3. 断言转换后的长度不超过 `MaxArrayLen`；超限同样以 `makeslice: len out of range` panic。
4. 使用 `vec![0; len]` 构造缓冲区，元素初始化为零并返回所有权。

`Free` 没有分支：调用者把缓冲区所有权移入函数，函数立即丢弃它。直接生产调用链为 `Allocator::Alloc -> New` 和 `Allocator::Free -> Free`（`pkg/lightning/manual/allocator.rs`）；crate 根再导出也允许其他依赖方直接调用，但限定 Rust 引用搜索未发现仓库内其他直接调用点。

## 数据与状态

本模块没有结构体、静态可变变量、锁或缓存。唯一模块级数据是编译期常量 `MaxArrayLen`。

缓冲状态完全由返回的 `Vec<u8>` 携带：长度决定可见字节数，容量代表其后备存储。`New` 初始化所有可见字节为零。`Free` 接受任意拥有所有权的 `Vec<u8>`，因此也能释放“长度已截为零但容量仍非零”的缓冲；`migration_aster_unit_test.rs::free_accepts_a_zero_length_slice_with_backing_storage` 专门覆盖此边界。

## 依赖与调用关系

本文件只依赖 Rust 标准库和预导入项：`usize::try_from`、`Vec`、`vec!`、`assert!` 与 `drop`，`pkg/lightning/manual/Cargo.toml` 没有普通第三方依赖。

模块上游关系是：

- `pkg/lightning/manual/lib.rs` 声明模块并再导出 `New`、`Free`、`MaxArrayLen`。
- `pkg/lightning/manual/allocator.rs::Allocator::Alloc` 在可选计数加一后调用 crate 根再导出的 `New`。
- `pkg/lightning/manual/allocator.rs::Allocator::Free` 在可选计数减一后调用 crate 根再导出的 `Free`。
- `pkg/lightning/manual/migration_aster_unit_test.rs` 直接验证这两个 API；`allocator_test.rs` 通过 `Allocator` 间接覆盖它们。

工作区根 `Cargo.toml` 以 `facade_lightning_manual` 登记本 crate；`pkg/ingestor/ingestctrl/Cargo.toml` 仅在 Windows target 依赖列表中声明它。限定源码搜索未找到该依赖在 `ingestctrl` Rust 源码中的实际引用，因此不能据此声称当前 Rust 导入主链已调用本模块。Go 主链的直接证据位于 `pkg/lightning/backend/kv/session.go`：`newBytesBuf` 调用 `manual.New`，`BytesBuf.destroy` 调用 `manual.Free`。

## 错误处理与边界

接口不返回 `Result`。以下失败通过 panic 或进程级分配失败表现：

- 负数在 `usize::try_from` 的 `expect` 处 panic。
- 大于 `MaxArrayLen` 的非负数在断言处 panic。
- 合法但无法满足的巨大分配遵循 Rust `Vec`/全局分配器的失败策略；本文件没有恢复、降级或错误包装。

零长度是正常输入，返回 `Vec::new()`。`Free` 对空 `Vec` 和保留容量的零长度 `Vec` 都安全，因为它消费整个容器而不是索引首元素。与 Go 不同，Rust 的类型系统阻止正常安全代码在 `Free(bytes)` 之后继续使用同一个 `bytes` 值；也无需针对 nil 切片分支。

`MaxArrayLen` 是主动施加的 Go 兼容上限，而不是所有 Rust 平台上 `Vec` 的理论最大值。修改该上限会改变 panic 边界，并可能带来跨架构兼容与内存压力风险。

## 并发与资源生命周期

本模块无共享可变状态，函数本身不加锁、不创建线程或异步任务。不同线程持有各自 `Vec<u8>` 时可独立调用 `New` 和 `Free`；实际分配器的线程安全由 Rust 全局分配器保证，不由本文件实现。

资源生命周期从 `New` 返回 `Vec` 开始，在所有权被传给 `Free` 或调用方自行丢弃时结束。显式 `Free` 不是内存安全所必需的特殊释放协议，而是保留 Go API 的生命周期表达；Rust 即使不调用该函数，也会在 `Vec` 离开作用域时自动释放。相邻 `Allocator` 的 `RefCnt` 只统计经其方法发生的逻辑 Alloc/Free 配对，并不改变 `Vec` 的真实所有权规则。

## 与 Go 版本的对应关系

对照文件为 `pkg/lightning/manual/manual.go` 和 `manual_nocgo.go`。

- 常量与可观察内容：Rust `MaxArrayLen` 与 Go cgo 版本数值相同；两者的 `New` 都返回长度为 `n`、内容清零的字节缓冲。
- 分配机制：Go cgo 版本用 `C.calloc` 获取 C 内存，以固定上限数组指针构造切片，并要求 `Free` 调用 `C.free`；Rust 版本用 `Vec<u8>` 和全局分配器，`Free` 仅消费所有权后 `drop`。因此二者 API 意图一致，但分配器来源、FFI 指针属性和泄漏条件并不等价。
- 分配失败：Go cgo 版本在 `calloc` 返回 nil 时调用 `runtime.throw` 终止进程；Rust 交给 `Vec` 的分配失败策略处理。两者都没有可恢复的错误返回，但不应假定失败文本或终止细节完全相同。
- 空缓冲：Go cgo `Free` 必须检查容量，并为长度零但容量非零的切片重新扩展后取得首地址；Rust `drop(Vec)` 不需要该指针操作。
- 非 cgo 版本：Go `manual_nocgo.go` 用普通 `make` 且 `Free` 为空操作。Rust 的 `manual_nocgo.rs` 是单独可调用模块；Cargo 的 `cgo` feature 当前未用于在两个 Rust 模块间自动切换。

相关 Rust 回归位于独立文件 `pkg/lightning/manual/migration_aster_unit_test.rs`，覆盖清零、零长度、保留后备存储的零长度释放，以及 nocgo 负长度 panic。当前同目录没有 Go `*_test.go`，所以 Go 语义证据来自实现文件和 `pkg/lightning/backend/kv/session.go` 的真实调用点。

## 扩展指南

- 修改长度校验、初始化策略或 panic 契约时，应优先修改 `New`，并在独立的 `migration_aster_unit_test.rs` 增加负数、零、上界和超上界用例；不要把测试嵌入 `manual.rs`。
- 修改释放形状时，应修改 `Free`，同步验证空缓冲、截断为零但有容量的缓冲，以及 `Allocator::Free` 的计数配对。不得引入基于 `bytes[0]` 的释放逻辑，否则会破坏零长度边界。
- 若要真正按 `cgo` feature 或 target 切换实现，应在 `lib.rs` 的模块声明/再导出层接线，并同步检查 `Cargo.toml` feature；不能只改 `manual_nocgo.rs`。这类改动还需核实依赖方期待的公开路径。
- 若要求与 Go 的 C 分配器、FFI 地址稳定性或手动泄漏语义完全一致，`Vec` 方案不足，需要单独设计安全封装、分配失败策略和 `unsafe` 边界；不能仅把当前实现描述为 `calloc/free` 的直接移植。
- 性能调整应关注清零成本、大缓冲分配峰值和上层复用策略。缓存/池化应放在 `Allocator` 或 `pkg/lightning/membuf` 等拥有策略的层，而不是在本无状态原语内偷偷加入全局池。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引包含 `pkg/lightning/manual/manual.rs`；`files --filter pkg/lightning/manual` 确认模块及独立测试文件；`node --file ... --offset 1 --limit 240` 读取到本文件全部 44 行；`node pkg/lightning/manual/manual.rs::New` 确认函数定义和控制流。精确 `callers/callees` 查询在 30 秒窗口内未返回结果，因此调用关系又以限定路径源码搜索核验，未将超时当成“无调用者”。
- Rust 源码：`pkg/lightning/manual/manual.rs`（三个公开符号）、`lib.rs`（模块声明与再导出）、`allocator.rs`（直接生产调用边）、`manual_nocgo.rs`（相邻回退实现）。
- crate 配置：`pkg/lightning/manual/Cargo.toml`（crate 名、库入口、空 feature 定义）以及工作区根 `Cargo.toml`、`pkg/ingestor/ingestctrl/Cargo.toml`（工作区登记和 Windows 条件依赖）。
- Go 对照：`pkg/lightning/manual/manual.go`、`manual_nocgo.go`、`allocator.go`；实际 Go 使用点为 `pkg/lightning/backend/kv/session.go::newBytesBuf` 与 `BytesBuf.destroy`。
- 测试证据：`pkg/lightning/manual/migration_aster_unit_test.rs::{new_returns_zeroed_memory_and_free_accepts_it, free_accepts_a_zero_length_slice_with_backing_storage}`；`allocator_test.rs` 通过 `Allocator` 验证间接调用和计数配对。本任务按计划不运行 Cargo。
