# `pkg/expression/builtin_json.rs`

## 文件定位

`builtin_json.rs` 是 `astersql-expression` crate 内的 JSON 标量语义内核，对照 Go 的 `pkg/expression/builtin_json.go` 实现 JSON 值上的类型查询、路径提取与修改、构造、合并、包含关系、元数据查询、schema 验证和 CRC32 汇总。它操作 `types-json-functions` 提供的 TiDB `BinaryJSON`，不是纯文本 JSON 工具集。

crate 根 `pkg/expression/lib.rs:226-230` 用 `#[path = "builtin_json.rs"]` 将文件装入私有模块 `builtin_json_kernel`，再通过公开模块 `builtin_json` 重导出全部公开符号。`pkg/expression/lib.rs:767-769` 还为 crate 内测试提供 `expression_builtin_json` 别名。`pkg/expression/Cargo.toml` 定义包名 `astersql-expression`、库入口 `lib.rs`，并声明本文件直接用到的 `crc32fast`、`serde_json`、`jsonschema` 和 path 依赖 `types-json-functions`。

这个文件不定义 SQL 函数类、`Expression` 参数求值、返回字段类型、PB signature 或向量化行调度。源码头注释也明确说明“表达式构造与行级调度由上层完成”。精确 Rust 引用搜索显示，本文件的标量函数当前主要由独立测试直接调用；不应仅凭 `lib.rs` 的公开重导出宣称其已接入完整 SQL 求值主链。

## 核心职责

1. 把 JSON path 文本统一解析为 `JSONPathExpression`，并使路径错误通过 `JsonBuiltinError` 传播（`parsed_paths`）。
2. 维持 SQL NULL 与 JSON null 的区分：`Option<BinaryJSON>::None` 表示 SQL NULL，构造或修改 JSON 值时由 `argument_value` 显式转成 JSON `null`；而部分函数使用返回值的 `Option` 表达 SQL NULL。
3. 封装 `BinaryJSON` 的路径操作：`json_extract`、`json_set`、`json_insert`、`json_replace`、`json_remove`、`json_array_append` 和 `json_array_insert`。
4. 实现 JSON 组合与关系语义：数组/对象构造、preserve/patch 合并、`MEMBER OF`、`JSON_CONTAINS`、`JSON_OVERLAPS` 和 `JSON_CONTAINS_PATH`。
5. 实现表示与元数据函数：`json_quote`、`json_unquote`、`json_pretty`、`json_type`、`json_valid_string`、`json_storage_free`、`json_storage_size`、`json_depth`、`json_keys` 和 `json_length`。
6. 用 `jsonschema` 验证 JSON document，用 `crc32fast` 实现 JSON 数组元素类型转换后的 IEEE CRC32 累加。
7. 重导出 `types_json_functions` 中的 JSON 核心类型、常量与操作函数，使调用者可以从 `builtin_json` 模块获取统一 API。

## 主要符号

