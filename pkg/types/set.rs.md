# `pkg/types/set.rs`

## 文件定位

[`pkg/types/set.rs`](set.rs) 实现 MySQL `SET` 值的内存表示，以及“成员名 ↔ 位掩码”的解析。源码虽然位于 `pkg/types`，但直接编译边界是 `pkg/types/internal/file_group/lib.rs`：该入口用 `#[path = "../../set.rs"] pub mod set` 将文件纳入 `astersql-types-file-group` crate。随后 `pkg/types/internal/datum/lib.rs` 再导出 `ParseSet`、`ParseSetValue` 和 `Set`，根 `astersql-types` crate 又以 `datum` 等内部 crate 组成对外类型层。

它处在 SQL 类型系统的值转换与编解码基础层，而不是 SQL 语法层：字符串/数值转换为 SET 时会进入这里，存储中的 SET 位掩码被解码、校验或生成哈希键时也会进入这里。直接证据包括 `pkg/types/datum.rs::convertToMysqlSet`、`pkg/tablecodec/tablecodec.rs` 的 `mysql::TypeSet` 分支、`pkg/util/rowcodec/decoder.rs` 的 SET 解码分支和 `pkg/util/codec/codec.rs` 的 SET 哈希编码分支。

## 核心职责

- 用 `Set { Name, Value }` 同时保存 SET 的规范化文本和 `u64` 位掩码；第 `i` 个声明成员对应第 `i` 位。
- 用指定 collation 的排序键匹配成员名，而不是直接按 Rust 字符串相等比较；因此大小写、尾随空格等行为由 `collate::GetCollator` 返回的实现决定。
- 将输入成员去重并按 `elems` 的定义顺序重排，确保 `Name` 是稳定的规范形式。例如测试证明 `"a,b,a"` 和 `"b,a"` 均规范化为 `"a,b"`、数值为 `3`。
- 支持从无符号整数位掩码反解成员列表，并拒绝不能映射到 `elems` 的残留位。
- 为 Go `strconv.ParseUint(name, 0, 64)` 的输入约定提供本地兼容解析，使 `ParseSet` 在名称解析失败后可接受十进制及带前缀的二、八、十六进制数值。

## 主要符号

- `pub struct Set { pub Name: String, pub Value: u64 }`：SET 值载体。`Name` 是按声明顺序连接的规范成员名，`Value` 是成员位图。派生 `Clone`、`Default`、`Eq`、`PartialEq`；默认值也是空名、零掩码。
- `Set::String(&self) -> String` 与 `fmt::Display`：前者按 Go API 形态返回一份拥有所有权的名称副本，后者将同一名称写入 formatter。
- `Set::ToNumber(&self) -> f64`：把 `Value` 转为数值运算所需的 `f64`。大于 `2^53` 的掩码转换后可能不能被 `f64` 精确表示，这是整数到浮点转换本身的边界。
- `Set::Copy(&self) -> Set`：通过 `Clone` 深拷贝 `String`；`pkg/types/overflow_10_aster_unit_test.rs` 还检查副本的字符串存储与原值不是同一指针。
- `pub static zeroSet: LazyLock<Set>`：共享的空 SET 模板。返回空值时调用 `Copy`，不会把可变状态暴露给调用者。
- `pub struct SetError(String)`：本文件的解析错误，支持 `Display` 和 `std::error::Error`。其字段不公开，调用方通过标准错误接口读取消息。
- `ParseSet(elems, name, collation)`：综合入口；先调用 `ParseSetName`，失败后调用私有的 `parse_uint_base_zero`，数值有效时再调用 `ParseSetValue`。
- `ParseSetName(elems, name, collation)`：按 collation key 解析逗号分隔名称，构造规范名称和位图。
- `ParseSetValue(elems, number)`：按位图反解名称并验证没有未知位。
- `setIndexValue` / `setIndexInvertValue`：长度均为 64 的位掩码表，分别用于检测和清除某一位；由两个私有 `const fn` 在编译期生成。
- `init()`：Go 初始化函数的兼容形态。在 Rust 中掩码表已于编译期生成，所以该函数只建立引用，不承担运行时初始化。
- `format_elems` 与 `parse_uint_base_zero`：分别负责错误消息中的成员列表格式化，以及 Go base-0 风格的无符号整数解析。

## 执行流程

`ParseSet` 的流程如下：

