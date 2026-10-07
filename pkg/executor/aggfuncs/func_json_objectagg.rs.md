# `pkg/executor/aggfuncs/func_json_objectagg.rs`

## 文件定位

本文件属于 `astersql-executor-aggfuncs` crate；该 crate 由同目录 `Cargo.toml` 定义，入口是 `lib.rs`，并通过 `lib.rs::func_json_objectagg` 公开本模块。它提供 SQL `JSON_OBJECTAGG(key, value)` 所需的一种 Rust 部分聚合状态 `JsonObjectAgg`，负责把已经完成类型转换的 `(可空字符串键, SpillValue)` 流收集成对象成员表。

这个文件不是完整的聚合执行器。它不负责表达式求值、字段类型/字符集检查、把结果构造成 `BinaryJSON`、向结果 chunk 写值或 spill 编解码。Rust 的 `builder.rs::build_aggregate_function` 能把 `FunctionName::JsonObjectAgg` 识别为 `AggImplementation::JsonObjectAgg`，但 RustCodeGraph 对本文件的直接使用关系只列出 `func_json_objectagg_test.rs` 和 `go_scenario_coverage_test.rs`；当前证据未显示该枚举变体会实例化本文件的 `JsonObjectAgg`。因此应把它理解为已公开、已测试的状态逻辑，而不能据此宣称完整 SQL 执行链已经接通。

## 核心职责

- `JsonObjectAgg::update` 顺序接收键值对，拒绝空键，并以 `HashMap::insert` 实现重复键后写覆盖。
- `JsonObjectAgg::merge` 把源状态的所有成员克隆进目标状态；冲突键由源状态覆盖目标状态。
- `JsonObjectAgg::reset` 丢弃旧 map 并恢复为空状态；`result` 把空状态映射成 `None`，非空状态返回只读引用。
- `value_memory_delta` 按 Go `getValMemDelta` 的口径估算一个 `SpillValue` 的值内存，为 `update` 和 `merge` 提供记账数据。

职责边界很重要：调用者必须先把 SQL 值转换为 `SpillValue`，本文件也不验证二进制字符集键、参数个数或输出 JSON 的合法性。这些在 Go 版本中由 `jsonObjectAgg.UpdatePartialResult`、`getRealJSONValue` 和 `AppendFinalResult2Chunk` 承担。

## 主要符号

- `pub struct JsonObjectAgg`：唯一公开类型。内部私有字段 `entries: HashMap<String, SpillValue>` 保存对象成员；派生 `Clone`、`Debug`、`Default` 和 `PartialEq`，便于创建、复制和测试状态。
- `pub fn update(&mut self, v: impl IntoIterator<Item = (Option<String>, SpillValue)>) -> Result<i64, AggError>`：批量更新入口。返回本批新键对应的内存增量，遇到第一个 `None` 键即返回 `AggError`。
- `pub fn merge(&mut self, s: &Self) -> i64`：合并另一个 partial state，返回按源状态每一项计算的内存增量。
- `pub fn reset(&mut self)`：以新建 `HashMap` 替换旧表，而不是仅调用 `clear`，使旧表容量随旧值释放。
- `pub fn result(&self) -> Option<&HashMap<String, SpillValue>>`：空表返回 `None`，非空返回借用；不会复制或转移状态。
- `fn value_memory_delta(value: &SpillValue) -> i64`：模块私有计量函数。固定加入 `DEF_INTERFACE_SIZE`，再依据 `SpillValue` 变体加入定长大小或载荷长度。

本文件没有 trait、模块级常量、条件编译项或异步函数。

## 执行流程

`update` 的流程是：遍历输入；通过 `Option::ok_or_else` 将空键变成消息为 `JSON documents may not contain NULL member names` 的 `AggError`；在插入前用 `contains_key` 判断是否是新键；仅对新键累计 `key.len() + value_memory_delta(value)`；最后用 `insert` 写入，因此已有键的值会被覆盖。若一个批次前面已经写入若干成员、后面才遇到空键，函数直接报错，但不会回滚前面的写入，返回值也不携带此前已累计的增量。

`merge` 遍历源 `entries`。每一项都先累计键长和值内存，再克隆键和值并插入目标表；无论目标中是否已有同名键，都会报告该源项的增量。因为源表是 `HashMap`，跨 partial 的重复键由哪一侧覆盖取决于调用方向：`destination.merge(&source)` 保留 source 的值；源内部的遍历顺序不稳定，但不同键之间互不影响。