- `JsonBuiltinError(String)` / `JsonBuiltinResult<T>`：本文件的轻量错误边界。它实现 `Display` 和 `std::error::Error`，并从 `types_json_functions::JsonBinaryError` 与 `serde_json::Error` 保留文字转换。内部字符串不对外暴露字段。
- `json_null` / `argument_value`：创建 JSON null，并在 JSON 构造/修改边界把 SQL NULL 值参数转换为 JSON null。
- `parsed_paths`：批量调用 `ParseJSONPathExpr`；任一路径解析失败就整体返回错误。
- `json_type`、`json_extract`、`json_quote`、`json_unquote`：分别对应 JSON_TYPE/EXTRACT/QUOTE/UNQUOTE。`json_extract` 用 `Option` 表示未命中；`json_unquote` 额外检测引号文档根后还有值的 Go 错误分支。
- `ModifyMode` / `json_modify` / `json_set` / `json_insert` / `json_replace`：共享修改主链。三个公开函数只选择 `JSONModifySet/Insert/Replace` 模式，具体路径和值整理在 `json_modify` 中完成。
- `json_merge_preserve` / `json_merge` / `json_merge_patch`：`json_merge` 是 preserve 语义的别名；patch 接受 `Option<&BinaryJSON>` 以保留 SQL NULL 的行语义。
- `json_object` / `json_array`：通过 serde `Map`/`Value` 构造后转回 `BinaryJSON`。对象键为 SQL NULL 时报错，重复键由后值覆盖前值。
- `normalized_one_or_all`、`json_contains_path`、`json_search`：对 `one`/`all` 大小写不敏感并拒绝其他模式。`json_search` 还处理默认 escape 字节和可选路径。
- `json_member_of`、`json_contains`、`json_overlaps`：使用 `CompareBinaryJSON`、`ContainsBinaryJSON` 和 `OverlapsBinaryJSON` 保留 TiDB JSON 比较规则，避免用 serde 的普通结构相等取代二进制 JSON 语义。
- `json_array_append` / `json_array_insert`：按参数对依次更新局部 `result`，后一对会观察到前一对的修改。
- `json_storage_free`、`json_storage_size`、`json_depth`、`json_keys`、`json_length`：读取存储和结构元数据。`json_storage_free` 因 TiDB 不支持 JSON 局部更新而恒返回 0。
- `json_schema_valid`：先把 schema 转成 serde 值并强制它是 object，然后每次构建 validator 并验证 document。
- `JsonSumType`、`display_sum_item`、`json_sum_crc32`：把数组元素转成 Signed/Unsigned/Double/String/Boolean 的 Go `%v` 风格文本，对每个文本做 IEEE CRC32，最后累加为 `i64`。
- `sorted_object_entries`：借助 `BTreeMap` 返回按键排序的对象条目；非 object 输入报错。

## 执行流程

路径提取流程是：调用者传入已是 `BinaryJSON` 的 document 和路径文本；`json_extract` 先通过 `parsed_paths` 全部解析，再一次调用 `BinaryJSON::Extract`。命中时返回 `Some(BinaryJSON)`，一条都未命中时返回 `Ok(None)`，路径无效则返回 `Err`。多路径或可多选路径的组合结果由底层 `Extract` 保持 Go 行为。

SET/INSERT/REPLACE 共享一条有意的求值顺序：

1. `json_modify` 先从所有 `(path, value)` 对收集路径文本。
2. `parsed_paths` 在处理任何值之前解析所有路径，因此坏路径的错误优先于值转换错误，对齐 Go `jsonModify`。
3. `argument_value` 将每个 SQL NULL 值转为 JSON null。
4. `ModifyMode` 被映射到底层修改常量，最后一次调用 `BinaryJSON::Modify`。

`json_array_append` 逐对处理：拒绝可多选路径，忽略未命中路径，将被选中的标量先包成数组，再把新值当作一个整体元素 append，然后用 SET 模式写回。`json_array_insert` 同样顺序应用参数对，但把“父路径不存在或不是数组”的具体不变语义交给 `BinaryJSON::ArrayInsert`。

`json_contains`、`json_keys` 和 `json_length` 的可选路径流程相似：解析路径，拒绝 `*`/范围等可多选路径，再提取单值。路径缺失返回 SQL NULL；`json_keys` 选中非 object 也返回 SQL NULL；`json_length` 对标量返回 1。

`json_sum_crc32` 要求输入是 JSON array，把每个 serde 元素交给 `display_sum_item`。任一元素不能转成目标类型就终止；成功文本的 CRC32 转为 `i64` 后累加。

## 数据与状态

- 核心数据是 `BinaryJSON { TypeCode, Value }`；本文件通过 `BinaryJSONToSerde`/`CreateBinaryJSON` 在需要 serde 构造、pretty print 或 schema 验证时往返转换。
- SQL NULL 不与 `Value::Null` 混用。参数中的 `Option<BinaryJSON>` 或 `Option<&BinaryJSON>` 是 SQL NULL 通道；构造器内再按 Go 规则转为 JSON null。查询类 API 的 `Result<Option<T>, _>` 用内层 `None` 表示 SQL NULL/路径缺失。
- 修改类函数不就地改写调用者的 document。`json_modify` 返回新 `BinaryJSON`；append/insert 函数先 clone 为局部 `result`，再顺序替换。
- `json_object` 的 serde `Map` 保留单键单值，后插入的重复键覆盖前值。`sorted_object_entries` 另用 `BTreeMap` 为需要 TiDB 二进制 JSON 键顺序的调用者生成确定性序列。
- 本文件没有全局可变状态、会话状态或持久化状态。`json_schema_valid` 的 validator 是每次调用的局部值，并未实现 Go `builtinJSONSchemaValidSig.schemaCache` 的按求值上下文缓存。
- `JsonSumType` 是调用者已选定的元素目标类型，不携带 Go `FieldType` 的 charset、flen 或 array metadata。

