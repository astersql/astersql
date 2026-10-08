# `pkg/util/regionsplit/split_handle.rs`

## 文件定位

`split_handle.rs` 是 `astersql-util-regionsplit` crate 中的 Region 切分键核心实现。crate 入口 `pkg/util/regionsplit/lib.rs` 将本文件的公开项全部再导出，同时再导出 `model_handle.rs` 中面向真实 `model::TableInfo` 与 `types::Datum` 的适配 API。Cargo 边界由 `pkg/util/regionsplit/Cargo.toml` 定义；本文件自身只使用 `std`，而真实模型适配层使用清单中的 `astersql-kv`、`astersql-meta-model`、`astersql-tablecodec`、`astersql-types` 和 `astersql-util-codec` 等依赖。

本文件不是 TiKV RPC 或 Region 调度器：它只把表/索引元信息、上下界和目标段数转换成有序键。生产 DDL 的策略路径在 `pkg/ddl/split_region.rs` 中调用 `GetSplitTableKeysForModel` / `GetSplitIndexKeysForModel`，由同 crate 的 `model_handle.rs` 复用本文件的 `MinRegionStepValue`、`encode_i64`、`gen_table_record_prefix` 和 `get_values_list`。本文件公开的简化 `TableInfo`、`Datum` 与 `GetSplit*Keys` 主要承载核心算法及编码兼容性测试，不等同于完整 SQL 类型系统。

## 核心职责

1. 用 `SplitHandleCols` 抽象整数 handle 与 common handle 的构造差异，并由 `BuildHandleColsForSplit` 根据 `TableInfo::is_common_handle` 选择策略。
2. `GetSplitTableKeys` 为记录键空间生成边界：必要时先加入记录前缀，再对整数 handle 做等差切分，或对 common handle 编码后的字节范围做插值。
3. `GetSplitIdxPhysicalStartAndOtherIdxKeys` 和 `GetSplitIndexKeys` 为索引键空间补齐物理起止边界，并在上下界之间生成中间键。
4. 提供与 TiDB/TiKV 键序一致的局部编码原语：`encode_i64`、表/索引前缀、datum 类型标签、字节串 memcomparable 编码及 `get_values_list` 插值。
5. 以 `SplitError` 统一报告非法范围、空输入、无效拆分数及 handle/datum 构造失败；以原子变量 `MinRegionStepValue` 保持整数切分的可调最小步长。

## 主要符号

- `MinRegionStepValue: AtomicI64`：初值为 `1000`，`calculate_int_bound_value` 和 `model_handle.rs::GetSplitTableKeysForModel` 都以 Acquire 顺序读取它。它是进程级共享策略值，不属于某次语句上下文。
- `SplitError::{InvalidRanges, InvalidDatum}`：两种错误均保存可展示字符串，并实现 `Display` 与 `std::error::Error`。前者表示范围/数量/步长问题，后者表示输入不足或编码上下文问题。
- `Datum`：简化的 `Null`、`Int`、`UInt`、`Bytes`、`String` 值集合。`as_i64` / `as_u64` 对非数值返回零；`Display` 用于错误消息，不执行 SQL 类型转换。
- `StatementContext`：当前仅保存 `time_zone` 字符串；简化 API 的 handle 构造暂不消费它，真实适配层会解析并用于 key 编码。
- `IndexInfo`、`TableInfo`：只保留切分算法所需的 id、名称、handle 标志和索引列表。它们与 `astersql_meta_model` 中的完整结构是不同类型。
- `Handle::{Int, Common}` 与 `Handle::encoded`：整数采用无 datum 标签的 8 字节可比较编码；common handle 直接持有已编码字节。
- `SplitHandleCols`：公开策略 trait，要求实现 `BuildHandleByDatums` 与 `IsInt`。`intHandleCols` 取第一列构造整数 handle；`commonHandleCols` 用 `encode_datums` 编码整行。
- `calculate_int_bound_value`：私有整数范围校验和步长计算函数，分别处理无符号主键与其他整数 handle。
- `GetSplitTableKeys`、`GetSplitIndexKeys`：公开的简化表/索引切分入口；返回既有 `keys` 加新边界组成的列表。
- `GetSplitIdxPhysicalStartAndOtherIdxKeys`：索引物理边界辅助函数。目标索引不是首索引时加入自身前缀，并总是加入 `index.id + 1` 的前缀作为结束边界。
- `encode_i64`、`gen_table_record_prefix`、`get_values_list`：`pub(crate)` 原语，供 `model_handle.rs` 复用；其余编码函数保持文件私有。