1. 先把完整输入交给 `ParseSetName`。这保证当某个合法成员本身名为数字字符串时，名称语义优先于数值语义。
2. `ParseSetName` 对空字符串直接返回 `zeroSet.Copy()`；否则取得指定 collation 的 collator，以逗号拆分输入，并将每段的 `collator.Key` 收入 `HashSet<Vec<u8>>`。
3. 它按 `elems` 从左到右遍历；如果某成员的 collation key 存在于集合，就设置 `1_u64 << index`、把声明中的原始成员名加入输出，并从待匹配集合删除该 key。
4. 待匹配集合为空表示所有输入段均被识别，函数用逗号连接输出项。由 HashSet 去重、由 `elems` 驱动输出，因此重复输入不会重复占位，输入顺序也不会改变规范结果。
5. 若仍有 key 未被消费，名称解析失败。`ParseSet` 随后用 `parse_uint_base_zero` 尝试数值解析：可选前导 `+`，`0x`/`0X`、`0b`/`0B`、`0o`/`0O` 前缀，以及传统前导 `0` 八进制；运算使用 `checked_mul`/`checked_add` 拒绝 `u64` 溢出，并校验下划线位置。
6. 数值解析成功后，`ParseSetValue` 保存原始掩码，按 `elems` 顺序检测每一位。命中时追加成员并清除该位；遍历结束仍有残留位就报错，否则返回原始 `Value` 和规范 `Name`。

应用主链中的典型路径是：`Datum::convertToMysqlSet` 对字符串、字节、ENUM、SET 输入调用 `ParseSet`，对其他可数值化 Datum 调用 `ParseSetValue`；表数据和 row codec 解码则从已编码的无符号数值调用 `ParseSetValue`；表达式 `chunk_executor` 对 SET 结果逐行调用 `ParseSetName`，且在该调用点将失败结果替换为默认空 SET。

## 数据与状态

核心不变量是 `Value` 的第 `i` 位与 `elems[i]` 对应，成功解析后的 `Name` 由所有置位成员的声明名按 `elems` 顺序连接。`Set` 自身不保存 `elems` 或 collation，因此它不能脱离创建时的字段元数据自行验证 `Name` 与 `Value` 是否一致；调用者直接构造公开字段时也必须维护该不变量。

解析名称时的临时状态包括待匹配 collation key 集合 `marked`、规范输出 `items` 和累计位图 `value`；解析数值时以可变局部变量 `number` 记录尚未消费的位，以不可变 `value` 保留原始输入。所有状态都局限于一次调用。

MySQL SET 最多使用 64 位，本文件的静态掩码表也固定为 64 项。当前函数没有显式检查 `elems.len() <= 64`：`ParseSetName` 在索引达到 64 时执行移位，`ParseSetValue` 会索引长度 64 的表。因此字段元数据必须遵守最多 64 个成员的上游不变量；若未来接受不可信、未校验的 `elems`，应先在这里增加显式长度错误并补独立测试。

## 依赖与调用关系

下游依赖很窄：标准库提供 `HashSet`、格式化和 `LazyLock`；`astersql-util-collate`（在 `pkg/types/internal/file_group/Cargo.toml` 中以 `collate` 命名）提供 `GetCollator` 和 `Key`。`pkg/types/Cargo.toml` 则把 `types-file-group` 注册为路径依赖，并把它纳入整个 types 聚合层；本模块没有 feature 条件编译项。

对外暴露分两条主要路径：`pkg/types/internal/scalar/lib.rs` 再导出 `Set`，`pkg/types/internal/datum/lib.rs` 再导出 `ParseSet`、`ParseSetValue`、`Set`，并将 `SetError` 转换为 Datum 层 `errors::Error`。`ParseSetName` 没有经 Datum 门面再导出，但 `pkg/expression/lib.rs` 从 `types_dependency::file_group::set` 直接再导出它。

已核验的上游调用包括：

- `pkg/types/datum.rs::convertToMysqlSet`：SQL Datum 到 SET 的类型转换。
- `pkg/tablecodec/tablecodec.rs` 和 `pkg/util/rowcodec/decoder.rs`：从存储位掩码恢复 SET。
- `pkg/expression/chunk_executor.rs`：表达式列结果按字段 collation 解析 SET 名称。
- `pkg/util/codec/codec.rs`：根据有效位图重建规范名称，再按 collation 生成哈希编码。
- `pkg/tablecodec/tablecodec_test.rs`、`pkg/types/convert_test.rs` 等测试也通过聚合门面使用 `ParseSetValue`/`ParseSet`，说明导出链能到达更高层模块。

