# `pkg/expression/builtin_regexp_util.rs`

## 文件定位

[对应 Rust 源文件](builtin_regexp_util.rs) 属于 `astersql-expression` crate；crate 由 `pkg/expression/Cargo.toml` 定义，并在 `pkg/expression/lib.rs` 中通过 `#[path = "builtin_regexp_util.rs"] mod builtin_regexp_util_kernel;` 装配。它保存从 Go `pkg/expression/builtin_regexp_util.go` 对照迁移而来的正则内建辅助模型：可空列、参数列缓冲、缓冲归还、行级 NULL 判断、结果 NULL 填充，以及 position 越界的特殊边界判断。

当前生产接线需要分层理解：`pkg/expression/builtin_regexp.rs` 直接导入并调用 `check_out_range_pos`，用于正则子串等位置裁剪；`Column`、`FuncParam`、`BufferAllocator`、`RegexpMemorizedSig` 和其余辅助函数目前没有接入真实 `chunk_dependency::Column` 或生产向量执行器，主要由同 crate 的独立测试使用。因此本文件既不是完整正则引擎，也不是 Go `chunk`/缓冲池的通用 Rust 替代品。

## 核心职责

- 用 `Column<T>` 表示测试/移植辅助层中的逐行可空值，保留“按行判断 NULL”和“预留结果容量”的核心语义。
- 用 `FuncParam<T>` 区分无需列缓冲的常量参数与持有物化列的参数，并通过 `get_buffers` 只收集后者。
- 用 `BufferAllocator<T>` 与 `release_buffers` 模拟 Go `baseBuiltinFunc.bufAllocator` 的归还行为，同时保留参数仍可观察到原列的特征。
- 用 `is_result_null` 和 `fill_null_string_into_result` 表达向量正则路径的 NULL 传播与提前返回结果构造。
- 用 `check_out_range_pos` 集中表达 position 已落在常规范围外时的唯一例外：空输入且 position 为 1 不算非法。
- 用 `RegexpMemorizedSig<T, E>` 记录“编译结果或编译错误均可缓存”的 Go 数据形状；Rust 生产缓存的实际实现位于 `pkg/expression/builtin_regexp.rs::RegexpBase`。

## 主要符号

- `Column<T> { values: Vec<Option<T>> }`：本文件私有存储、公开类型。`new` 构造列，`values` 返回只读切片，`is_null` 通过索引判断 NULL，`append_null` 追加 NULL，`reserve` 先清空旧行再预留容量。它不是 `pkg/util/chunk` 的实际列类型。
- `RegexpMemorizedSig<T, E>`：两个公开可选字段分别保存成功值和失败值。类型本身不执行编译、不维持互斥锁，也不保证两字段互斥；调用者若使用它必须建立该不变量。当前 Rust 搜索未发现生产构造或读取点。
- `FuncParam<T>`：以私有 `Option<Column<T>>` 保存物化列。`constant()` 产生 `None`，`column(...)` 产生 `Some`，`get_col()` 只借用列。
- `BufferAllocator<T>`：以私有 `Vec<Column<T>>` 保存归还列。`put` 追加所有权，`returned_len` 暴露计数；它不复用、限容或清空已归还列。
- `get_buffers(&[FuncParam<T>]) -> Vec<&Column<T>>`：按参数原顺序过滤常量参数并收集列引用。
- `release_buffers(&mut BufferAllocator<T>, &[FuncParam<T>])`：克隆每个物化列后交给分配器，故要求 `T: Clone`，且不会把 `FuncParam.column` 置空。
- `is_result_null(&[Column<T>], row) -> bool`：若任一列在该行为 NULL 则返回 `true`；空列集合返回 `false`。
- `fill_null_string_into_result(&mut Column<String>, num)`：先 `reserve(num)` 清空旧结果，再精确追加 `num` 个 NULL。
- `check_out_range_pos(str_len, pos) -> bool`：实现 `str_len != 0 || pos != 1`；只有 `(0, 1)` 返回 `false`。

## 执行流程

缓冲辅助流程从一组 `FuncParam` 开始。调用方可用 `get_buffers` 提取所有物化列，随后逐行调用 `is_result_null`：迭代器按列顺序短路，只要任一列为 NULL，该行就应产生 NULL。若某个常量参数本身决定整批结果为 NULL，则 `fill_null_string_into_result` 会丢弃结果列旧内容、预留目标容量并追加恰好指定行数。计算结束后，`release_buffers` 遍历参数；常量参数被跳过，物化列被克隆并登记到 `BufferAllocator.returned`，而参数自身仍持有原列。