## 执行流程

表记录切分从 `GetSplitTableKeys` 开始。它先用 `physical_id` 生成 `t + comparable(table_id) + _r` 前缀。如果表含索引，并且不是“common handle 且只有一个索引”的情形，就先把记录前缀加入输出，使记录与索引键空间形成独立边界。

整数路径由 `handle_columns.IsInt()` 选择。`calculate_int_bound_value` 先拒绝零段数或空上下界，再检查 `upper > lower`。无符号主键使用 `u64` 差值；其他情况将 wrapping 的有符号差值转成 `u64` 后除以段数。计算出的步长必须不小于当前 `MinRegionStepValue`。随后循环 `1..number`，每次以 wrapping 加法推进 record id，追加一条 `record_prefix + IntHandle` 键，因此产生 `number - 1` 个内部切分点。

common handle 路径分别调用 `BuildHandleByDatums` 编码上下界，以 `Handle::encoded()` 的字节序验证下界严格小于上界，再组成完整记录键并交给 `get_values_list`。后者保留两端的最长公共前缀，将余下最多八个字节左对齐补齐成 `u64`，按 `(upper-lower)/number` 计算步长，生成 `number - 1` 个中间值；上下界本身不加入结果。

索引路径 `GetSplitIndexKeys` 先拒绝零段数，再由 `GetSplitIdxPhysicalStartAndOtherIdxKeys` 加入索引空间边界。上下界通过 `encode_index_key` 编成“表索引前缀 + datum 序列 + `INT_HANDLE_FLAG` + `i64::MIN` handle”；固定使用最小 handle 避免 handle 后缀扰动索引值区间。确认编码后下界小于上界后，再调用同一 `get_values_list` 插值。

## 数据与状态

所有生成的键都是拥有所有权的 `Vec<u8>`，输出是 `Vec<Vec<u8>>`。三个公开生成函数都接收已有 `keys`，在其后追加结果；调用方必须意识到错误可能发生在追加之后：例如 `GetSplitTableKeys` 可先加入记录前缀，再因非法整数范围返回错误。Rust 的 `Result` 不携带发生错误时已修改的局部向量，因此错误分支不会把部分结果返回给调用者。

键序不依赖本机字节序：`encode_i64` 先翻转符号位，再写大端字节，使有符号整数的字典序与数值序一致；`UInt` 直接写大端；字符串和 bytes 使用 8 字节分组、零填充与 marker 的 memcomparable 格式。`encode_memcomparable_bytes` 的 `offset <= len` 会为长度恰为 8 的倍数（包括空串）额外写终止组，这是排序编码的一部分。

唯一的可变全局状态是 `MinRegionStepValue`。其类型允许其他模块或测试通过原子 store 调整阈值；当前文件只 load，不恢复或管理调用方写入的值。`StatementContext`、`TableInfo`、`IndexInfo`、输入 datum 和 handle 策略都通过共享引用读取；生成过程不修改输入。

## 依赖与调用关系

RustCodeGraph 对本文件的文件关系显示直接使用者包括 `pkg/util/regionsplit/model_handle.rs` 与 `pkg/util/regionsplit/tests/split_handle_test.rs`；后者直接验证简化 API，前者把编码原语接入完整模型。`pkg/util/regionsplit/lib.rs` 通过 `pub use split_handle::*` 暴露公开接口。

生产上游位于 `pkg/ddl/split_region.rs`：表策略调用 `GetSplitTableKeysForModel`，索引策略调用 `GetSplitIndexKeysForModel`；适配层内部再使用本文件的共享阈值和插值/前缀原语。生成出的键最终交给 DDL 的 split 闭包/存储层执行 Region 拆分。因此本文件位于“DDL 解析并归一化策略”之后、“向存储发起 split”之前。

