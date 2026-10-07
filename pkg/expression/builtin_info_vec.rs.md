# `pkg/expression/builtin_info_vec.rs`

## 文件定位

该文件属于 `astersql-expression` crate，是信息类 SQL 内置函数的向量化求值内核。模块由 [`lib.rs`](lib.rs) 以 `#[path = "builtin_info_vec.rs"] mod builtin_info_vec_kernel;` 私有装配，复用 [`builtin_info.rs`](builtin_info.rs) 中的会话模型、标量语义和键解码接口，而不是重新实现 SQL 规则。它接收一个批次的行数或已求值参数列，返回 `Vec<T>` 形式的结果列。

当前 RustCodeGraph 索引显示这些公开函数的直接调用者主要位于独立测试，未发现从生产表达式派发器到本模块的调用边。因此，本文件是已经实现并受测试约束的移植内核，但不能仅凭现有代码断言它已经接入完整 SQL 执行主链。Go 对照实现 [`builtin_info_vec.go`](builtin_info_vec.go) 则直接实现各 `builtin*Sig` 的 `vectorized` 与 `vecEval*` 方法。

## 核心职责

- `InfoBuiltinKind::vectorized` 保存与 Go 版本一致的向量化能力矩阵：18 种信息函数中，`TiDBMvccInfo`、`TiDBEncodeRecordKey`、`TiDBEncodeIndexKey` 为不可向量化，其余为可向量化。
- `vec_database` 至 `vec_found_rows`、`vec_last_insert_id`、`vec_version` 等包装器只计算一次会话或系统标量，再按 `rows` 广播到整列；这保证同一批次内会话快照一致，也避免逐行重复读取。
- `vec_benchmark` 按循环次数重复触发一次“整列子表达式求值”，成功后生成指定行数的全零、非 NULL 整数列。
- `vec_last_insert_id_with_id` 保持参数列原样返回，同时从后向前寻找最后一个非 NULL 值并写入 `SessionInfo.last_insert_id`。
- `vec_decode_key` 必须逐行处理，因为每行输入可能不同；它保留 NULL、支持无 codec 时原文回退，并传播 codec 的首个错误。

## 主要符号

- `pub enum InfoBuiltinKind`：列出 `Database`、`ConnectionId`、`TiDBVersion`、`RowCount`、`CurrentUser`、`CurrentResourceGroup`、`CurrentRole`、`User`、`TiDBIsDdlOwner`、`FoundRows`、`Benchmark`、两种 `LastInsertId`、`Version`、三种 MVCC/键编码函数和 `TiDBDecodeKey`。它是能力描述，不携带求值状态。
- `InfoBuiltinKind::vectorized(self) -> bool`：仅用否定匹配排除三个不可向量化变体。增加新变体时若不更新该匹配，会默认判为可向量化，因此必须先确认 Go 侧能力。
- 广播包装器：`vec_database` 返回 `Vec<Option<String>>`；`vec_connection_id`、`vec_row_count`、`vec_tidb_is_ddl_owner`、`vec_found_rows`、`vec_last_insert_id` 返回 `Vec<i64>`；`vec_tidb_version`、`vec_current_resource_group`、`vec_version` 返回 `Vec<String>`。
- 可失败的身份包装器：`vec_current_user`、`vec_current_role`、`vec_user` 先调用对应标量函数，成功后广播字符串，失败则返回 `ExpressionError`，不会产生部分结果列。
- `vec_benchmark(rows, loop_count, evaluate_child_vector)`：通过 `FnMut() -> Result<(), ExpressionError>` 抽象一次整列子表达式求值。
- `vec_last_insert_id_with_id(&mut SessionInfo, Vec<Option<i64>>)`：唯一直接修改会话状态的函数；参数列按所有权传入并原样返回。
- `vec_decode_key(&[Option<String>], Option<&dyn KeyCodec>)`：借助 `decode_key` 标量内核逐项收集为结果列。

## 执行流程

1. 对无行相关输入的信息函数，包装器先调用 [`builtin_info.rs`](builtin_info.rs) 中的 `database`、`connection_id`、`current_user`、`current_role`、`user`、`row_count` 等标量内核一次。
2. 若标量内核成功，`vec![value; rows]` 克隆该值形成结果列；`rows == 0` 时仍会先求标量值，然后返回空列。对会失败的身份函数，这意味着零行批次也可能因会话信息缺失而报错。
3. `vec_benchmark` 先拒绝 `loop_count <= 0`，再顺序调用闭包恰好 `loop_count` 次。即使 `rows == 0`，闭包也照常执行；任一轮失败立即停止并原样传播错误，全部成功后返回 `rows` 个零。
4. `vec_last_insert_id_with_id` 通过 `values.iter().rev().flatten().next()` 找到列中最后一个非 NULL 整数，将其以 Rust `as u64` 语义写入会话，再返回原 `values`；全 NULL 或空列不会修改会话。
5. `vec_decode_key` 按输入顺序调用标量 `decode_key(value.as_deref(), codec)` 并 `collect`。NULL 产生 NULL；无 codec 时非 NULL 原文返回；出现首个错误时终止且不返回部分列。