生产位置检查流程位于 `pkg/expression/builtin_regexp.rs`。`trim_utf8_at_position` 和 `trim_bytes_at_position` 先判断 position 是否小于 1 或大于字符数/字节数；当调用路径允许空输入的 position=1 特例时，再调用 `check_out_range_pos`。只有空输入且 position=1 会继续执行，并得到空后缀；其他越界值转换为 `RegexpError::InvalidIndex`。该函数本身只判断布尔条件，不切片也不创建错误。

正则缓存不经过本文件的 `RegexpMemorizedSig`：生产路径由 `builtin_regexp.rs::RegexpBase::get_regexp_with_argument` 在 pattern 与 match type 可记忆化时，按 `context_id` 查询或写入 `Arc<Mutex<HashMap<u64, CachedRegexp>>>`，其中 `CachedRegexp` 同样同时覆盖成功和失败结果。

## 数据与状态

`Column<T>`、`FuncParam<T>` 和 `BufferAllocator<T>` 都是拥有所有权的普通值，没有内部可变性。`Column.values` 的长度就是当前行数；`reserve` 的关键状态转换是先将长度归零，再至少为新批次请求容量，所以调用它不是单纯扩容。`get_buffers` 返回的引用生命周期受参数切片约束，不能脱离参数存活。

`release_buffers` 使用克隆而非移动：归还后存在两份逻辑相同的列，一份仍在参数中，一份在 `returned` 中。这是为了模拟 Go 代码“把列指针放回池但不清空 `funcParam.col`”的可观察状态，而不表示两个 Rust 值共享后续修改。`RegexpMemorizedSig` 仅是数据容器；真正跨调用共享的缓存状态和锁均在 `RegexpBase`。

## 依赖与调用关系

本文件只依赖 Rust 标准库的 `Vec`、`Option`、切片、泛型与派生 trait，没有直接使用 `Cargo.toml` 中的 `regex`、`chunk-dependency` 或异步运行时。`pkg/expression/lib.rs` 将其注册为 crate 私有模块；在 `#[cfg(test)]` 的 `expression_regexp` 门面中又重导出全部符号，供同 crate 测试访问。

定向调用证据如下：

- `pkg/expression/builtin_regexp.rs` 通过 `use crate::builtin_regexp_util_kernel::check_out_range_pos` 导入该函数；`trim_utf8_at_position` 与 `trim_bytes_at_position` 是生产下游调用点。
- `pkg/expression/builtin_regexp_util_test.rs` 直接覆盖缓冲归还、结果重置与越界索引 panic。
- `pkg/expression/builtin_regexp_util_23_aster_unit_test.rs` 覆盖 NULL 判定、NULL 填充和 position 特例，并连同完整正则路径做 Go 语义回归。
- 仓库搜索未发现其他 Rust 生产文件直接使用 `RegexpMemorizedSig`、本地 `Column`、`FuncParam`、`BufferAllocator`、`get_buffers`、`release_buffers`、`is_result_null` 或 `fill_null_string_into_result`。Go 版本则在 `builtin_regexp.go` 与 `builtin_ilike_vec.go` 的向量路径广泛调用对应辅助函数。

## 错误处理与边界

本文件没有 `Result` 返回值或自定义错误类型。`Column::is_null(row)` 直接索引 `Vec`，越界会 panic；`is_result_null` 继承该行为，只要迭代触及长度不足的列就会 panic。独立测试 `null_lookup_panics_for_an_out_of_range_row_like_chunk_column` 明确将此视为与 Go `chunk.Column` 对齐的边界，而不是静默返回非 NULL。

`get_buffers`、`release_buffers` 和 `is_result_null` 对空输入均自然成功：分别返回空向量、不归还任何列和返回 `false`。`fill_null_string_into_result(..., 0)` 会清空旧结果并保持零行。`check_out_range_pos` 必须只在调用方已判定“常规范围外”后解释；孤立地传入非空字符串长度和合法 position 仍会返回 `true`，因为它不是完整合法性检查器。

容量申请可能因内存不足而中止进程，克隆 `T` 也可能成本很高；本文件没有恢复或限流策略。生产正则错误（非法索引、编译失败、缓存锁中毒等）由 `builtin_regexp.rs::RegexpError` 处理，不由本文件生成。

## 并发与资源生命周期

本文件没有线程、异步任务、通道、锁或事务。所有状态通过 `&self`、`&mut self` 和所有权传递约束在调用栈内；`BufferAllocator` 也不是线程安全池。若在多个线程使用这些值，调用方必须遵循 Rust 自动 trait 推导并自行同步。

