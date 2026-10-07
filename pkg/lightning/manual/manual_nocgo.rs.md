# `pkg/lightning/manual/manual_nocgo.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-lightning-manual`（见 [`Cargo.toml`](Cargo.toml)），是 `manual` crate 在未启用 `cgo` feature 时提供的纯 Rust 字节缓冲兼容实现。文件级 `#![cfg(not(feature = "cgo"))]` 控制本文件中的项目是否参与编译；crate 默认 feature 为空，因此默认构建可见 `manual_nocgo::New` 和 `manual_nocgo::Free`，启用 `cgo` 后该模块声明仍在，但这两个函数不再存在。

[`lib.rs`](lib.rs) 通过 `pub mod manual_nocgo` 暴露该子模块，但 crate 根导出的 `New`、`Free` 来自 `manual.rs`，不是本文件。因此当前实现是可显式调用的无 cgo 对照入口，并未像 Go 的构建标签那样自动替换 crate 根 API。RustCodeGraph 的文件关系也将本文件报告为 `used by 0 files`；源码搜索确认生产代码没有直接调用，直接使用只出现在 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)。

## 核心职责

- `New` 用纯 Rust `Vec<u8>` 构造指定长度的清零缓冲，模拟 Go 无 cgo 版本的 `make([]byte, n)`，不接触 C 分配器。
- `Free` 消费 `Vec<u8>` 并立即执行 `drop`，用 Rust 所有权表达“不需要调用 `C.free`”的无 cgo释放路径。
- 文件只提供上述两个函数，没有常量、类型、trait、`impl`、全局状态或其他条件编译分支。

它不负责引用计数。引用计数包装位于 [`allocator.rs`](allocator.rs)，且后者通过 crate 根的 `New`/`Free` 工作，当前不会转发到 `manual_nocgo`。

## 主要符号

`pub fn New(n: isize) -> Vec<u8>`（`manual_nocgo.rs:28`）先用 `usize::try_from(n)` 做有符号到无符号的受检转换，再用 `vec![0; len]` 返回长度为 `len` 的零填充缓冲。它是公开函数，但只在 `not(feature = "cgo")` 时存在。命名保留 Go API 的大写形式；crate 根在 [`lib.rs`](lib.rs) 统一允许 `non_snake_case`。

`pub fn Free(bytes: Vec<u8>)`（`manual_nocgo.rs:36`）取得缓冲的唯一所有权并调用 `drop(bytes)`。签名保证调用之后原变量不能再次使用，也不需要返回状态。

RustCodeGraph `query manual_nocgo --json` 精确定位了文件节点和这两个函数节点；对全仓高度重名的 `New`/`Free` 执行精确 callers/callees 时，工具仍混入同名定义，因此调用关系采用文件级图结果与精确源码引用搜索交叉确认，不采用混入结果。

## 执行流程

调用 `manual_nocgo::New(n)` 时：

1. `usize::try_from(n)` 检查 `n` 是否能表示为平台的 `usize`。
2. 转换失败（实际关注负数）时，`expect("makeslice: len out of range")` 触发 panic。
3. 转换成功后，`vec![0; len]` 申请并初始化缓冲；`n == 0` 时得到空 `Vec`。
4. 所有权返回给调用者。

调用 `manual_nocgo::Free(bytes)` 时，缓冲所有权移入函数并被丢弃；最后一个所有者释放 `Vec` 的堆分配。当前唯一直接验证路径在 `nocgo_fallback_allocates_zeroed_memory` 中依次执行 `New(7)`、检查七个零字节并执行 `Free`。

## 数据与状态

唯一输入状态是 `New` 的 `isize` 长度，唯一返回状态是拥有缓冲的 `Vec<u8>`。缓冲的长度由转换后的 `len` 决定，元素初始化值固定为 `0`。本文件不缓存分配结果，不保存裸指针，不共享引用计数，也不修改 crate 级状态。

生命周期完全随 `Vec` 所有权移动：`New` 创建所有权，调用者持有和传递所有权，`Free` 消费所有权。与 [`manual.rs`](manual.rs) 不同，本文件没有 `MaxArrayLen` 上限检查；在 64 位平台上，能转换成 `usize` 但大于 `2^31-1` 的长度不会先被本文件拒绝，而会进入 `Vec` 分配路径。这是当前源码事实，不能把 cgo 实现的上限约束推断到本文件。

## 依赖与调用关系

下游只使用 Rust 标准库与语言设施：`usize::try_from` 完成受检转换，`vec!` 创建零填充缓冲，`drop` 结束所有权生命周期。`Cargo.toml` 没有声明普通依赖，只声明空的 `default` feature 集和 `cgo` feature。

上游装配点是 [`lib.rs`](lib.rs) 的 `pub mod manual_nocgo`。当前直接调用边均来自 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)：

- `nocgo_fallback_allocates_zeroed_memory -> manual_nocgo::New`
- `nocgo_fallback_allocates_zeroed_memory -> manual_nocgo::Free`
- `nocgo_fallback_rejects_negative_lengths_like_go -> manual_nocgo::New`

生产调用搜索未找到 `manual_nocgo::New`、`manual_nocgo::Free` 或外部 crate 路径下的直接使用。crate 根的 [`allocator.rs`](allocator.rs) 调用 `super::{New, Free}`，而 [`lib.rs`](lib.rs) 将这两个名字从 `manual` 模块再导出，所以该链为 `Allocator -> manual.rs`，不是 `Allocator -> manual_nocgo.rs`。

## 错误处理与边界