下游全部是本地确定性计算：标准库格式化与原子操作，以及本文件内的键编码函数。虽然 `Cargo.toml` 声明多个 workspace crate，`split_handle.rs` 不直接 import 它们；这些依赖服务于同 crate 的 `model_handle.rs`。这一区分对扩展很重要：SQL 类型、collation、索引前缀截断和真实时区语义应进入模型适配层，而不是扩张此处的简化 `Datum` 假装覆盖完整语义。

## 错误处理与边界

- 表整数切分要求 `number > 0`、上下界非空、`upper > lower` 且整数步长至少为原子阈值；失败返回 `InvalidRanges`。
- `Datum::as_i64` / `as_u64` 对非数值静默得到零，这是对简化 Go 取值习惯的局部模拟，而非类型安全转换。生产调用方应在 `model_handle.rs` 入口之前完成类型归一化。
- `intHandleCols::BuildHandleByDatums` 对空行返回 `InvalidDatum`；`commonHandleCols` 允许空行并会得到空编码。是否允许空 common handle 范围最终仍受上下界排序及调用层约束。
- 表 common handle 与索引路径都要求编码后的下界严格小于上界；错误消息包含表/索引名和格式化 datum。`Bytes` 的显示使用 Rust debug 格式，与 Go 的 `Datum.ToString` 文本不保证完全一致。
- `get_values_list` 单独防御零段数，但不再次检查 `lower < upper`；公开入口在调用前负责顺序校验。它只对公共前缀之后的前八字节插值，且采用 wrapping 减法/加法，调用者不能把它当作任意精度字节整数算法。
- `GetSplitIdxPhysicalStartAndOtherIdxKeys` 用 `index.id + 1` 计算结束前缀；当前实现没有对 `i64::MAX` 做显式溢出处理。正常元数据 id 应满足该隐含前置条件。
- `calculate_int_bound_value` 的有符号极值差使用 wrapping 语义，以匹配 Go 转换路径；这不是普通数学上的有符号减法。

## 并发与资源生命周期

文件不创建线程、任务、通道、锁、事务、网络连接或文件句柄。每次调用只分配并返回字节向量，临时 handle 与编码缓冲区在函数返回时释放；没有异步取消或外部资源清理要求。

