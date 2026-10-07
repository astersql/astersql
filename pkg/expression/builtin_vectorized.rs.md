# `pkg/expression/builtin_vectorized.rs`

## 文件定位

本文件属于 `astersql-expression` crate，是内置表达式向量化求值的基础辅助模块。它提供两类能力：可并发复用的临时 `chunk::Column` 缓冲池，以及当内置函数没有专用整列算法时，将 `builtinFunc` 的逐行 `evalInt` / `evalString` 接口适配为整列结果的回退函数。源码由 `pkg/expression/lib.rs:281-282` 以私有模块 `builtin_vectorized_kernel` 装配；crate 根仅在 `pkg/expression/lib.rs:366` 公开再导出 `GetColumn` 和 `PutColumn`，其余符号目前保持 crate 内可见。

`pkg/expression/Cargo.toml` 将该目录定义为 `astersql-expression`，入口为 `lib.rs` 且关闭自动测试发现。文件直接使用的列、类型和 MySQL 类型分别经 crate 根映射到 `astersql-util-chunk`、`astersql-types` 和 `astersql-parser-mysql`；没有由本文件单独控制的 Cargo feature 或条件编译项。

## 核心职责

1. `columnBufferAllocator` 统一临时列的借出、归还和分配器自身内存计量协议（`builtin_vectorized.rs:28-37`）。
2. `localColumnPool` 用 `Mutex<Vec<Box<chunk::Column>>>` 保存已归还的列，在多线程间安全复用；池为空时复制 `columnTempl`（`builtin_vectorized.rs:41-61,77-101`）。
3. `GetColumn` / `PutColumn` 将单例 `globalColumnAllocator` 暴露为 crate 的公共借还边界（`builtin_vectorized.rs:61-75`）。
4. `vecEvalIntByRows` 和 `vecEvalStringByRows` 逐行调用标量内置函数，构造与输入逻辑行数相同的向量结果，并保留 NULL 与首错即停语义（`builtin_vectorized.rs:113-148`）。

本文件不负责判断某个表达式是否可向量化、不负责选择具体返回类型，也不重置借出的列。调用方必须按目标求值类型初始化或调整列，并保证每次成功借出都有且只有一次归还。

## 主要符号

- `pub trait columnBufferAllocator`：内部抽象。`get(&self) -> Result<Box<chunk::Column>, Error>` 转移列所有权给调用方；`put(&self, Box<Column>)` 收回所有权；`MemoryUsage()` 只报告分配器对象自身大小。
- `pub struct localColumnPool`：基于互斥 `Vec` 的具体实现。字段私有，因此只能通过 `newLocalColumnPool` 构造并通过 trait 操作。
- `columnTempl: LazyLock<Box<chunk::Column>>`：延迟创建的 LONG LONG 列模板，初始容量为 `chunk::InitialCapacity`。`chunk::Column::CopyConstruct(None)` 会完整克隆模板的位图、偏移和数据等存储；这里的模板初始为空。
- `newLocalColumnPool() -> localColumnPool`：创建空池；第一次取列才会触发模板克隆。
- `globalColumnAllocator: LazyLock<localColumnPool>`：进程内该 crate 的共享列池，首次调用时初始化。
- `GetColumn(types::EvalType, usize)` / `PutColumn(Box<Column>)`：公共包装。当前实现有意忽略求值类型和容量，返回的列不能据参数推断为已初始化。
- `emptyLocalColumnPoolSize`：`mem::size_of::<localColumnPool>()` 的静态值，不包括锁后 `Vec` 的容量和池中列占用。
- `write_i64`：将一个 `i64` 按本机字节序写到固定宽度列的 `data` 对应槽位；前置条件是结果列已由 `ResizeInt64` 分配足够空间。
- `vecEvalIntByRows` / `vecEvalStringByRows`：整数、字符串的逐行回退入口。二者接收只读求值上下文、动态 `builtinFunc`、输入 `Chunk` 和可变结果列，成功返回 `Ok(())`。

## 执行流程

列池借还流程如下：

1. `GetColumn` 访问延迟初始化的 `globalColumnAllocator` 并调用 `localColumnPool::get`。
2. `get` 获取互斥锁；若锁已中毒，返回 `types::errors::New("localColumnPool lock poisoned")`。
3. 若池中有列，以 LIFO 顺序 `pop` 并转移所有权；否则从 `columnTempl` 复制一个新列。借出时不清除旧数据。
4. 调用方按实际求值类型重置/扩容并使用列，最后把所有权交给 `PutColumn`。
5. `put` 成功加锁时将列压回池；若锁已中毒则静默丢弃该缓冲，因为 trait 的归还接口没有错误返回通道。

整数回退 `vecEvalIntByRows` 先以 `input.NumRows()` 调用 `result.ResizeInt64(n, false)`，随后按逻辑行号调用 `input.GetRow(i)` 和 `sig.evalInt`。每行先写 NULL 标志，再由 `write_i64` 写固定八字节值；即使该行是 NULL，也仍写入返回的占位值，读取方必须以 NULL 位图为准。