## 依赖与调用关系

- 上游装配：`pkg/expression/lib.rs` 将本文件装为 `builtin_json_kernel`，并通过 `pub mod builtin_json` 重导出。`lib.rs` 还在 `#[cfg(test)]` 下装入 `builtin_json_test.rs`、`builtin_json_14_aster_unit_test.rs`、`builtin_json_vec_test.rs` 和 `builtin_json_vec_13_aster_unit_test.rs`。
- 当前直接调用者：RustCodeGraph 能从 `test_json_extract` 定位到 `json_extract`；精确文本引用复核显示 `json_type`、`json_extract`、`json_set`、`json_merge_patch`、`json_schema_valid`、`json_sum_crc32` 和 `sorted_object_entries` 的直接 Rust 调用均在独立测试中；`typeinfer_test.rs` 使用的是 SQL 函数文本，不是这些 Rust 函数的静态调用。
- 文件内调用边：RustCodeGraph 确认 `json_set/json_insert/json_replace -> json_modify -> parsed_paths + argument_value`，`json_merge -> json_merge_preserve`，`json_contains_path -> normalized_one_or_all`，`json_search -> normalized_one_or_all + parsed_paths`，`json_sum_crc32 -> display_sum_item`。
- JSON 下游：`types_json_functions` 提供 path parser、`Extract`、`Modify`、`Remove`、`ArrayInsert`、`Search`、merge/contains/overlaps、二进制 JSON 比较和 serde 转换。本文件将这些底层错误收敛为 `JsonBuiltinError`。
- 第三方下游：`serde_json` 负责 quote/unquote 校验、pretty print 和 JSON Schema 的 serde 输入；`jsonschema::validator_for` 编译 schema；`crc32fast::hash` 实现 IEEE CRC32；`BTreeMap` 保证 object key 排序。
- Go 对照：`pkg/expression/builtin_json.go` 是语义与错误顺序的主要对照文件；`pkg/expression/Cargo.toml` 的 `[package.metadata.porting].go-package = "pkg/expression"` 也明确了迁移边界。

RustCodeGraph 对 `json_extract` 这类常见名称的 `callers/callees` 会合并多个同名候选；因此文档只采信其明确指向 `pkg/expression/builtin_json.rs:<line>` 的边，并用 `lib.rs` 与精确 Rust 引用搜索复核上游。

## 错误处理与边界