并发共享点只有 `MinRegionStepValue`。Acquire load 保证读取与相应原子发布操作的顺序关系，但一次 `GetSplitTableKeys` 只读取阈值一次，后续循环不受并发修改影响。调用方若在测试或运行期修改阈值，需要自行约束跨测试污染和策略一致性。本文件其他类型由不可变引用读取，因而算法本身没有内部数据竞争。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/regionsplit/split_handle.go`。两边都有 `MinRegionStepValue`、`SplitHandleCols`、整数范围计算、表/索引切分入口、索引物理边界函数、整数/common handle 策略和 `BuildHandleColsForSplit`。关键相同行为包括：整数路径生成 `num-1` 个内部点；common handle 单索引表不额外插入记录前缀；非首索引加入自身前缀且所有索引加入下一 id 前缀；索引上下界都使用最小整数 handle；非法顺序与过小步长返回错误。

Rust 文件刻意是简化核心，不是 Go API 的类型级一比一替换。Go 版本使用 `model.TableInfo`、`types.Datum`、`stmtctx.StatementContext`、`tablecodec`、`codec` 和 `kv.Handle`，并由 `terror.Error` 生成 SQL 错误；Rust 的完整模型接线在 `model_handle.rs`。尤其是 Go `commonHandleCols` 会复制输入并按主索引前缀长度调用 `TruncateIndexValues`，而本文件的 `commonHandleCols` 仅编码整行；Rust 对应的前缀截断在 `ModelHandleCols::BuildHandleByDatumsAt` 中实现。Go 的 `GetSplitIndexKeys` 使用真实 `tables.NewIndex(...).GenIndexKey`，Rust 简化入口使用局部编码器，而生产适配入口 `GetSplitIndexKeysForModel` 使用 `astersql_tablecodec::GenIndexKey`。

`pkg/util/regionsplit/tests/split_handle_test.rs` 验证简化实现与 Go 键布局兼容：整数记录键、复合 handle 的 memcomparable 编码、索引值与 handle 标志位置。`pkg/util/regionsplit/tests/go_merge_32_test.rs` 验证真实模型路径的主键列选择、common handle 前缀截断且不修改输入，以及转换错误归一化。Go 的更高层回归位于 `pkg/ddl/split_region_test.go`，覆盖策略边界解析及 `regionsplit.GetSplitIndexKeys` 等接线；这些测试是语义参照，但本纯文档任务未运行测试。

## 扩展指南

新增切分算法时，先判断它属于无 SQL 类型依赖的键空间算法还是完整模型语义。前者可在本文件扩展 `get_values_list` 或新增私有编码原语；后者（collation、时区、SQL 类型转换、索引前缀长度、真实错误类型）应优先接入 `model_handle.rs`，避免让两套 `Datum`/`TableInfo` 语义继续分叉。

增加新 handle 类别时，需要同步审查 `Handle::encoded`、`SplitHandleCols`、`BuildHandleColsForSplit`、表上下界比较与记录键编码；不能只让 `IsInt` 返回 false 就假设所有非整数 handle 都能共用 common handle 字节序。增加 datum 类型时必须定义稳定的类型标签、值编码和跨类型排序，并与 Go `codec.EncodeKey` 输出逐字节对照。

修改插值或最小步长规则时，需要同时审查 `model_handle.rs` 的生产适配实现，因为它直接复用了部分原语、又复制了整数范围逻辑。修改索引键布局时应同步真实 `tablecodec::GenIndexKey` 路径。测试逻辑必须继续放在独立文件：核心编码回归扩展 `pkg/util/regionsplit/tests/split_handle_test.rs`，完整模型/前缀主键回归扩展 `pkg/util/regionsplit/tests/go_merge_32_test.rs`，DDL 可见行为则扩展 `pkg/ddl/split_region_test.go` 或对应独立 Rust DDL 测试，不能内嵌到本源文件。

兼容性风险主要是键字节变化会把切分点放到错误的 Region；正确性风险包括上下界排序、无符号/极值溢出、零步长和索引结束前缀；性能风险集中在每个 datum 和每个切分点的新向量分配。任何优化都必须保持输出字典序及 Go 字节兼容，而不能只比较结果数量。

## 验证依据

- 源码全貌：`pkg/util/regionsplit/split_handle.rs`，核对了所有公开/私有类型、trait、函数、常量与 impl；文件无条件编译项。
- crate 边界：`pkg/util/regionsplit/Cargo.toml` 与 `pkg/util/regionsplit/lib.rs`，确认 crate 名、模块声明、再导出和 workspace 依赖。
- 生产接线：`pkg/util/regionsplit/model_handle.rs`、`pkg/ddl/split_region.rs`，确认简化原语如何进入真实模型路径，以及生成键如何交给 DDL split 流程。
- Go 对照：`pkg/util/regionsplit/split_handle.go` 与 `pkg/ddl/split_region.go`，核对整数/common handle、索引边界、最小 handle、错误和调用位置。
- 独立测试：`pkg/util/regionsplit/tests/split_handle_test.rs`、`pkg/util/regionsplit/tests/go_merge_32_test.rs`、`pkg/ddl/split_region_test.go`。前两者分别覆盖简化编码与真实模型适配；Go DDL 测试提供上层策略语义证据。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`explore "pkg/util/regionsplit/split_handle.rs splitHandle SplitHandle"` 定位 Go/Rust 对照符号；`node --file pkg/util/regionsplit/split_handle.rs --offset 195 --limit 420` 核对表、索引入口及全部编码辅助函数。图的 blast-radius 结果同时表明同名 Go 调用边较完整，而 Rust 生产路径经 `*ForModel` 适配层间接复用本文件原语。
- 本任务是纯文档分析，按计划不运行 Cargo。结构校验应确认目标文件存在且恰有十一个规定的二级标题；人工复核重点是简化 API 与生产适配层边界、Go 差异和安全扩展位置均有直接路径或符号依据。