字符串回退 `vecEvalStringByRows` 先调用 `result.ReserveString(n)` 清理并预留变长列结构，再逐行调用 `sig.evalString`。NULL 行追加 `AppendNull`，非 NULL 行追加 `AppendString`。两条回退都遵循输入 `Chunk` 的逻辑行视图；`NumRows` 和 `GetRow` 会反映 selection，因此结果位置是选择后行序，而不是原物理行号。

## 数据与状态

持久状态只有两个延迟初始化的静态对象：不可变空列模板 `columnTempl` 与内部可变的全局池 `globalColumnAllocator`。池里的每个 `Box<Column>` 具有唯一所有者；所有权从池转给调用方，再由调用方转回池，Rust 类型系统阻止归还后继续通过原变量访问。

列池明确保留列的既有内容和容量。`pkg/expression/builtin_vectorized_test.rs:388-403` 验证同一指针可被复用且旧值仍存在；因此“池复用”不是“获得空列”的同义词。整数回退会用 `ResizeInt64` 重建长度和 NULL 状态，字符串回退会用 `ReserveString` 重建变长列容器，但其他使用者仍必须自行选择正确的重置方法。

`MemoryUsage` 只返回 `emptyLocalColumnPoolSize`，不会随着缓存列数量或容量变化。该值适合保持 Go 接口兼容，不能用于统计池的真实堆内存。文件本身没有缓存求值上下文、表达式或行数据；逐行函数产生的唯一可观察状态是结果列的渐进写入以及 `builtinFunc` 实现可能产生的上下文副作用（例如警告）。

## 依赖与调用关系

上游装配关系为 `pkg/expression/lib.rs` → `builtin_vectorized_kernel`。公开 Rust API 只有 crate 根再导出的 `GetColumn` / `PutColumn`；`columnBufferAllocator`、`localColumnPool` 和两个逐行回退虽声明为 `pub`，仍受私有父模块限制，主要供 crate 内部使用和测试访问。

RustCodeGraph 对精确 Rust 定义的查询结果为：`vecEvalIntByRows`、`vecEvalStringByRows`、`GetColumn`、`PutColumn` 的生产调用者均为空；`vecEvalIntByRows` 的图内下游边仅识别到私有 `write_i64`。仓库级 Rust 搜索也只找到 `pkg/expression/builtin_vectorized_test.rs` 对这些入口的直接调用。因此截至当前源码，公共列池已导出但尚无生产 Rust 调用点，两条回退也尚未接入 Rust 内置函数分派主链；不能据 Go 调用关系推断 Rust 已接线。

真实的运行期下游依赖包括：

- `chunk::Column`：分配、复制、定长/变长重置、NULL 位图和数据写入。
- `chunk::Chunk` / `chunk::Row`：提供逻辑行数及逐行视图。
- `builtinFunc`（定义于 `pkg/expression/builtin_core.rs`）：提供 `evalInt`、`evalString` 标量协议。
- `EvalContext`：把 SQL 模式、时区、警告等语义传递给具体内置函数；本文件不直接读取上下文字段。
- `types::Error`：统一传播求值与锁错误。

## 错误处理与边界

- `localColumnPool::get` 唯一自身错误是互斥锁中毒；错误发生时不返回列。模板复制在当前接口中不返回错误。
- `localColumnPool::put` 无法返回错误，锁中毒时会丢弃传入列。这不会破坏内存安全，但会降低后续复用率。
- 两个逐行回退使用 `?` 原样传播 `builtinFunc` 的首个错误，之后的行不再求值，且已经写入的结果不回滚。整数和字符串首错即停分别由 `builtin_vectorized_test.rs:357-386` 覆盖。
- 空输入是合法边界：整数结果被调整为长度 0，字符串结果被预留为 0 行，循环不执行。
- NULL 与错误相互独立：成功返回的 `isNull` 决定 NULL 位；错误则立即结束，不能依赖该错误行的结果槽。
- `write_i64` 直接切片写入，若调用顺序破坏了“先 `ResizeInt64(n, false)`”的不变量会 panic；它是私有函数，当前唯一调用点维持该不变量。
- `GetColumn` 忽略 `EvalType` 和容量是当前 Go/Rust 共同契约，不应把返回值当作与参数匹配的已初始化列。

## 并发与资源生命周期

`LazyLock` 保证模板与全局池只初始化一次；`Mutex` 串行化每次 `get` / `put` 对内部 `Vec` 的访问。锁仅覆盖弹出或压入操作，不覆盖调用方使用列的时间，因此不同线程借到的列互不共享。`builtin_vectorized_test.rs:406-431` 以 5 个线程各执行 128 次借还，验证并发操作不 panic 且结束后仍可借出列。

