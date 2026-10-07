# `pkg/expression/builtin_json_vec.rs`

## 文件定位

本文件属于 `astersql-expression` crate，是 Go `pkg/expression/builtin_json_vec.go` 的 Rust 向量化 JSON 运算内核。crate 根在 `pkg/expression/Cargo.toml` 中以 `lib.rs` 声明；`pkg/expression/lib.rs:231-232` 将本文件装配为私有模块 `builtin_json_vec_kernel`。

当前接线必须特别区分：`builtin_json_vec_kernel` 会进入普通 crate 编译，但 `lib.rs:770-773` 只在 `#[cfg(test)]` 下通过 `expression_json_vec` 重导出它的公开符号。RustCodeGraph 查到的直接使用者主要是 `builtin_json_vec_test.rs`、`builtin_json_vec_13_aster_unit_test.rs` 等测试。因此，这里已经实现并验证了可复用内核，但尚未像 Go 的各 `builtin*Sig.vecEval*` 方法那样直接挂到生产表达式调度与 `chunk.Column` 执行链；不能把“函数存在”表述为“Rust SQL 主链已接入”。

文件内没有条件编译项、全局可变状态或模块级运行时常量；唯一常量函数 `vectorized()` 固定返回 `true`，用于对应 Go 各签名的 `vectorized() bool` 声明。

## 核心职责

- 用 `Vec<Option<T>>` 表示列并逐行计算 JSON 内置函数。外层 `None` 是 SQL `NULL`，而 `Some(BinaryJSON(JSON null))` 是真实 JSON 值；这一区分贯穿构造、修改和合并逻辑（`JsonColumn`、`json_null`、`vec_json_modify`）。
- 提供 JSON 修改与路径操作：SET/INSERT/REPLACE、ARRAY_INSERT/ARRAY_APPEND、EXTRACT/REMOVE、KEYS、LENGTH、CONTAINS_PATH 和 SEARCH。
- 提供构造、比较和元数据操作：ARRAY/OBJECT、MEMBER OF、CONTAINS、OVERLAPS、TYPE、DEPTH、STORAGE_SIZE/FREE。
- 提供字符串与合并操作：QUOTE/UNQUOTE/PRETTY、MERGE_PRESERVE/旧 `JSON_MERGE` 警告、MERGE_PATCH，以及 JSON 数组元素的 SUM_CRC32。
- 在列形状不一致、路径非法、模式参数非法或底层 JSON 运算失败时，以 `JsonVecError` 统一向上传播错误，而不是静默截断列或跳过行。

## 主要符号

- 列与结果类型：`JsonColumn = Vec<Option<BinaryJSON>>`、`StringColumn`、`IntColumn`；`JsonVecResult<T>` 与 `JsonVecError` 构成此内核的错误边界。
- 编解码辅助：公开的 `parse_json`、`json_value` 便于把文本、`BinaryJSON` 与 `serde_json::Value` 互转；内部 `binary_json`、`json_null` 完成反向转换和 JSON null 构造。
- 参数守卫：`parse_path` 可按调用点决定是否允许通配符/范围；`same_len`、`columns_len`、`pair_columns_len` 保证同一批次所有参数列逐行对齐，且 path/value 数量成对。
- 修改族：`vec_json_modify` 是 SET/INSERT/REPLACE 的公共实现，`vec_json_insert`、`vec_json_replace`、`vec_json_set` 仅选择 `JSONModifyType`。`vec_json_array_insert` 与 `vec_json_array_append` 按参数对顺序修改每一行。
- 查询与比较族：`vec_json_extract`、`vec_json_remove`、`vec_json_keys(_at_path)`、`vec_json_length`、`vec_json_contains(_path)`、`vec_json_search`、`vec_json_member_of`、`vec_json_overlaps`。
- 构造与格式族：`vec_json_array`、`vec_json_object`、`vec_json_quote`、`vec_json_unquote`、`vec_json_pretty`。
- 合并族：`vec_json_merge_with_warnings` 返回 `JsonMergeOutcome { values, warnings }`；`vec_json_merge` 丢弃警告只返回值；`vec_json_merge_patch` 保留 SQL NULL 指针语义交给底层合并函数。
- CRC32：`JsonCrc32Type` 定义 Signed/Unsigned/Real/String 四种元素转换，`crc32_cast` 与 `vec_json_sum_crc32` 逐元素转换、哈希并用 wrapping addition 累加。
- 外部 JSON API：本文件重导出 `BinaryJSON`、JSON 路径和修改类型，并调用 `types-json-functions` 的解析、比较、包含、合并、修改、搜索和反引号解码能力。