- 语法无效的 JSON path 由 `ParseJSONPathExpr` 报错。`json_contains`、`json_array_append`、`json_array_insert`、`json_keys` 和 `json_length` 还明确拒绝可以命中多值的 `*`/范围路径，错误文本为 `JSON path may not contain * or a range`。`json_extract`、`json_remove`、`json_search` 则把它们各自支持的路径语义交给底层。
- `json_modify` 先解析所有 path 再处理 value，这是错误优先级契约，不是可随意重排的实现细节。
- `json_object` 拒绝 SQL NULL 成员名；NULL 值则合法并转为 JSON null。`json_array` 也把 SQL NULL 参数编码成 JSON null。
- `json_contains`、`json_keys`、`json_length`、`json_extract` 的路径缺失是正常 SQL NULL 结果，不是错误。调用者必须保留 `Result` 与 `Option` 的两层区分。
- `json_search` 只接受空/缺省 escape（解释为 `\`）或恰好一字节的 escape。它以字节长度判断，多字节 UTF-8 字符不等于“一字节”。
- `json_unquote` 对头尾是引号但整体不是单一合法 JSON 值的文本，返回 Go 对应的“document root 后有其他值”错误。
- `json_schema_valid` 要求 schema 是 object；无效 schema 编译错误和 serde/二进制转换错误继续向上传播。返回 `false` 表示文档不符合合法 schema，不表示 schema 本身无效。
- `json_sum_crc32` 拒绝非 array 输入和不能转为指定 `JsonSumType` 的元素。目标类型的构建期禁止规则（如 YEAR/JSON/FLOAT/DECIMAL array、charset 和 flen）位于 Go 函数类层，本 Rust API 本身没有足够的 `FieldType` 信息重现这些构建检查。
- CRC32 和存储长度的累加使用普通 Rust 整数加法，没有显式 wrapping/saturating 策略；扩展到极端数组大小时应保持与 Go `int64` 行为和项目 overflow 策略一致。

## 并发与资源生命周期

本文件中所有标量语义函数都是同步函数；没有线程、async task、channel、锁、事务或 I/O 资源。输入通过共享引用读取，修改类 API 返回拥有所有权的新 `BinaryJSON`，因此不在调用者可见对象上持有跨调用可变借用。

临时资源都限于单次调用：路径向量、serde `Value`、`Map`、append/insert 的中间 document、JSON Schema validator 和 CRC32 文本在返回后释放。`json_schema_valid` 每次重新编译 schema，因此无锁也无共享 cache，但重复 schema 的性能特性与 Go 带 `builtinFuncCache` 的签名不同。

Go 的 `builtinJSON*Sig` 注释要求新字段必须可线程安全或不可变，因为表达式可以跨会话共享。那是上层签名对象的生命周期契约，不是本文件中已有的状态。若未来在此处加入 schema cache 或其他可变缓存，必须同时定义 clone、上下文隔离和跨会话共享规则。

## 与 Go 版本的对应关系

- Rust 的 `json_type`、`json_extract`、`json_unquote`、SET/INSERT/REPLACE/REMOVE、merge/object/array、contains/overlaps、append/insert/search、storage/depth/keys/length 和 schema/CRC32 函数，对应 Go `builtinJSON*Sig.evalString/evalJSON/evalInt` 中的核心值语义。
- Go 实现通常有两层：`json*FunctionClass.getFunction/verifyArgs` 负责实参数、隐式 CAST、返回 `FieldType`、charset/collation 和 PB code；`builtinJSON*Sig.eval*` 负责逐行求值。本 Rust 文件仅对应后者的语义核心，接收的已是类型化值，不得把缺少的函数类检查误认为无需要。
- Rust `json_modify` 显式保留 Go `jsonModify` 的“先求值/解析所有路径，后求修改值”顺序；`argument_value` 保留 SQL NULL value 变 JSON null 的语义。
- Rust `json_merge` 与 `json_merge_preserve` 的返回值一致，但 Go 的 deprecated `JSON_MERGE` 还会通过求值上下文追加弃用 warning。本文件没有 `EvalContext`，注释明确把告警交给上层。
- Rust `json_quote` 使用 `serde_json::to_string`，与 Go encoder 的 `SetEscapeHTML(false)` 目标一致，不转义 `<`、`>` 和 `&`。`json_unquote` 保留 Go 对根后附加值的专用错误。
- Rust `json_storage_size` 与 Go 都是 `Value` payload 长度加 1 字节 TypeCode；`json_storage_free` 与 Go 一样在有效非 NULL 文档上返回 0，但 SQL NULL/无效文本的评估仍由 Rust 上层签名负责。
- Rust `json_schema_valid` 保留 schema 必须是 object 和验证 true/false 的核心行为，但 Go `builtinJSONSchemaValidSig` 对常量 schema 使用 `builtinFuncCache[jsonschema.Schema]`，并经 `context.Background()` 执行 qri validator；Rust 当前每次使用 `jsonschema::validator_for`，没有缓存。`builtin_json_test.rs::test_json_schema_valid_cache` 只验证重复调用结果，不能证明 Rust 存在 cache。
- Rust `JsonSumType` 覆盖 Go 实际求值时的 int/uint/real/string/bool 转换族。Go 还在 `jsonSumCRC32FunctionClass.getFunction` 利用 array `FieldType` 拒绝 YEAR、JSON、FLOAT、DECIMAL、不支持 charset 和未指定长度的 BLOB；这些构建期分支未由此文件的简化 enum 表达。

## 扩展指南

1. 新增 JSON 标量语义时，先确定它是本文件的“已类型化纯语义”职责，还是上层函数类/行调度职责。参数个数、SQL 隐式 CAST、charset/collation、PB code、SQL warning 和 SQL NULL 短路不应偷偷塞进一个只接收 `BinaryJSON` 的 helper。
2. 路径类函数应复用 `parsed_paths` 或 `ParseJSONPathExpr`，并按 Go 对照明确决定是否允许 `CouldMatchMultipleValues()`。不要在不支持多选的 API 中静默取第一个命中。
3. 涉及 JSON value 参数时，显式设计 SQL NULL 与 JSON null 的类型表达；设计 `Result<Option<T>, E>` 时记录 `None` 的 SQL 含义，避免调用者把路径缺失当成错误。
4. 修改评估顺序前先对照 Go 的 `eval*` 和测试。特别是 `json_modify` 的路径优先报错、append/insert 多对顺序可见性、missing path 的忽略/NULL/不变差异，都是外部行为。
5. 扩展 `JsonSumType` 时要同时复核 Go `convertJSON2Tp`、`FieldType.ArrayType()` 的构建限制、`fmt %v` 文本和 CRC32 累加行为；不能只让某个 serde 转换通过。
6. 若为 `json_schema_valid` 增加 cache，需要先定义 cache key（schema 值还是求值上下文）、构建失败是否缓存、clone 与跨会话共享语义，并添加真正观测初始化次数的并发/上下文测试；仅重复调用结果正确不是 cache 证据。
7. 测试必须保持在独立文件。直接标量语义优先同步 `pkg/expression/builtin_json_test.rs`；Go 分支覆盖/迁移 parity 可同步 `pkg/expression/builtin_json_14_aster_unit_test.rs`；涉及批量行或向量化调度时同步 `pkg/expression/builtin_json_vec_test.rs` 和 `pkg/expression/builtin_json_vec_13_aster_unit_test.rs`，并检查 `builtin_json_vec.rs` 的独立实现。
8. 扩展后要分别验证正常值、SQL NULL、JSON null、路径缺失、多选路径、非法模式/escape、类型转换失败、多对顺序和标量/向量一致性。

## 验证依据

- 计划与依赖：已读取只读总计划 `.plans/2026-10-07-rust-全架构逐文件解析/plan.md` 与任务 1171；任务标注“批次 1，无依赖”，因此开始条件已满足。
- RustCodeGraph 状态：`rustcodegraph status` 显示索引含 11,467 个文件、307,296 个节点和 1,848,419 条边。`files --filter builtin_json.rs` 未直接列出文件，但 `node --file pkg/expression/builtin_json.rs --offset ...` 能完整读取 589 行并报告引用概览；`query json_extract/json_modify/json_sum_crc32 --kind function` 精确定位到本文件符号。
- RustCodeGraph 调用边：`callees` 输出确认 `json_extract -> parsed_paths`、`json_set/json_insert/json_replace -> json_modify`、`json_modify -> parsed_paths + argument_value`、`json_merge -> json_merge_preserve`、`json_contains_path -> normalized_one_or_all`、`json_search -> normalized_one_or_all + parsed_paths`、`json_sum_crc32 -> display_sum_item`；对重名候选的噪声已用精确文件引用搜索排除。
- 已读 Rust 源码：`pkg/expression/builtin_json.rs` 全部 589 行，以及 `pkg/expression/lib.rs` 的模块装配/测试装配片段。目标包没有 `pkg/expression/doc.go`。
- 已读 crate 声明：`pkg/expression/Cargo.toml`，核对了库入口、`autotests = false`、`doctest = false`、`types-json-functions`、`crc32fast`、`serde_json`、`jsonschema` 及 porting metadata。
- 已读 Go 对照：`pkg/expression/builtin_json.go` 中 JSON_TYPE/SUM_CRC32/EXTRACT、修改、构造/合并、contains/overlaps、append/insert/search、storage/depth/keys/length 和 schema validation 的函数类与 `eval*` 实现，特别复核了弃用告警、构建期类型限制与 schema cache 差异。
- 已读独立 Rust 测试 `pkg/expression/builtin_json_test.rs` 全部 465 行，并检查 `builtin_json_14_aster_unit_test.rs` 与 `builtin_json_vec_13_aster_unit_test.rs` 的测试名/覆盖面；证据包括 SQL NULL 转 JSON null、重复键覆盖、missing path、多选路径拒绝、append 数组嵌套、CRC32 类型错误和 schema object 限制。
- 本任务是纯文档分析，按计划不运行 Cargo。交付验证为固定十一章结构检查及人工事实复核，不以编译或零测试替代源码证据。
