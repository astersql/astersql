# `pkg/types/json_binary_functions.rs`

## 文件定位

该文件实现 AsterSQL Rust 侧的 MySQL/TiDB 二进制 JSON 运算核心。它不是独立模块：`pkg/types/internal/json_functions/lib.rs` 先定义 `BinaryJSON`、JSON 类型码、修改模式和 `JSONPathExpression`，再用 `include!` 把本文件纳入 `astersql-types-json-functions` 子 crate；顶层 `pkg/types/lib.rs` 又将该 crate 再导出为 `json_functions`。因此表达式层可以经 `astersql_types::json_functions` 或直接依赖 `types-json-functions` 使用这些 API。

该层位于“SQL JSON 内建函数适配”和“二进制 JSON 存储表示”之间：例如 `pkg/expression/builtin_json.rs`、`pkg/expression/builtin_json_vec.rs` 将 JSON_EXTRACT、JSON_SET/INSERT/REPLACE、JSON_REMOVE、JSON_MERGE、JSON_CONTAINS、JSON_OVERLAPS、JSON_SEARCH 等 SQL 语义委托给本文件；`pkg/util/codec/codec.rs` 使用 `PeekBytesAsJSON` 划分编码流中的单个 JSON 值；会话运行时 `pkg/session/runtime/relational_value.rs` 使用转换、比较和 Merge Patch 能力。

## 核心职责

1. 读写二进制 JSON 布局。容器载荷由 8 字节头、对象 key entry、value entry 和尾部实际数据组成；字面量直接内联在 value entry，其余值通过小端偏移定位（`HEADER_SIZE`、`KEY_ENTRY_SIZE`、`VALUE_ENTRY_SIZE`、`value_entry`）。
2. 在 `serde_json::Value` 与 `BinaryJSON` 之间转换，并提供 `Serialize`、`Type`、`Unquote` 和文本格式化（`CreateBinaryJSON`、`BinaryJSONToSerde`、`format_binary_json`）。
3. 执行 JSON Path 读取、写入、删除、数组插入、遍历和字符串搜索（`Extract`、`Modify`、`ArrayInsert`、`Remove`、`Walk`、`Search`）。路径语法本身由包含本文件的 `internal/json_functions/lib.rs::ParseJSONPathExpr` 解析。
4. 实现 MySQL/TiDB JSON 的排序、合并、包含、重叠和深度规则（`CompareBinaryJSON`、`MergePatchBinaryJSON`、`MergeBinaryJSON`、`ContainsBinaryJSON`、`OverlapsBinaryJSON`、`GetElemDepth`）。
5. 对截断载荷、非法变长整数、非法路径用途、过深文档和过长对象键给出 `JsonBinaryError`，并用 `PeekBytesAsJSON` 在无需完整解码时计算序列化值长度。

## 主要符号

- `JsonBinaryError(String)`：本文件公开的轻量错误类型；构造器 `new` 仅在文件/包含模块内部使用，错误信息通过 `Display` 暴露。
- 布局常量：`HEADER_SIZE = 8`、`DATA_SIZE_OFFSET = 4`、`KEY_ENTRY_SIZE = 6`、`VALUE_ENTRY_SIZE = 5`、`MAX_JSON_DEPTH = 100`。`floatEpsilon = 1e-8` 则用于整数/浮点混合比较时容忍精度损失。
- 编解码辅助：`read_u16/u32/u64`、`append_uvarint`、`read_uvarint`、`value_payload_len`、`value_entry`、`array_elements`、`object_entries`；构造端由 `build_binary_elements`、`buildBinaryJSONArray`、`buildBinaryJSONObject` 重新生成合法偏移和总长度。
- 转换与呈现：`CreateBinaryJSON(serde_json::Value)`、`BinaryJSONToSerde(&BinaryJSON)`；`BinaryJSON::{Serialize, Type, Unquote}`；`UnquoteString`。`DecodeOneEscapedUnicodeForTest`、`UnquoteJSONStringForTest`、`QuoteJSONStringForTest` 是为独立测试暴露的薄包装。
- 路径读取：`extract_recursive` 解释 key、数组单下标/范围/通配符与 `**`；公开入口 `BinaryJSON::Extract` 根据路径数和 `CouldMatchMultipleValues` 决定返回单值还是自动包装数组。
- 路径修改：`modify_recursive` 和 `remove_recursive` 以不可变输入、重建容器的方式工作；公开入口为 `BinaryJSON::{Modify, ArrayInsert, Remove}`。`JSONModifyInsert/Replace/Set` 决定“仅缺失时插入”“仅存在时替换”或两者兼有。
- 比较与集合关系：`precedence`、`numeric_compare`、`compare_binary_json` 支撑 `CompareBinaryJSON`；后者又是 `ContainsBinaryJSON` 和 `OverlapsBinaryJSON` 的标量相等判据。
- 合并：`merge_patch` / `MergePatchBinaryJSON` 实现 RFC 7396 风格补丁；`merge_array`、`merge_objects` / `MergeBinaryJSON` 实现旧式 JSON_MERGE_PRESERVE 语义。
- 遍历搜索：`callback_extract` 先按可选起始路径定位，`walk_tree` 深度优先遍历并按完整路径去重；`BinaryJSON::Walk` 对外承载回调，`Search` 通过 `like_match` 在字符串叶子上执行 SQL LIKE 风格匹配。