## 数据与状态

主要只读状态来自 `builtin_info.rs::SessionInfo`：当前数据库、连接 ID、上一语句影响行数与 `FOUND_ROWS`、认证/登录用户、活动角色、资源组和上一语句的自增 ID。`vec_current_resource_group` 继承标量内核“语句 hint 优先、会话默认值次之”的规则；`vec_current_role` 继承角色规范化、排序和空列表输出 `NONE` 的规则。

本文件唯一持久状态变化是 `vec_last_insert_id_with_id` 写入 `SessionInfo.last_insert_id`。它读取的无参 `vec_last_insert_id` 则来自 `previous_last_insert_id`，二者字段含义不同。测试还确认负 `i64` 写入时按 Rust 转换为对应 `u64` 位模式，例如 `-1` 成为 `u64::MAX`。

所有输出列由函数新建并拥有；字符串广播会克隆标量字符串，输入解码列只被借用。`InfoBuiltinKind` 是 `Copy` 的无状态枚举，不维护注册表或全局缓存。

## 依赖与调用关系

- crate 边界：[`Cargo.toml`](Cargo.toml) 声明包名 `astersql-expression`、库入口 `lib.rs` 且关闭自动测试发现；本模块没有直接使用第三方 crate。
- 模块装配：[`lib.rs`](lib.rs) 将本文件命名为私有的 `builtin_info_vec_kernel`，并在 `cfg(test)` 下装配 [`builtin_info_vec_test.rs`](builtin_info_vec_test.rs)。模块内函数虽为 `pub`，但因父模块私有，目前不是 crate 对外 API。
- 下游：本文件从 `builtin_info_kernel` 调用 `database`、`connection_id`、`tidb_version`、`row_count`、`current_user`、`current_resource_group`、`current_role`、`user`、`tidb_is_ddl_owner`、`found_rows`、`last_insert_id`、`version` 和 `decode_key`；还依赖 `SessionInfo` 与 `KeyCodec`。错误类型暂从 `builtin_ilike_kernel::ExpressionError` 共享。
- 上游：RustCodeGraph 对 `vec_benchmark`、`vec_last_insert_id_with_id`、`vec_decode_key` 展示的调用边落在 `builtin_info_vec_test.rs` 或综合 Aster 单元测试中；仓库文本检索也未发现生产 Rust 调用点。因此完整应用中的预期位置是“表达式向量派发之后、标量信息内核之前”，但生产接线状态为未验证/未接线证据不足，不能写成已接入。
- Go 主链：[`builtin_info_vec.go`](builtin_info_vec.go) 的接收者方法直接挂在各具体 builtin signature 上，输入为 `chunk.Chunk`、输出为 `chunk.Column`；Rust 文件目前用轻量 `Vec` 和闭包保留相同核心语义，没有移植 Go 的具体 Chunk/allocator 派发框架。

## 错误处理与边界

- `vec_current_user`、`vec_current_role`、`vec_user` 传播标量内核的 `ExpressionError::MissingSession`；`vec_database` 对空数据库不是错误，而是广播 NULL。
- `vec_benchmark` 将“不为正数”的前置条件表示为 `loop_count <= 0` 的 `InvalidArgument`。这比标量 `benchmark` 更严格：标量允许零次并返回 0、负数返回 NULL；向量入口只代表 Go 中 `constLoopCount > 0` 时才会选择的路径。Rust 函数签名本身不能判断参数是否来自常量，调用者仍须负责只在常量循环次数的派发路径调用它。
- `vec_benchmark` 不捕获闭包错误，也没有部分成功结果；错误前已经发生的子表达式副作用不会回滚。
- `vec_last_insert_id_with_id` 对全 NULL 或空输入保持现有会话值，对多个非 NULL 只选择行序最后一个。转换负值不是溢出错误，而是 Go `uint64(i64)` 对齐所需的位模式转换。
- `vec_decode_key` 对 NULL 不调用 codec；codec 缺失时返回原字符串；codec 错误立即传播。它不验证输入行数，也不做十六进制合法性判断，这些行为由注入的 `KeyCodec` 决定。
- 所有广播函数都接受 `rows == 0` 并返回空向量；但可失败标量会在分配空向量前求值，因此不能普遍假定零行一定成功。

## 并发与资源生命周期

文件中没有线程、异步任务、锁、事务、通道、全局可变变量或 `unsafe`。只读包装器仅借用 `&SessionInfo`，可变入口 `vec_last_insert_id_with_id` 需要独占 `&mut SessionInfo`，由 Rust 借用规则阻止同一时刻并发读写该快照。`KeyCodec` trait 在标量文件中要求 `Send + Sync`，所以实现可安全跨线程共享；本函数仅在调用期间借用它，不持有或释放外部资源。

各结果向量在函数返回时转移给调用者；发生错误时已创建的局部值按 RAII 自动释放。与 Go `vec_benchmark` 使用 `bufAllocator.get/put` 且 `defer` 归还临时列不同，Rust 抽象把子表达式临时缓冲生命周期交给闭包调用者，本文件自身不管理池化资源。