`result` 只区分空与非空：空聚合返回 `None`，对应 Go 最终输出 NULL 的状态判断；它本身不创建 JSON。`reset` 后 `result` 再次为 `None`。

## 数据与状态

核心状态是 `HashMap<String, SpillValue>`。键已被拥有，值使用 `aggfuncs.rs::SpillValue` 的封闭枚举，当前覆盖 `Bool`、`Int64`、`Uint64`、`Float64`、`String`、`BinaryJson`、`Opaque`、`Time` 和 `Duration`。因此类型集合比 Go 的动态 `any` 更窄；Go 在进入 map 前还会把 `float32`、decimal 和字节切片转换成兼容 JSON 的表示。

内存增量是逻辑记账值而非 allocator 的精确占用。`update` 对首次出现的键计入 UTF-8 字节长度以及值大小，对覆盖写返回零；它不计 `HashMap` 桶扩容，也不比较新旧值载荷大小。`merge` 则按 Go `MergePartialResult` 的约定，对源 partial 的每个条目报告键和值大小，即使该键会覆盖目标中的旧值。字符串使用 `len` 而非 `capacity`；`BinaryJson` 和 `Opaque` 在载荷长度外加一个类型码字节。

`result` 返回对内部 map 的共享借用，生命周期受 `&self` 约束；借用存在时 Rust 会阻止更新、合并或重置该状态。

## 依赖与调用关系

直接依赖均来自标准库或本 crate：`std::collections::HashMap` 提供状态容器；`crate::aggfuncs::SpillValue` 提供可落盘值类型；`AggError` 承载空键错误；`DEF_INTERFACE_SIZE`、`DEF_BOOL_SIZE`、`DEF_INT64_SIZE`、`DEF_UINT64_SIZE`、`DEF_FLOAT64_SIZE`、`DEF_TIME_SIZE` 和 `DEF_DURATION_SIZE` 定义 Go 对齐的计量常量。`Cargo.toml` 表明 crate 直接依赖 `astersql-types` 与 `astersql-util-serialization` 等组件，但本文件没有直接引用外部 crate。

上游静态关系为 `lib.rs` 声明公开模块；`func_json_objectagg_test.rs` 和 `go_scenario_coverage_test.rs` 导入并直接调用 `JsonObjectAgg`。`builder.rs::build_aggregate_function` 接受非 `Dedup` 的 JSON_OBJECTAGG 描述符并生成 `AggImplementation::JsonObjectAgg` 元数据，但当前图证据没有从该变体到本结构的实例化边。

相邻的 spill 路径操作的是 `aggfuncs.rs::JsonObjectPartialResult`，而非本文件的 `JsonObjectAgg`：`spill_serialize_helper.rs::serialize_json_object` 写出其键值对，`spill_deserialize_helper.rs::deserialize_json_object` 清空并重建该另一类型的 map。这两套相似状态目前不能在文档中视为同一对象。

## 错误处理与边界

唯一显式错误是空键：`update` 返回 `AggError`，错误消息与 MySQL/TiDB 的 NULL member name 约束对应。错误通过 `Result` 交给调用者处理，没有 panic 路径。由于更新不是事务性的，调用者若要求“整批成功或完全不变”，必须在外层预校验键或在临时状态上更新后再合并。

重复键不是错误；同一 `update` 批次中后出现的值覆盖先出现的值，`merge` 中源值覆盖目标值。空输入成功返回零且不改变状态。空状态通过 `result == None` 表示，而不是返回空 JSON 对象。

本文件不会拒绝二进制字符集键，也不会处理表达式求值错误、decimal 转换错误、无效 JSON 输出或不支持的第二参数类型。Go 文件中这些错误分别由 `UpdatePartialResult`、`getRealJSONValue` 和 `types.CreateBinaryJSONWithCheck` 等路径产生；为 Rust 补齐完整执行链时必须单独恢复这些约束。

## 并发与资源生命周期

类型没有内部锁、原子变量、任务或通道。所有修改方法都要求 `&mut self`，并发共享必须由上层分片状态或外部同步机制保证；文件本身不承诺多线程并发更新。`SpillValue` 和 `HashMap` 所有权随 `JsonObjectAgg` 生命周期管理，离开作用域时自动释放。

`update` 消费传入的键和值；`merge` 因只借用源状态而克隆每个键和值，源状态保持不变。`reset` 用新 map 替换旧 map，立即结束旧条目及旧容量的所有权。`result` 只暴露共享引用，避免调用者绕过更新方法修改 map。