## 执行流程

典型入口遵循“校验列形状 → 为结果预分配 → 按行处理 SQL NULL → 解析路径/转换值 → 调底层 JSON API → 写入同位置结果”的流程。例如 `vec_json_modify` 先用 `pair_columns_len` 验证参数，再逐行处理：文档为 SQL NULL 时直接输出 NULL；任一路径为 SQL NULL 时整行输出 NULL；值为 SQL NULL 时改写成 JSON null；最后一次调用 `BinaryJSON::Modify` 处理该行的全部 path/value 对。

路径函数按 SQL 约束选择 `parse_path` 模式。`vec_json_extract`、`vec_json_remove`、`vec_json_search`、`vec_json_contains_path` 允许可匹配多个值的路径；要求单位置语义的 CONTAINS 带路径、ARRAY_INSERT、KEYS 带路径和 LENGTH 带路径则传入 `allow_multiple=false`，遇 `*` 或范围立即报错。路径不存在时，各函数按 Go 语义分别返回 NULL、保持原文档或给出布尔结果。

ARRAY/OBJECT 构造器逐行收集参数：值侧 SQL NULL 转成 JSON null；OBJECT 的键侧 SQL NULL 是整批错误。`vec_json_array_append` 先提取目标；目标缺失时保持文档，标量目标先装箱为数组，追加的数组值还会额外包一层，因此 `[1]` 追加 `[2,3]` 得到 `[1,[2,3]]` 而不是摊平。

MERGE 每行只要任一参数为 SQL NULL 就输出 NULL；旧别名模式对每个非 NULL 结果行追加一条弃用警告。MERGE_PATCH 把每行参数转为 `Option<&BinaryJSON>`，由 `MergePatchBinaryJSON` 决定补丁结果是否仍为 SQL NULL。

## 数据与状态

所有业务状态都位于函数栈和返回值中。输入切片只读，修改类函数按行克隆文档后生成新 `BinaryJSON`，不会原地修改调用者列。结果向量通常用输入行数 `with_capacity` 预分配，并保持输入顺序与一行一结果的不变量。

`BinaryJSON` 是底层二进制表示；对象键排序、ARRAY/OBJECT 构造、PRETTY 和部分比较需要暂时转换成 `serde_json::Value`。`object_keys` 显式排序键，保证结果稳定。布尔 SQL 结果使用 `IntColumn` 的 `Some(0)`/`Some(1)`，SQL NULL 使用 `None`。

唯一额外状态是 `JsonMergeOutcome.warnings`，它与值列分离，不依赖 session warning 容器。与 Go 不同，本文件不持有 `EvalContext`、`chunk.Chunk`、`chunk.Column` 或 `columnBufferAllocator`，也没有可复用列缓冲池。

## 依赖与调用关系

直接依赖由 `pkg/expression/Cargo.toml` 佐证：`crc32fast = "1"` 用于 SUM_CRC32，`serde_json = "1"` 用于中间 JSON 树和字符串格式化，`types-json-functions` 指向 `pkg/types/internal/json_functions` 并提供所有 `BinaryJSON` 核心操作。