`New` 不返回 `Result`。负长度在 `usize::try_from` 处 panic，消息包含 `makeslice: len out of range`；`nocgo_fallback_rejects_negative_lengths_like_go` 用 `#[should_panic(expected = ...)]` 固定了这一契约。非负但无法实际分配的长度由 `Vec` 分配路径处理，本文件没有恢复、降级或自定义内存不足错误。

零长度合法：`vec![0; 0]` 返回空缓冲，`Free` 也可消费空缓冲。`Free` 不检查长度或容量，不会产生业务错误。由于它按值接收 `Vec`，Rust 调用者不能像 Go 调用者那样在调用空函数后继续使用自己的切片变量；这是用所有权换取确定释放的接口差异。

启用 `cgo` feature 后调用 `manual_nocgo::New`/`Free` 会是编译期缺少符号，而不是运行时回退。新增调用者必须在 feature 边界上自行保证只在 `not(feature = "cgo")` 配置引用它们。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道、事务或共享可变状态。每次 `New` 产生独立的 `Vec<u8>`；是否跨线程移动由 `Vec<u8>` 的标准 `Send` 语义决定，本文件不创建并发执行。

资源释放是同步、确定性的：`Free` 获取所有权后在函数返回前执行 `drop`。若调用者不调用 `Free`，缓冲仍会在其正常 Rust 所有者离开作用域时自动释放；若调用者用 `mem::forget` 等手段绕过析构，则不属于本文件能够防止的范围。本文件不涉及 C 指针，因此没有 Go cgo 版本中 `calloc`/`C.free` 配对和指针传递规则的风险。

## 与 Go 版本的对应关系

直接对照文件是 [`manual_nocgo.go`](manual_nocgo.go)。Go 用 `//go:build !cgo` 让 `New(n int) []byte` 与空操作 `Free(b []byte)` 在无 cgo 构建中成为包级实现；Rust 用文件级 `cfg(not(feature = "cgo"))` 提供 `New(n: isize) -> Vec<u8>` 与消费所有权的 `Free(Vec<u8>)`。两者的共同语义是：分配长度为 `n` 的零值字节序列，且不调用 C 的 `malloc`/`free`。

需要保留的差异有：

- Go 的 `Free` 是空函数，由垃圾回收器决定回收时间；Rust 的 `Free` 立即丢弃传入 `Vec`。
- Go build tag 让 nocgo 文件替换同包 `New`/`Free`；Rust crate 根仍固定再导出 [`manual.rs`](manual.rs)，nocgo 版本只在显式子模块路径可达。
- Go 使用平台 `int`，Rust 使用 `isize`；两者都拒绝负切片长度，但 Rust 将 panic 文本显式固定为 `makeslice: len out of range`。
- Rust nocgo 实现没有 [`manual.rs`](manual.rs) 的 `MaxArrayLen` 检查，也没有 Go cgo 实现的 C 分配失败 `throw("out of memory")` 路径。

Go 同目录没有独立的 `manual_nocgo_test.go`；当前移植语义由 Rust 的 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 覆盖清零、释放与负长度 panic。

## 扩展指南

若要让无 cgo feature 真正选择 crate 根 API，应首先修改 [`lib.rs`](lib.rs) 的条件再导出，而不是仅改本文件；同时检查 [`allocator.rs`](allocator.rs) 是否应随 feature 转发，并为 `cgo` 开关的两种编译配置增加独立测试。这个改动会改变公开接线路径，不能从当前文件的存在推断为已完成。

若要调整长度边界，应修改 `New` 的转换/上限逻辑，并明确是否需要与 [`manual.rs`](manual.rs) 的 `MaxArrayLen` 完全一致。测试至少应放在独立的 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 或新增同目录独立 `*_test.rs` 文件中，覆盖 `0`、负数、正常长度和选定上限；不要把测试写回生产源文件。

若要改变释放策略，应修改 `Free` 的所有权签名和实现，并同步核对所有调用者是否仍能保证单次释放。兼容风险主要是公开 API 路径及 panic/所有权语义；性能风险主要是大缓冲的清零成本和分配峰值。任何引入复用池、共享所有权或延迟释放的方案都需要另外说明并发同步与容量回收策略。

## 验证依据

本说明核对了以下文件：目标 [`manual_nocgo.rs`](manual_nocgo.rs)、模块入口 [`lib.rs`](lib.rs)、默认实现 [`manual.rs`](manual.rs)、分配器 [`allocator.rs`](allocator.rs)、crate 声明 [`Cargo.toml`](Cargo.toml)、Go 对照 [`manual_nocgo.go`](manual_nocgo.go) 与 [`manual.go`](manual.go)，以及独立 Rust 测试 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 和 [`allocator_test.rs`](allocator_test.rs)。[`BUILD.bazel`](BUILD.bazel) 仅描述 Go library，未为本 Rust 文件提供额外接线。

执行过的 RustCodeGraph 证据包括：`status`（索引含 11,467 个文件，目标目录的 9 个 Go/Rust 文件均已索引）、`files --filter pkg/lightning/manual`、`query manual_nocgo --json`、目标及相邻文件的 `node --file`，以及 `New`/`Free` 的 `callers`/`callees` 尝试。文件图报告目标 `used by 0 files`；同名函数图查询存在消歧污染，因此又用 `rg` 精确核对 `manual_nocgo` 和 `astersql_lightning_manual` 引用，结果只发现模块声明和上述迁移测试调用。

任务是纯文档分析，按计划未运行 Cargo。结构验收以目标文档存在且固定标题恰好为十一个为准；人工复核重点是模块没有自动替换根 API、feature 边界、负数 panic、无 `MaxArrayLen` 限制以及 `Vec` 所有权释放语义均有源码或测试依据。