与 Go 的 `sync.Pool` 不同，Rust 的 `Vec` 缓存不会由运行时在 GC 周期自动清空，并且采用确定的 LIFO 顺序；这可能提高复用稳定性，也可能长期保留峰值容量。所有权接口避免同一列被同时归还或使用，但仍要求业务路径在提前返回时显式归还；本文件没有 RAII 借用守卫。两个逐行回退本身不创建线程、不持锁、不启动异步任务，结果列由调用方独占可变借用，因此并发安全边界位于调用方如何分享 `builtinFunc`、`EvalContext` 和输入数据。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/expression/builtin_vectorized.go`，Rust 保留了同名接口、构造器、全局分配器、内存计量常量以及整数/字符串逐行算法：先初始化结果列，按行调用标量函数，保存 NULL，并在首个错误处返回。`GetColumn` 在两种语言中都忽略求值类型和容量。

关键实现差异如下：

- Go `localColumnPool` 嵌入无锁 `sync.Pool`，运行时可以清除缓存；Rust 使用 `Mutex<Vec<Box<Column>>>`，缓存持久且加锁，借出顺序为 LIFO。
- Go `get` 通过动态类型断言防御池内出现非 `*chunk.Column`；Rust 容器的静态元素类型从结构上排除了错误对象类型，转而只需处理锁中毒。
- Go 整数路径通过 `result.Int64s()` 写切片；Rust 以私有 `write_i64` 写 `Column.data`，语义依赖固定宽度布局和本机字节序。
- Go 接口和返回对象由 GC 管理；Rust 用 `Box` 所有权表达借还，归还后不能再访问原对象。
- Go 生产代码 `pkg/expression/builtin_time_vec.go` 有多个 `vecEvalIntByRows` / `vecEvalStringByRows` 回退调用，而当前 Rust 代码搜索没有生产调用者。因此算法已移植并有测试，但 Rust 主链接线仍未由本文件证据确认。

Go 测试 `pkg/expression/builtin_vectorized_test.go` 覆盖更广的所有求值类型、selection、并行性能和向量化判断；Rust 独立测试聚焦本文件实际提供的整数/字符串回退、错误/NULL 以及列池借还。不能把 Go 的其他类型测试视为本 Rust 文件已经实现相应回退的证据。

## 扩展指南

- 新增固定宽度逐行回退时，应仿照 `vecEvalIntByRows`：先使用对应的 `Column` 重置 API，再按逻辑行调用 `builtinFunc` 标量接口，同时保留 NULL 和首错即停。不要复用 `write_i64` 写不同宽度或不同布局。
- 新增变长类型回退时，应仿照 `vecEvalStringByRows` 使用追加 API，确保 NULL 行也推进结果长度。
- 若要让 `GetColumn` 尊重类型或容量，必须同步核对 Go 合约及所有调用方的重置假设；改变“返回未初始化缓冲”的行为可能影响性能和旧内容复用测试。
- 若将缓冲池接入新的生产路径，应使用能保证提前返回也归还的局部守卫或明确清理结构，避免错误分支永久移出缓冲。还应评估互斥竞争、峰值列容量长期驻留以及 `MemoryUsage` 低估问题。
- 若修改池结构或计量语义，同步更新 `emptyLocalColumnPoolSize` / `MemoryUsage` 的契约，并与 Go 的 `unsafe.Sizeof(localColumnPool{})` 行为比较。
- 测试必须继续放在独立的 `pkg/expression/builtin_vectorized_test.rs`，不要内嵌到生产文件。至少覆盖空输入、selection、NULL、首行/中间行错误、复用前旧内容、并发借还和锁中毒策略；涉及生产接线时还应在对应内置函数的独立 `*_test.rs` 中验证实际分派路径。

## 验证依据

- 目标源码：`pkg/expression/builtin_vectorized.rs`（完整 148 行），核对全部 trait、结构、静态对象、常量、函数和 impl；文件无条件编译项。
- 模块与 crate：`pkg/expression/lib.rs:281-282,366,633-634`；`pkg/expression/Cargo.toml` 的 `[package]`、`[lib]`、依赖和 `autotests = false`。
- Rust 测试：`pkg/expression/builtin_vectorized_test.rs:322-447`，覆盖值与 NULL、首错停止、局部池指针复用、并发借还和全局池往返。
- Go 对照：`pkg/expression/builtin_vectorized.go`；Go 测试 `pkg/expression/builtin_vectorized_test.go`；Go 生产调用点 `pkg/expression/builtin_time_vec.go:359,943,1226,1253,1895`。
- 下游定义：`pkg/expression/builtin_core.rs:32,74,82` 的 `builtinFunc`；`pkg/util/chunk/column.rs:282,603,638` 的 `CopyConstruct`、`ResizeInt64`、`ReserveString`。
- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go；精确 `query` 找到本文件与 Go 对应符号；带 `--file pkg/expression/builtin_vectorized.rs` 的 `callers` 对 `vecEvalIntByRows`、`vecEvalStringByRows`、`GetColumn`、`PutColumn` 均返回空数组，`callees vecEvalIntByRows` 返回 `write_i64`。仓库 `rg` 复核 Rust 侧引用只存在于 `lib.rs` 和独立测试。
- 本任务为纯文档分析，按计划不运行 Cargo；完成前以指定命令验证恰有 11 个固定二级章节，并人工复核所有“已接线/已支持”陈述均有上述源码或调用证据。