## 执行流程

创建值时，`CreateBinaryJSON` 先按 serde 类型选择类型码；字符串写入 uvarint 长度前缀，数组递归转换后调用 `buildBinaryJSONArray`，对象递归转换并由 `buildBinaryJSONObject` 按 key 字节序排序。构造完成后调用 `GetElemDepth`，深度超过 `MAX_JSON_DEPTH + 1` 即报错。反向转换由 `BinaryJSONToSerde` 根据类型码读取固定长度、变长字符串或容器 entry 并递归组装 serde 值。

抽取时，`Extract` 对每条路径调用 `extract_recursive`。数组选择先由 `selection_range` 把正下标、`last` 相对下标、范围或 `*` 化成闭区间；对象确定 key 时利用已排序 entry 二分查找；`**` 先尝试当前节点，再带着原路径递归所有子节点。结果按遍历身份去重；没有结果返回 `None`，单条确定路径的唯一结果直接返回，其余结果编码为数组。

修改时，`Modify` 先验证路径和值等长并拒绝 `*`、范围和 `**`，然后按调用顺序把每一对路径/值应用到上一步结果。`modify_recursive` 只沿实际存在的中间容器下降，在叶节点应用 Insert/Replace/Set；容器每次通过构造函数重新编码，所以原值不被原地破坏。`ArrayInsert` 先抽取父数组、夹紧插入位置，再以 `JSONModifySet` 写回；`Remove` 同样逐路径重建父容器，缺失路径保持不变。

比较时，`compare_binary_json` 先比较类型优先级，再按具体类型处理：数值允许有符号、无符号、浮点混比；数组逐元素字典序比较；对象先比成员数，再按排序后的 key/value 比较； opaque 比原始缓冲区；时间和 duration 比编码数值。合并时，Merge Patch 从最右侧的非对象或空值处截断无效前缀，随后依次应用补丁；普通 Merge 将相邻对象递归按键合并，最后把数组展平并把标量/对象自动包装进结果数组。

`Walk` 从根或指定起始路径进行 DFS，先回调当前节点，再递归数组元素或对象值，并以路径字符串防止重复回调；回调返回 `true` 会全局提前停止。`Search` 只匹配字符串叶子，`one` 在首个命中停止，`all` 收集所有命中路径；零、一、多项分别返回 `None`、字符串 JSON、字符串数组 JSON。`like_match` 用二维动态规划实现 `%`、`_` 和转义字符，其中 `_` 消耗一个 Unicode 字符而非一个 UTF-8 字节。

## 数据与状态

核心值 `BinaryJSON` 只有 `TypeCode: u8` 和拥有所有权的 `Value: Vec<u8>`（定义见 `pkg/types/internal/json_functions/lib.rs`）。数组/对象 entry 内保存的是相对该容器载荷起点的偏移；对象 key 必须以字节序排序，这是 `object_search_key` 可以二分查找的前提。`buildBinaryJSONObject` 会主动排序并拒绝长度大于 `u16::MAX` 的 key。

本文件没有全局可变状态。所有写操作返回新的 `BinaryJSON`；递归过程使用局部 `Vec`、`BTreeMap` 或 `HashSet`。`BTreeMap` 同时保证 Merge Patch/对象合并输出稳定有序；抽取使用 `Vec<usize>` 记录当前节点身份；Walk 使用 `HashSet<String>` 按完整 JSON Path 去重。

空间与时间上，读取容器通常会通过 `array_elements` / `object_entries` 克隆所有子值，修改还会重编码沿途容器。对象 lookup 在 materialize 完整 entry 列表后才做二分查找，理论查找虽为 O(log n)，但当前整体仍含 O(n) 解码/复制成本。`like_match` 的表大小为 `(pattern_chars + 1) × (value_chars + 1)`；深层路径、Merge、Contains 和 Walk 都使用递归调用栈。

## 依赖与调用关系