RustCodeGraph 能解析本文件内部边：`ParseSet -> ParseSetName`、`ParseSet -> parse_uint_base_zero`、`ParseSet -> ParseSetValue`，以及 `ParseSetName -> GetCollator/Key`。其本次 `callers` 查询没有报告跨 crate 再导出的调用边，所以上述上游关系由 RustCodeGraph 文件节点配合精确符号引用检索核验，不将空 callers 误解释为“未接线”。

## 错误处理与边界

`ParseSetName` 在至少一个输入段无法匹配声明成员时返回 `SetError("item ... is not in Set [...]")`。`ParseSet` 有意丢弃这次具体错误以尝试数值回退；若数值语法也无效，最终返回同类“item 不在集合”错误。由此，非法数字（如测试中的八进制 `"08"`）、负数、空数字主体、溢出值和下划线位置非法都会落到成员错误，而不会泄露整数解析细节。

`ParseSetValue` 对零返回空 SET。非零值中若存在 `elems` 未覆盖的位，就返回 `"invalid number <residual> for Set [...]"`；消息中的数字是清除已识别位后的残留值而非原始输入，这与同路径 Go 实现一致。上层对错误的处置不统一：Datum 转换和 table/row codec 通常传播或包装错误，而 `pkg/expression/chunk_executor.rs` 明确 `unwrap_or_default()`，把非法名称降级为空 SET；后者是调用点策略，不是本文件静默容错。

名称匹配以 collation key 为身份。重复段以及在当前 collation 下等价的段会被 HashSet 合并；输出采用 `elems` 中的拼写。空输入被专门解释为空 SET，而包含空段的非空输入（例如单独逗号或尾随逗号）只有在空字符串本身是已声明成员且 key 能匹配时才可能成功。

`ToNumber` 的 `u64 -> f64` 可能丢失低位精度；需要精确位语义的代码应继续读取 `Value`。另外，64 项上限目前依赖上游字段定义校验，越界 `elems` 不是可恢复的 `SetError` 路径。

## 并发与资源生命周期

解析函数没有锁、任务、通道、事务或 I/O；每次调用只分配局部 HashSet、Vec 和结果 String，返回后由 Rust 所有权自动回收。`Set` 也不包含引用或外部资源，可以安全 clone 和按值跨层传递；是否在线程间共享由调用者决定。

唯一的全局对象 `zeroSet` 使用 `std::sync::LazyLock`，首次访问的初始化由标准库保证线程安全。调用者得到的是 `zeroSet.Copy()` 而不是全局对象的可变引用。两个位掩码表是编译期生成的不可变静态数组，可被并发读取；兼容函数 `init()` 不改变任何状态，因此不存在 Go 版本运行时初始化的竞态窗口。