资源生命周期的关键点是：`get_buffers` 只借用参数列；`release_buffers` 为保持参数可见性而克隆列，将副本的所有权交给分配器；分配器析构时统一释放其 `returned` 向量。与之不同，生产 `RegexpBase` 使用 `Arc<Mutex<...>>` 共享编译缓存，缓存生命周期随最后一个 `RegexpBase` 克隆结束，锁中毒会转换为 `RegexpError::CachePoisoned`。

## 与 Go 版本的对应关系

`pkg/expression/builtin_regexp_util.go` 是直接语义对照：`regexpMemorizedSig` 对应 `RegexpMemorizedSig`，`releaseBuffers`/`getBuffers`/`isResultNull`/`fillNullStringIntoResult`/`checkOutRangePos` 分别对应同名 snake_case Rust 函数。NULL 短路、参数顺序、清空后填充、空串 position=1 特例均保持一致。

实现层面存在明确差异。Go 使用真实 `chunk.Column`、`baseBuiltinFunc.bufAllocator` 和指针；Rust 本文件使用轻量泛型列与只记录归还项的分配器，并以克隆模拟指针归还后参数仍引用列。Go 的 `regexpMemorizedSig` 被 `builtin_regexp.go` 实际缓存使用，而 Rust 同名结构当前未接线；生产 Rust 改由 `RegexpBase` 缓存 `Result<Arc<CompiledRegexp>, RegexpError>`。此外，Go 向量正则/ILIKE 路径广泛调用缓冲辅助函数，当前 Rust 生产调用仅确认到 `check_out_range_pos`。因此扩展时不能仅因符号对照存在就假设 Rust 已完成 Go 的整套向量缓冲集成。

## 扩展指南

若要把缓冲辅助接入生产向量执行，应优先评估复用真实 `chunk_dependency::Column` 和现有 `pkg/expression/builtin_func_param.rs`，避免继续扩展本文件的测试型 `Column`/`FuncParam` 并形成第二套列模型。接线必须保持 Go 的参数顺序、常量参数跳过、NULL 合并和提前填充行为；同时应决定归还后参数是否仍允许访问，并用所有权设计代替无意的整列克隆。相关测试应放在独立文件，至少同步更新 `builtin_regexp_util_test.rs`，并按实际正则行为补充 `builtin_regexp_test.rs` 或专门的向量测试。

若修改 position 规则，应同时审查 `check_out_range_pos`、`builtin_regexp.rs::trim_utf8_at_position`、`trim_bytes_at_position`、INSTR 的专用裁剪函数以及 Go `checkOutRangePos` 的调用前置条件。重点风险包括 UTF-8 字符数与字节长度混用、空输入 position=1 兼容性、负数转 `usize`、切片 panic和标量/向量分支不一致。

若整合正则缓存，需在“保留 `RegexpMemorizedSig` 数据形状”与“统一到 `RegexpBase` 的并发缓存”之间选定单一真实实现；必须继续缓存失败结果、按上下文隔离，并覆盖锁中毒/动态参数不缓存等 Rust 特有行为。不要把本文件的无锁容器直接描述或改造成共享缓存而缺少并发测试。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/expression/builtin_regexp_util.rs` 确认目标文件含 20 个符号；`node --file ... --offset 1 --limit 500` 读取了全部 158 行并报告模块使用文件；逐符号 `query` 定位了本文列出的类型和函数。`callers/callees` 命令在本环境长时间无返回，因此精确调用点由后续定向搜索补齐。
- 已读生产源码与配置：`pkg/expression/builtin_regexp_util.rs`、`pkg/expression/builtin_regexp.rs`、`pkg/expression/lib.rs`、`pkg/expression/Cargo.toml`。
- 已读 Go 对照与调用证据：`pkg/expression/builtin_regexp_util.go`，以及 `pkg/expression/builtin_regexp.go` 中的缓冲、NULL 与 position 调用段；定向搜索还确认 `pkg/expression/builtin_ilike_vec.go` 使用 Go 辅助函数。
- 已读独立 Rust 测试：`pkg/expression/builtin_regexp_util_test.rs`、`pkg/expression/builtin_regexp_util_23_aster_unit_test.rs`。它们验证归还后参数仍持列、结果旧行被清空、越界索引 panic、NULL 传播和 `(str_len=0, pos=1)` 特例。
- 事实边界：本文没有以测试存在替代生产接线；所有未发现的生产使用均表述为“仓库定向搜索未发现”，没有推断未来设计。