## 与 Go 版本的对应关系

[`builtin_info_vec.go`](builtin_info_vec.go) 是直接语义基准：数据库空串转 NULL、会话标量整列广播、角色排序拼接、DDL owner 转 0/1、带参 `LAST_INSERT_ID` 从后向前取最后一个非 NULL、解码逐行保留 NULL，均在 Rust 标量内核与本向量包装层分工复现。

主要结构差异如下：

- Go 每个具体签名都有 `vectorized()`；Rust 用集中式 `InfoBuiltinKind` 矩阵表达能力，三个不可向量化的 MVCC/编码键变体没有对应向量求值函数。
- Go 直接读 `EvalContext`/session variables 并写 `chunk.Column`；Rust 从显式 `SessionInfo` 快照读写并返回 `Vec`，DDL owner 状态和 `KeyCodec` 也由调用者注入。
- Go `BENCHMARK` 只有 `constLoopCount > 0` 才报告可向量化，按子表达式求值类型选择 `VecEval*`，并借还缓冲列；Rust 入口由闭包统一代表一次整列求值，并用错误保护确保调用者不会把零或负循环次数送入向量路径。
- Go `TIDB_DECODE_KEY` 的默认解码函数可结合 infoschema，且 Go 回调返回字符串；Rust `KeyCodec` 可返回 `ExpressionError`，因此额外具有显式的逐行失败路径。
- [`builtin_info_vec_test.go`](builtin_info_vec_test.go) 通过通用向量/标量一致性框架覆盖 SQL 类型和 `BENCHMARK` 的多种子表达式类型；Rust [`builtin_info_vec_test.rs`](builtin_info_vec_test.rs) 更直接地验证广播、错误、状态写回、能力矩阵与 codec 注入。

## 扩展指南

新增或调整信息函数向量化时，先在标量内核确认单行语义，再在本文件增加 `InfoBuiltinKind` 变体和对应包装器。必须显式决定是否可向量化：若不可向量化，应加入 `vectorized` 的排除匹配；若可向量化，应说明是“求值一次后广播”还是“逐行求值”，避免把依赖每行输入或具有顺序副作用的函数错误地广播。

会话写操作必须保持行序语义，并使用 `&mut SessionInfo` 明示独占修改；类似带参 `LAST_INSERT_ID` 的函数还需确认 NULL、空列、多非 NULL 和有符号到无符号转换。可失败的逐行转换应保留输入顺序、NULL 位置和首错传播。若接入真实 Chunk 执行框架，需要在独立生产模块完成派发和缓冲管理，不应在本文件内嵌测试或用简化桩代替。

测试应同步修改独立文件 [`builtin_info_vec_test.rs`](builtin_info_vec_test.rs)，至少覆盖零行、缺失会话、NULL、错误传播、状态副作用和能力矩阵；Go 对齐变化还应核对 [`builtin_info_vec.go`](builtin_info_vec.go) 与 [`builtin_info_vec_test.go`](builtin_info_vec_test.go)。Rust 单元测试不得写入生产源文件。

兼容性风险主要是 MySQL/TiDB 返回格式、NULL/错误差异和会话副作用；性能风险主要是大 `rows` 的字符串克隆、过大 `loop_count` 的同步循环、逐行动态分派 codec，以及未来若重复调用本应广播的昂贵标量函数。

## 验证依据

- RustCodeGraph `status`：索引覆盖当前仓库，包含 `pkg/expression/builtin_info_vec.rs`；`node --file ...` 读取到完整 172 行源码。
- RustCodeGraph `query/node`：确认 `InfoBuiltinKind`、`vec_benchmark`、`vec_last_insert_id_with_id`、`vec_decode_key` 的定义；`node` 的调用 trail 将关键入口指向 `builtin_info_vec_test.rs` 或综合 Aster 单元测试，未给出生产调用边。
- 源码：[`builtin_info_vec.rs`](builtin_info_vec.rs)（能力矩阵、广播、循环、状态更新、逐行解码）；[`builtin_info.rs`](builtin_info.rs)（`SessionInfo`、标量函数、`KeyCodec` 与 `ExpressionError` 传播语义）；[`lib.rs`](lib.rs)（模块与测试装配）。
- crate 配置：[`Cargo.toml`](Cargo.toml)（`astersql-expression` 包、`lib.rs` 入口、`autotests = false`、无本文件专属 feature）。
- Rust 测试：[`builtin_info_vec_test.rs`](builtin_info_vec_test.rs)（广播和零行、缺失会话、BENCHMARK 次数/错误、LAST_INSERT_ID 最后非 NULL 与负值、能力矩阵、解码 NULL/回退/错误、DDL owner 0/1）。
- Go 对照：[`builtin_info_vec.go`](builtin_info_vec.go)（各 signature 的向量实现与三个不可向量化例外）；[`builtin_info_vec_test.go`](builtin_info_vec_test.go)（通用向量一致性和 benchmark 覆盖）。
- 本任务是纯文档分析，按计划不运行 Cargo；结构验证用于确认目标文件存在且恰含 11 个固定二级章节。