主要性能成本来自名称解析时为每个输入段及每个声明成员计算 collation key，以及构造 HashSet；复杂度约为输入段数与 `elems` 数量之和（另加 key 生成成本）。数值解析只线性扫描 `elems`。扩展时应避免在循环中重复创建 collator 或引入全局可变缓存，除非能明确处理 collation 身份、生命周期与并发一致性。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/types/set.go`，Rust 保留了 `Set`、`String`、`ToNumber`、`Copy`、`ParseSet`、`ParseSetName`、`ParseSetValue`、空 SET 和两张 64 位掩码表的整体结构。两端都先按名、后按 base-0 数字解析，都按 collator key 去重匹配，并以声明顺序规范化输出；`pkg/types/set_test.rs` 与 `pkg/types/set_test.go::TestSet` 的主要表驱动用例一致，覆盖名称顺序/重复、空值、数字零、大小写及尾随空格、位图反解和非法成员/未知位。

实现语言差异如下：

- Go 的 `zeroSet` 是普通包变量；Rust 使用 `LazyLock<Set>` 并在返回时 clone。
- Go 的 `Copy` 用 `stringutil.Copy` 强制复制字符串底层存储；Rust 的 `String::clone` 完成拥有所有权的深拷贝。
- Go 在 `init()` 中动态填充两个 `[]uint64`；Rust 以 `const fn` 生成固定 `[u64; 64]`，`init()` 仅保留 API/迁移形态。
- Go 用 `strconv.ParseUint(..., 0, 64)`；Rust 私有 `parse_uint_base_zero` 明确实现前缀、传统八进制、下划线和溢出规则。额外迁移测试 `pkg/types/overflow_10_aster_unit_test.rs` 验证 `0xf`、`017` 成功而 `08` 失败。
- Go 返回通用 `error`；Rust 返回本地 `SetError`，再由 `pkg/types/internal/datum/lib.rs` 转成 Datum 错误。错误文本保持 Go 风格，但类型身份并不跨该转换保留。

当前 Rust 测试比直接 Go `set_test.go` 多验证了 `utf8_general_ci` 行为和深拷贝指针；额外迁移测试又覆盖了 base-0 数值字面量。文档只据现有源码和测试陈述这些行为，没有把未运行 Cargo 测试等同于本次重新证明运行正确性。

## 扩展指南

- 修改名称解析或 collation 语义时，入口是 `ParseSetName`；必须保持“按声明顺序输出”“按 collation key 去重”和名称优先于数值回退，并同步 `pkg/types/set_test.rs`，同时对照 `pkg/types/set_test.go`。涉及新的 collation 边界时还应在 collate crate 的独立测试中验证 key 行为。
- 修改数字语法时，入口是 `parse_uint_base_zero` 和 `ParseSet` 的回退分支；需要覆盖正号、各进制前缀、传统八进制、下划线位置、负数、空主体和 `u64` 溢出，并与 Go `strconv.ParseUint` 行为逐项核对。
- 修改位图反解时，入口是 `ParseSetValue` 及两张掩码表；必须覆盖零、稀疏位、最高合法位、残留非法位和 64 成员边界。若要把 `elems.len() > 64` 从 panic 风险改为显式错误，应同时修改 `ParseSetName` 与 `ParseSetValue`，并在独立测试文件增加回归用例。
- 修改 `Set` 字段或不变量时，需要审计直接结构体构造者、Datum 的 `SetMysqlSet`/`GetMysqlSet`、chunk 列存储、serialization、tablecodec、rowcodec 和 codec 哈希路径；公开字段意味着仅改构造函数不足以覆盖所有生产者。
- 不应把测试内嵌回 `set.rs`。本仓库约定 Rust 测试独立放置，首要测试文件是 `pkg/types/set_test.rs`；迁移补充覆盖目前位于 `pkg/types/overflow_10_aster_unit_test.rs`。
- 兼容性风险主要是规范名称、错误文本、collation 等价关系和 numeric fallback 顺序；性能风险主要是增加 collation key 计算或额外分配。任何行为改动都应与 `pkg/types/set.go` 保持一致，除非明确记录为 Rust/Go 的受控差异。

## 验证依据

- 源码与模块边界：`pkg/types/set.rs`、`pkg/types/internal/file_group/lib.rs`、`pkg/types/internal/file_group/Cargo.toml`、`pkg/types/Cargo.toml`、`pkg/types/lib.rs`、`pkg/types/internal/datum/lib.rs`、`pkg/types/internal/scalar/lib.rs`、`pkg/expression/lib.rs`。
- Go 对照与测试：`pkg/types/set.go`、`pkg/types/set_test.go`、`pkg/types/set_test.rs`、`pkg/types/overflow_10_aster_unit_test.rs`。
- 生产调用证据：`pkg/types/datum.rs::convertToMysqlSet`、`pkg/tablecodec/tablecodec.rs` 的 SET 分支、`pkg/util/rowcodec/decoder.rs` 的 SET 分支、`pkg/expression/chunk_executor.rs` 的 SET 分支、`pkg/util/codec/codec.rs` 的 SET 分支。
- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`node --file pkg/types/set.rs` 核对了完整实现；`query ParseSet`/`ParseSetName`/`ParseSetValue` 定位 Rust 与 Go 对照符号；`callees` 核对 `ParseSet -> ParseSetName/parse_uint_base_zero/ParseSetValue`、`ParseSetName -> GetCollator/Key` 和 `ParseSetValue -> 掩码表/Set`。跨 crate `callers` 未返回边，已以 RustCodeGraph 上述调用文件节点和精确引用检索补证。
- 本任务按计划为纯文档分析，未运行 Cargo。交付前执行任务指定的结构命令，确认文件存在且恰有 11 个固定二级标题；并人工复核了文件定位、运行流程、安全扩展点及未验证限制。