- crate 装配：`pkg/types/internal/json_functions/lib.rs` 定义类型/路径解析并 `include!` 本文件；其 `Cargo.toml` 直接依赖 `base64` 和启用 `preserve_order` 的 `serde_json`。`pkg/types/Cargo.toml` 再以路径依赖 `types-json-functions`，并从 `pkg/types/lib.rs` 再导出模块。
- 上游 SQL 层：`pkg/expression/builtin_json.rs` 是标量 JSON 内建函数适配器，`pkg/expression/builtin_json_vec.rs` 是向量化适配器；它们解析参数和 SQL NULL 后调用本文件的 Extract/Modify/Remove/Merge/Contains/Overlaps/Search。RustCodeGraph 对 `MergePatchBinaryJSON` 的精确 callers 查询还定位到 `pkg/session/runtime/relational_value.rs::as_str`。
- 上游编码/执行层：`pkg/util/codec/codec.rs` 调用 `PeekBytesAsJSON` 切分字节流；`pkg/executor/aggfuncs/func_max_min.rs` 与 `func_max_min_count.rs`、`pkg/executor/internal/vecgroupchecker/vec_group_checker.rs` 使用 `CompareBinaryJSON` 决定 JSON 顺序。
- 下游：RustCodeGraph 的 callee 结果确认 `MergePatchBinaryJSON -> merge_patch`，`ContainsBinaryJSON -> array_elements/object_entries/object_search_key/CompareBinaryJSON`，`Search -> Walk/CreateBinaryJSON/like_match`。内部进一步依赖二进制读取与构造函数以及 `JSONPathExpression` 的路径操作。
- 图索引限制：对方法调用和跨 crate 再导出的部分 callers，RustCodeGraph 没有返回完整集合；上述表达式层和 codec 调用因此由精确源码检索补充，不能把空 callers 结果理解成 API 未接线。

## 错误处理与边界

所有可恢复的底层解码、转换、路径修改、遍历操作原则上返回 `Result<_, JsonBinaryError>`。明确边界包括：截断的定长数/entry/字符串/opaque、无效或溢出的 uvarint、未知类型码、对象 key 超过 65535 字节、文档过深、Modify 参数数不一致、修改/删除路径含多值选择、Remove 空路径、ArrayInsert 非数组单元路径、Search 模式不是 `one`/`all`。空或越界但语义允许的路径通常不是错误，而是返回 `None` 或原值。

有几处 API 假设输入已由可信构造/解码路径验证，扩展时必须留意：`CompareBinaryJSON` 对内部错误使用 `expect("valid BinaryJSON")`，`Type` 遇到未知类型码会 panic；`ContainsBinaryJSON`、`OverlapsBinaryJSON` 和 `GetElemDepth` 会把部分容器解码错误分别降为 `false` 或空子集。`BinaryJSONToSerde` 对非法 UTF-8 字符串/key 使用替换字符，opaque 被呈现为 `base64:typeN:...` 字符串，时间类型转成原始整数，duration 只读取其前 8 字节；这些是当前 Rust 转换层的真实边界，不能等同于 Go 完整类型对象的无损往返。

深度检查仅在 `CreateBinaryJSON` 与 `Modify` 的最终结果上执行；直接手工构造 `BinaryJSON`、Merge、ArrayInsert 或 Remove 不会统一重新验证所有结构约束。`PeekBytesAsJSON` 计算声明长度但不保证输入缓冲区实际拥有这么多字节，调用者仍需自行检查切片边界。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、事务、通道或外部 I/O。所有输入通过共享引用读取，返回值拥有自己的 `Vec<u8>`；回调在 `Walk` 的同步调用栈内执行，返回后不被保存。因此同一 `BinaryJSON` 可由调用方按其类型约束并发只读使用，本文件自身没有共享可变状态或取消协议。