内部调用集中在少数枢纽：SET/INSERT/REPLACE → `vec_json_modify` → `parse_path`/`BinaryJSON::Modify`；ARRAY_APPEND → `append_json_array` → Extract/Merge/Modify；带路径的查询 → `parse_path` → Extract；所有底层错误 → `json_result` → `JsonVecError`。RustCodeGraph 还确认 `json_result`、`parse_path`、`same_len` 分别被多个公开入口复用。

上游方面，`pkg/expression/lib.rs` 装配模块；当前可见调用者是 `pkg/expression/builtin_json_vec_test.rs` 和 `pkg/expression/builtin_json_vec_13_aster_unit_test.rs`，另有跨模块测试使用重导出的 `BinaryJSON` 辅助。没有找到生产 Rust 调度器调用这些 `vec_json_*` 函数的调用边。Go 对照的上游则是各 JSON builtin signature，由表达式引擎在 `VecEvalJSON`、`VecEvalInt` 或 `VecEvalString` 路径中调用。

## 错误处理与边界

`JsonVecError` 只保存格式化后的消息；`json_result` 会丢失底层具体错误类型，但保留可读文本。列长度不一致、path/value 数量不等、OBJECT 参数不成对、NULL 对象键、SEARCH/CONTAINS_PATH 模式非法、ESCAPE 长度大于一、CRC32 输入非数组或元素无法转换，均返回 `Err` 并终止整个调用。

SQL NULL 通常按行传播；需要构造 JSON 值的位置则转成 JSON null。文档 NULL、路径 NULL、值 NULL 的规则不能合并成一个通用判断：例如修改函数的 NULL 路径使整行 NULL，但 NULL 值写入 JSON null；OBJECT 的 NULL 键报错；路径提取不到值通常返回 NULL；ARRAY_APPEND 的缺失路径保留原文档。

`json_null` 和根据类型码调用 `as_object`/`as_array` 的位置使用 `expect`，依赖底层库不变量：JSON null 永远可表示，且 `TypeCode` 与 payload 类型一致。若损坏的 `BinaryJSON` 打破这些不变量会 panic。STORAGE_SIZE 使用 `saturating_add(1)` 后再转 `i64`；在现实平台上载荷不可能接近溢出，但该转换没有显式上界错误。SUM_CRC32 使用 `wrapping_add`，有意保留固定宽度累加行为。

## 并发与资源生命周期

本文件没有锁、线程、异步任务、通道、事务或共享可变单例。所有函数都只借用输入并拥有局部结果，因此自身可重入；实际跨线程能力仍取决于 `BinaryJSON` 及调用者如何共享列，本文未据此声明额外的 `Send`/`Sync` 保证。

每行产生的临时路径列表、serde JSON 树和克隆值在该行完成后释放；结果列和警告列表转移给调用者。与 Go 实现的重要资源差异是：Go 从 `columnBufferAllocator` 获取 `chunk.Column` 并用 `defer put` 归还，Rust 内核直接接收已经求值完成的列并分配 `Vec`，没有缓冲池归还协议。因此未来接入生产向量执行时，应由上层负责子表达式求值、批次内存核算和列缓冲复用。

## 与 Go 版本的对应关系

Rust 入口按功能对应 `pkg/expression/builtin_json_vec.go` 的 `vecJSONModify` 及各 `builtinJSON*Sig.vecEval*`：NULL 传播、JSON null 转换、路径多选限制、SEARCH 的 one/all 与 ESCAPE、OBJECT NULL 键、MERGE 旧别名逐非 NULL 行告警、ARRAY_APPEND 嵌套数组语义都按 Go 流程移植。`pkg/expression/builtin_json_vec_test.go` 的 benchmark case 覆盖 JSON_KEYS、ARRAY_APPEND、CONTAINS_PATH、EXTRACT、LENGTH、ARRAY_INSERT、CONTAINS、OBJECT、SET/SEARCH/REPLACE/REMOVE/MERGE 等签名形状；Rust 两个独立测试文件以具体结果补充这些语义。