## 与 Go 版本的对应关系

Rust `JsonObjectAgg.entries` 对应 Go `partialResult4JsonObjectAgg.entries` 的成员映射；`update` 对应 `jsonObjectAgg.UpdatePartialResult` 写入 map 之后的核心覆盖逻辑；`merge` 对应 `MergePartialResult`；`reset` 对应 `ResetPartialResult`；`value_memory_delta` 对应 `getValMemDelta`。两端都遵守 NULL key 报错、重复 key 后写覆盖、update 只对新键记增量、merge 对源条目逐项记增量的意图。

当前 Rust 文件是有意缩小的状态层，不等价于 Go 完整实现。Go 还包含 partial 分配的结构/桶内存、表达式求值、binary charset key 拒绝、`Datum` 到 JSON 值的规范化、最终 `BinaryJSON` 创建与 chunk 写入，以及 spill 的序列化/反序列化。Go `getValMemDelta` 还列出 decimal 与 `[]uint8` 分支，而 Rust `SpillValue` 不含这两个变体，因为相应值应在进入状态前完成转换。

测试对应关系方面，Rust `func_json_objectagg_test.rs` 精确覆盖重复键、NULL 键、update/merge 内存口径；`go_scenario_coverage_test.rs` 提供 reset/非空结果及简单多项状态场景。Go `func_json_objectagg_test.go` 还覆盖多种字段类型组合、merge 结果、初始 map 成本和更完整的内存矩阵，这些不能由现有 Rust 状态测试替代。

## 扩展指南

若只增加 `SpillValue` 变体，必须同步修改 `value_memory_delta`，并检查 `spill_serialize_helper.rs` 与 `spill_deserialize_helper.rs` 的值协议；在独立的 `func_json_objectagg_test.rs` 增加新类型的 update、覆盖和 merge 计量测试。不要把测试内嵌到生产文件。

若要把本结构接入真实聚合执行链，应先确认 `AggImplementation::JsonObjectAgg` 的实例化位置，明确是复用 `JsonObjectAgg` 还是统一到 `JsonObjectPartialResult`，避免维护两套 map 状态。接线必须保留 Go 的参数求值、charset、类型转换、NULL key、输出 JSON、spill 和内存追踪语义，并用独立 Rust 测试覆盖；不能仅凭 builder 枚举存在就省略这些步骤。

若改变重复键策略或 merge 方向，要同时评估 hash/并行 partial 合并的确定性。SQL 语义要求保留“最后遇到的值”，而并行 merge 的实际顺序需要由上层执行器提供明确保证。若需要批量原子性，则应新增显式 API 或先在临时 map 中完成校验，不能悄悄改变现有 `update` 的部分写入行为。

性能风险集中在每次新键前的 `contains_key + insert` 双重查找、merge 的全量克隆以及未计入的 map 扩容；优化时必须同步校准 Go 兼容的内存记账与测试预期。

## 验证依据

- 目标源码：`pkg/executor/aggfuncs/func_json_objectagg.rs`，核对了 `JsonObjectAgg`、`update`、`merge`、`reset`、`result` 和 `value_memory_delta` 的完整实现。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`node --file ...func_json_objectagg.rs` 返回完整 78 行源码，并报告直接使用者为 `func_json_objectagg_test.rs`、`go_scenario_coverage_test.rs`；`query JsonObject` / `query ObjectAgg` 定位了结构、builder 枚举、Go 对照、测试与 spill helper 符号。
- crate 与接线：`pkg/executor/aggfuncs/Cargo.toml`、`lib.rs`、`builder.rs::build_aggregate_function`、`aggfuncs.rs::SpillValue` / `JsonObjectPartialResult`。
- spill 边界：`spill_serialize_helper.rs::serialize_json_object` 和 `spill_deserialize_helper.rs::deserialize_json_object`，确认它们使用 `JsonObjectPartialResult` 而非本结构。
- Rust 测试：`pkg/executor/aggfuncs/func_json_objectagg_test.rs` 与 `pkg/executor/aggfuncs/go_scenario_coverage_test.rs`。
- Go 对照与测试：`pkg/executor/aggfuncs/func_json_objectagg.go`、`pkg/executor/aggfuncs/func_json_objectagg_test.go`。
- 本任务为纯文档分析，按计划不运行 Cargo；最终使用任务文件指定的命令验证目标文件存在且恰有 11 个固定二级标题，并人工复核所有“已接线/已支持”表述均受上述直接证据约束。