资源生命周期主要是临时分配：容器展开、路径结果、合并映射和重编码缓冲区在一次调用内创建并随返回/作用域释放。Walk 的 `seen` 持续到整个遍历结束，Search 的命中路径持续到结果 JSON 构造完毕。回调错误通过 `?` 立即终止遍历；回调请求停止也会短路剩余分支。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/types/json_binary_functions.go`，独立测试是 `pkg/types/json_binary_functions_test.go`。公开功能基本一一对应：Go 的 `BinaryJSON.Type/Unquote/Extract/Modify/ArrayInsert/Remove/GetElemDepth/Search/Walk` 及 `CompareBinaryJSON`、`MergePatchBinaryJSON`、`MergeBinaryJSON`、`PeekBytesAsJSON`、`ContainsBinaryJSON`、`OverlapsBinaryJSON` 均在 Rust 文件中有同名或等价入口；`floatEpsilon`、Unicode 代理对、对象 key 排序、路径自动包装、RFC 7396 null 删除、LIKE Unicode 字符匹配等规则保持一致。

实现策略存在可观察或维护层面的差异：Go 修改器通过 `binaryModifier` 记录目标指针并只重建相关二进制结构，Rust 的 `modify_recursive` / `remove_recursive` 展开并重编码沿途容器；Go 使用包级结构化错误，Rust 目前用消息字符串；Go 的 `CreateBinaryJSON(any)` 支持时间、duration、opaque 等包内类型，而本文件的 `CreateBinaryJSON` 只接收 `serde_json::Value`，额外类型由其他 Rust 模块构造；Rust 特有的 `BinaryJSONToSerde` 是适配表达式/会话层的有损桥接。Go Merge 对空输入的内部前置条件较强，Rust `MergeBinaryJSON(&[])` 明确定义为空数组，但 `MergePatchBinaryJSON(&[])` 返回错误以避免 Go 版本可能的越界前置条件。

测试对应关系也分两层：`pkg/types/json_binary_functions_test.rs` 逐表移植 Go 文件中的 Unicode、Unquote、Merge 基准桩和 Compare 用例，并新增 JSON_SEARCH 的 Unicode `_` 回归；`pkg/types/json_binary_functions_6_aster_unit_test.rs` 额外覆盖 Extract→Modify→Remove→ArrayInsert、跨数值比较、Merge/Contains/Overlaps/Depth/Peek、Merge Patch、Walk 和 Search 的组合行为。测试逻辑保持在独立文件，未嵌入生产源文件。

## 扩展指南

- 新增类型码或改变布局时，应同步修改 `value_payload_len`、`value_entry`、`BinaryJSONToSerde`、`Type`、`precedence`、`compare_binary_json`、`PeekBytesAsJSON` 以及构造端；同时核对 `pkg/types/internal/json_functions/lib.rs` 的类型码定义和 Go 的 `json_binary.go` / `json_binary_functions.go`。布局修改还会影响 `pkg/util/codec/codec.rs`。
- 新增 JSON Path 选择形式时，先在 `internal/json_functions/lib.rs` 更新解析与显示，再同步 `selection_range`、`extract_recursive`、`has_multiple_selection`、`modify_recursive`、`remove_recursive` 和 `callback_extract`；明确它对“允许多值读取、禁止多值写入”的影响。
- 新增 SQL JSON 操作通常先在本文件提供纯 `BinaryJSON` 语义，再接入 `pkg/expression/builtin_json.rs` 和必要的 `builtin_json_vec.rs`。不要在本文件处理 SQL NULL、类型推断或错误码映射，那些属于表达式适配层。
- 比较/包含/重叠改动要同时检查数值混比、对象排序、数组递归和 opaque/时间类型；性能改动须警惕当前完整 materialize 与 clone 行为，并保持错误边界和对象有序不变量。
- 测试应继续放在独立文件：基础 Go 对照用例更新 `pkg/types/json_binary_functions_test.rs`，Rust 组合/回归场景更新 `pkg/types/json_binary_functions_6_aster_unit_test.rs`，SQL 可见行为还应同步相应 `pkg/expression/*_test.rs`。不要把 `#[cfg(test)]` 测试追加进本生产文件。

## 验证依据

- 源码与装配：完整读取 `pkg/types/json_binary_functions.rs`；读取 `pkg/types/internal/json_functions/lib.rs` 的类型、路径解析与 `include!`；读取 `pkg/types/internal/json_functions/Cargo.toml`、`pkg/types/Cargo.toml`、`pkg/types/lib.rs`。`pkg/types` 下不存在 `doc.go`。
- RustCodeGraph：`status` 显示索引含 11,467 文件、307,296 节点和 1,848,419 条边；按文件 `node` 读取了目标文件 1–1627 行。精确查询确认 `MergePatchBinaryJSON` 的调用者含会话运行时和独立测试，并确认 `MergePatchBinaryJSON -> merge_patch`、`ContainsBinaryJSON -> array_elements/object_entries/object_search_key/CompareBinaryJSON`、`Search -> Walk/CreateBinaryJSON/like_match` 等调用边。部分跨 crate callers 查询为空，另以 `rg` 核验实际调用点。
- Go 对照：读取 `pkg/types/json_binary_functions.go` 的 Extract/构造/修改器、比较、Merge、Peek、Contains/Overlaps、Depth、Search/Walk 实现，以及 `pkg/types/json_binary_functions_test.go` 的全部用例。
- Rust 测试：读取 `pkg/types/json_binary_functions_test.rs` 和 `pkg/types/json_binary_functions_6_aster_unit_test.rs`；它们分别覆盖直接 Go 对照边界和主要公开操作组合。
- 上下游证据：源码检索核验 `pkg/expression/builtin_json.rs`、`pkg/expression/builtin_json_vec.rs`、`pkg/session/runtime/relational_value.rs`、`pkg/util/codec/codec.rs`、`pkg/executor/aggfuncs/func_max_min.rs` 等直接调用点。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的命令校验目标文档存在且恰有 11 个固定二级标题，并人工复核本文能回答文件存在原因、主要运行路径和安全扩展位置。