两端架构并非一一同形：Go 方法直接拥有 signature、`EvalContext`、子 `Expression`、输入 `chunk.Chunk`、输出 `chunk.Column` 和 allocator；Rust API 接收已物化的强类型列，没有 signature 结构、表达式求值、session warning sink 或 chunk 内存复用。Rust 的弃用警告以 `JsonMergeOutcome` 返回，Go 则写入 `typeCtx(ctx).AppendWarning`。Rust 错误归一为字符串型 `JsonVecError`，Go 保留 TiDB 的结构化错误码/错误类型。

此外，Rust 文件提供 `vec_json_overlaps` 和 `vec_json_merge`/`vec_json_merge_with_warnings` 的函数式拆分，而 Go 文件按 signature 方法组织；`vectorized()` 常量函数只是对“这些签名在 Go 都声明可向量化”的概括，不是生产注册表。任何声称完全替代 Go 路径的结论目前均未验证。

## 扩展指南

新增 JSON 向量函数时，优先复用 `same_len`/`columns_len`/`pair_columns_len`、`parse_path` 和 `json_result`，并先明确三类 NULL（文档、路径、值）的不同语义。若函数接受单位置路径，必须拒绝 `CouldMatchMultipleValues()`；若接受多路径结果，则应明确空路径列表、缺失路径和任一 NULL 路径的结果。

修改现有能力时，最可能的接入点是：公共修改语义改 `vec_json_modify`；路径规则改 `parse_path` 或对应调用点；ARRAY_APPEND 改 `append_json_array`；MERGE 警告改 `vec_json_merge_with_warnings`；CRC 类型转换改 `crc32_cast`。不要把测试嵌入生产源文件，应同步更新独立的 `pkg/expression/builtin_json_vec_test.rs` 和更完整的 `builtin_json_vec_13_aster_unit_test.rs`，必要时也核对 Go 的 `builtin_json_vec_test.go` case。

若要真正接入 Rust SQL 主链，需要在本文件之外增加 signature/dispatcher 到这些列函数的适配，并处理 `EvalContext`、子表达式求值、结构化错误、警告写入与列缓冲生命周期；这属于接线工作，不能仅把 `expression_json_vec` 的测试专用重导出改成公开就视为完成。性能方面应关注每行路径重复解析、`BinaryJSON` 与 `serde_json::Value` 往返、文档克隆和小向量分配，优化时必须维持 Go 的逐行错误顺序与 NULL 行为。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；本文件完整读取为 895 行。
- RustCodeGraph `node --file pkg/expression/builtin_json_vec.rs`：核对全部类型、函数、实现、注释与内部调用；图谱显示它被 `builtin_json_vec_test.rs`、`builtin_json_vec_13_aster_unit_test.rs` 等测试使用。
- RustCodeGraph 对 `vec_json_modify`、`json_result`、`parse_path`、`same_len` 等符号的 query/explore：核对公共入口、辅助函数复用和主要调用边；未发现生产 Rust 表达式调度器的直接 caller。
- `pkg/expression/lib.rs:226-232,503-514,762-773`：核对模块装配、独立测试模块，以及仅测试环境存在的 `expression_json_vec` 重导出。
- `pkg/expression/Cargo.toml`：核对 crate 名称、`lib.rs` 根及 `crc32fast`、`serde_json`、`types-json-functions` 依赖。
- `pkg/expression/builtin_json_vec.go`：逐段核对 Go 的 chunk/allocator 流程、NULL/路径/错误/告警语义与函数覆盖面。
- `pkg/expression/builtin_json_vec_test.go`：核对 Go 向量签名 case 与参数形状；`pkg/expression/builtin_json_vec_test.rs` 和 `pkg/expression/builtin_json_vec_13_aster_unit_test.rs`：核对 Rust 的修改、元数据、构造、比较、搜索、路径、合并、CRC32 及错误边界。
- 本任务只新增说明文档，不修改运行时代码，按计划不运行 Cargo。交付前使用任务规定的命令验证十一章结构，并人工检查未把测试可调用能力误写成生产已接线。
