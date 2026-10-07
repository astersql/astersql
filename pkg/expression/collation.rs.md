# `pkg/expression/collation.rs`

## 文件定位

`collation.rs` 是 `astersql-expression` crate 的表达式字符集、排序规则（collation）、强制转换优先级（coercibility）和字符覆盖范围（repertoire）规则中心。`pkg/expression/lib.rs` 以 `#[path = "collation.rs"] mod expression_collation` 装入本文件，并通过 `pub use expression_collation::*` 再导出其中的公开 API；`pkg/expression/Cargo.toml` 则确认 crate 名为 `astersql-expression`、库入口为 `lib.rs`，且移植对照包是 Go `pkg/expression`。

它位于表达式“构建和类型推导”阶段，而不是字符串比较的执行内核：上游的 planner bridge、扩展函数、`GROUP_CONCAT` 类型推导和列替换安全检查收集表达式元数据，调用 `CheckAndDeriveCollationFromExprs`；本文件选出结果字符集/排序规则或拒绝非法混用；下游再把 `ExprCollation` 写回返回 `FieldType`。真正按 collation 比较或生成 sort key 的实现来自 `astersql-util-collate`，本文件只通过 crate 根的 `collate` 再导出使用其中的二进制规则转换。

当前 Rust 接线需要特别区分两层：公开的 `CheckAndDeriveCollationFromExprs` 已有真实生产调用；本文件私有的 `deriveCollation` 保留 Go 按函数名选参和覆盖返回元数据的完整分派，但精确引用搜索未发现本文件外的 Rust 调用者。`pkg/expression/builtin.rs` 与 `distsql_builtin.rs` 各自还有同名局部函数，不能因名称相同而把它们视作调用了本文件的私有实现。

## 核心职责

1. 用 `collationInfo` 与 `CollationInfo` 为列、常量、标量函数等表达式提供统一的 collation 元数据读写契约，并让该状态参与哈希与相等判断。
2. 定义 MySQL 兼容的七级 `Coercibility`、`ASCII`/`EXTENDED`/`UNICODE` repertoire 位图，以及用于列替换安全判定的 collation 严格性表。
3. 通过 `inferCollation` 从左到右聚合表达式参数，处理 binary、同/异字符集、Unicode、utf8→utf8mb4、ASCII 和显式 `COLLATE` 的优先级规则。
4. 通过 `CheckAndDeriveCollationFromExprs` 在聚合结果上追加返回类型规范化、无损转换检查和 JSON 最大长度相关的 binary collation 修正。
5. 通过 `deriveCoercibilityForConstant`、`deriveCoercibilityForColumn` 给基本表达式补默认 coercibility；通过 `deriveCollation` 保存按 SQL 函数语义选择参与参数的 Go 对齐规则。
6. 生成 MySQL 风格的 illegal-mix-of-collations 错误，并对未知字符集的 binary collation 查找留下日志和兜底值。

## 主要符号

- `ExprCollation`：一次推导的结果值，包含 `Coer`、`Repe`、`Charset`、`Collation`；字段为公开字段，调用者可在函数特例中覆盖。
- `collationInfo`：表达式节点内部状态。`coer` 和 `coerInit` 是原子字段；`repertoire`、`charset`、`collation`、`isExplicitCharset` 需可变借用才能修改。`Hash64` 与 `Equals` 按这六项的固定顺序/含义比较，因而“未初始化的零”和“显式设置为 `CoercibilityExplicit` 的零”不同。
- `CollationInfo`：对象层元数据 trait，定义 coercibility、repertoire、字符集/排序规则和显式字符集标志的读写。`pkg/expression/core_impl.rs` 将其实现接到 `Column`、`Constant`、`CorrelatedColumn` 和 `ScalarFunction`；`builtinFunc` 也以它为父 trait。
- `Coercibility` 与 `CoercibilityExplicit` 至 `CoercibilityIgnorable`：值域为 `0..=6`，数字越小优先级越高，依次表示显式、冲突、隐式、系统常量、字面量、数值、可忽略。
- `Repertoire`、`ASCII`、`EXTENDED`、`UNICODE`：字符覆盖范围位图。聚合时以按位或合并；非字符串值也保存该字段，但其内容只在字符串转换判断中有意义。
- `CollationStrictnessGroup` / `CollationStrictness`：进程级只读 `LazyLock<HashMap>`。`pkg/expression/util.rs::checkCollationStrictness` 用它判断替换后的 collation 是否与原规则相同或更严格。
- `CollationInput` / `InferCollationMetadata`：不依赖可执行 `Expression` 的公开值对象入口，便于直接验证聚合规则。它覆盖聚合核心，但不会做 `safeConvert` 的常量实际字节校验、JSON/BIT 类型规范化或 `fixStringTypeForMaxLength`。
- `deriveCoercibilityForScalarFunc`：保护性 panic；标量函数必须在构造时已完成推导。`deriveCoercibilityForConstant` 按 NULL、非字符串、字符串字面量返回 ignorable/numeric/coercible；`deriveCoercibilityForColumn` 对 NULL、BIT、JSON/字符串、其他类型分别返回 ignorable/implicit/implicit/numeric。
- `deriveCollation`：私有的函数名分派器；决定哪些实参参与聚合，并为比较、`LIKE`、系统常量、哈希/编码函数、JSON 函数、`CAST` 等覆盖返回元数据。当前没有确认到本文件外 Rust 调用边。
- `CheckAndDeriveCollationFromExprs`：主要公开入口，串联 `inferCollation`、非法混用检查、numeric→string 规范化、`safeConvert` 和 `fixStringTypeForMaxLength`。
- `inferCollation`：表达式版本的核心折叠器；`isUnicodeCollation`、`isBinCollation`、`getBinCollation` 是其规则辅助函数。
- `illegalMixCollationErr`：按参数个数生成两参、三参或通用错误文本，并用 `GetDisplayName` 显示 SQL 操作名。
- `hashFieldType`：crate 内可见的字段类型哈希辅助函数，按类型码、flag、长度、小数位、字符集、collation 顺序写入哈希器。

## 执行流程

`CheckAndDeriveCollationFromExprs(ctx, funcName, evalType, args)` 的主流程是：

1. `inferCollation` 取得第一个参数的 repertoire/coercibility，并规范化类型元数据：JSON 一律按 `utf8mb4`/`utf8mb4_bin`，BIT 一律按 `binary`/`binary`，其他类型沿用自身 `FieldType`。
2. 参数按原顺序左折叠，即 `agg(a,b,c)=agg(agg(a,b),c)`。任何一侧是 binary charset 时，binary 可与其他字符集共存；coercibility 相同时 binary 一侧获胜，否则数值更小的一侧优先。
3. 字符集不同时，只接受代码明确列出的无损路径：源 repertoire 是 ASCII、低优先级的系统常量/字面量、非 Unicode 转 Unicode、utf8 转 utf8mb4，或等优先级时依据 repertoire 选择能容纳扩展字符的一侧。无法转换时先记为 `CoercibilityNone` + binary，并设置 `unknownCS`，允许后续显式 collation 解开冲突；折叠结束仍未解开则失败。
4. 字符集相同时，较小 coercibility 获胜。同级且 collation 不同：两个显式规则直接失败；已有 `_bin` 保留，新的 `_bin` 接管；都不是 `_bin` 时降为 `CoercibilityNone` 并选择该字符集的 binary collation。
5. 聚合失败由 `illegalMixCollationErr` 转成 `Error`。非字符串结果如果仍是 `CoercibilityNone` 也失败；字符串结果若聚合为 numeric，则改用连接字符集/排序规则、coercible 和 ASCII。
6. `safeConvert` 遍历原参数。目标字符集相同、ASCII repertoire 或 binary string 可直接通过；常量会实际 `EvalString` 并验证目标 encoding；非常量只有在 binary 或 Unicode 目标等允许路径下通过。
7. `fixStringTypeForMaxLength` 对可能传播 JSON 大长度的字符串函数检查指定实参；命中时通过 `collate::ConvertAndGetBinCollation` 将结果调整到 binary collation。

私有 `deriveCollation` 在上述通用流程之前按函数语义选参：例如 `LEFT`/`RIGHT` 只看首参，`INSERT` 看第 1、4 个参数，`LPAD`/`RPAD` 看第 1、3 个参数，`IF` 忽略条件只看两个结果分支，`CASE` 只收集 THEN/ELSE；字符串比较和 `LIKE` 聚合完再把结果标成 numeric/ASCII；系统常量使用默认字符集，编码/摘要类返回 connection charset + coercible/ASCII，JSON pretty/quote 固定为 utf8mb4。未命中特例时，非字符串默认 numeric/ASCII/binary，字符串默认 connection charset + coercible。

## 数据与状态

`collationInfo` 把“coercibility 的值”和“该值是否初始化”分开保存。`Default` 令数值为 0 但 `coerInit=false`；`SetCoercibility` 先写 `coer` 再发布 `coerInit=true`。`Clone` 对原子值做快照并创建新的原子对象，所以克隆后修改 coercibility 不会共享原子存储。字符集和 collation 以拥有所有权的 `String` 保存，`CharsetAndCollation` 返回副本而非借用。

`ExprCollation` 和 `CollationInput` 都是一次调用拥有的纯值对象，没有全局缓存。聚合会复制字符串并对 repertoire 做按位或；参数表达式本身不被修改。两个严格性 map 使用 `LazyLock` 首次访问初始化，此后只读；表中只列出当前支持比较的常见规则，未知项在 `checkCollationStrictness` 中返回 false。

`coerString` 的数组下标假设 coercibility 落在 `0..=6`。该不变量由本文件常量和正常表达式构造维持；若外部实现 `CollationInfo` 返回越界整数，错误格式化路径会索引越界，因此新增实现必须保持值域。

## 依赖与调用关系

- 装配边：`pkg/expression/lib.rs` 装入并公开再导出本文件；`use crate::*` 让实现使用 crate 根再导出的 `Expression`、`BuildContext`、`EvalContext`、`types`、`charset`、`collate`、`ast`、`chunk`、`errors`、`logutil` 等。对应 Cargo path 依赖包括 parser charset/AST/MySQL、types、util collate/chunk/logutil 和 planner base。
- 表达式对象边：`pkg/expression/core_impl.rs` 把 `CollationInfo` 接到主要表达式类型；`pkg/expression/expression.rs` 将 `CollationInfo` 纳入表达式 trait 组合；`column.rs`、`constant.rs`、`builtin.rs` 的结构保存 `collationInfo`。
- 公开推导入口的直接上游：`pkg/expression/aggregation/base_func.rs::typeInfer4GroupConcat` 用结果设置 `GROUP_CONCAT` 返回类型；`planner_bridge.rs::InferType4ControlFuncsVariadic` 用它推导控制函数结果；`extension.rs::extensionFuncClass::getFunction` 用结果设置扩展函数返回类型和参数 CAST；`util.rs::ColumnSubstituteImpl` 在替换前后重算 collation，防止语义减弱。RustCodeGraph 还定位到 `builtin.rs` 的正式构建路径和 planner 表达式重写中的调用。
- 严格性表的下游：`pkg/expression/util.rs::checkCollationStrictness` 查询两张 map，允许相同组或配置为更严格的目标组。
- 编码/排序规则下游：`safeConvert` 调用 charset encoding 的 `IsValid` 校验常量字节；`fixStringTypeForMaxLength` 调用 util-collate 转换 binary collation；`getBinCollation` 只显式覆盖 utf8、utf8mb4、gbk。
- Go 对照：`pkg/expression/collation.go` 是逐逻辑对照源，`Cargo.toml` 的 porting metadata 也把 Go 包指向 `pkg/expression`。

## 错误处理与边界

`inferCollation`/`InferCollationMetadata` 用 `Option` 表示无法得到兼容结果；公开表达式入口将 `None` 转为包含操作名、collation 和 coercibility 名称的 `Error`。两参和三参错误保留详细元数据，其他参数数量使用通用文本。与 Go 不同，Rust 当前由 `errors::New(format!(...))` 直接构造消息，没有调用 Go 的 `collate.ErrIllegalMix*Collation` 错误类；调用者不应依赖更细的类型化错误身份。

`safeConvert` 的常量求值错误、目标编码无法表示常量字节、或非常量缺少安全转换路径都统一返回 false，随后转成 illegal-mix 错误。它不会修改原常量，也不会用替换字符掩盖损失。空参数是合法输入：表达式版本使用 parser 默认字符集/排序规则，`InferCollationMetadata` 明确返回 utf8mb4 常量；在当前默认配置下二者相同，但前者保留运行时默认来源。

`deriveCoercibilityForConstant` 对 `RetType` 使用 `unwrap()`，要求 Constant 已有返回类型；`deriveCoercibilityForColumn` 同样要求 `RetType` 存在。`deriveCollation` 多处分支按固定参数位置索引，依赖函数类已经完成参数个数校验。直接绕过构建器传入空/不足参数可能 panic，不是受支持的公开调用方式。

`getBinCollation` 遇到未知字符集会记录后台错误并退回 utf8mb4 binary collation；这被注释为不可达兜底，不代表未知字符集已获得正确支持。`deriveCoercibilityForScalarFunc` 也故意 panic，用于暴露遗漏的构造期推导。

## 并发与资源生命周期

本文件不创建线程、任务、通道、事务或外部资源。每次推导只借用构建/求值上下文和表达式切片，并返回拥有字符串的结果；常量安全检查使用临时空 `chunk::Row` 求值，调用结束即释放。

可并发共享的状态限于 `collationInfo` 的两个原子字段和两张只读 `LazyLock` map。所有 coercibility/初始化标志的读写与 clone/hash/equals 都使用 `Ordering::SeqCst`，因此发布顺序明确；但 repertoire、charset、collation 和 explicit 标志不是原子量，只能通过 `&mut self` 修改。也就是说，共享只读表达式可并发读取，非原子元数据的变更仍要求调用者持有独占可变借用；本文件没有内部锁替调用者协调复合更新。

`SetCoercibility` 的两次原子写不是一个整体事务：并发读者可能在极短窗口读到新值而初始化标志仍为 false，但不会看到 `coerInit=true` 后仍缺少先前的值写入。正常生命周期是在表达式构建期设置、执行期只读；扩展代码不应在运行期并发重配同一表达式的 collation 状态。

## 与 Go 版本的对应关系

Rust 的 `ExprCollation`、`collationInfo`、`CollationInfo`、coercibility/repertoire 常量、严格性表、默认推导函数、`deriveCollation`、`CheckAndDeriveCollationFromExprs`、`safeConvert`、`inferCollation` 和错误格式化顺序均可在 `pkg/expression/collation.go` 找到直接对应。原子 coercibility 对齐 Go `sync/atomic`，`coerInit` 对齐 `go.uber.org/atomic.Bool`；Rust 统一使用标准库 `AtomicI32`/`AtomicBool` 和顺序一致内存序。

主要实现差异如下：

- Rust 额外提供 `CollationInput`/`InferCollationMetadata`，把核心规则暴露为纯元数据 API，Go 文件没有该入口。它与表达式版算法高度相似，但输入不会自动规范化 JSON/BIT，也不执行常量字节安全验证。
- Go `Equals` 接受 `*collationInfo` 和值形式；Rust `Any::downcast_ref` 只匹配具体 `collationInfo` 值的动态类型，文档注释所称“值或引用”不应理解为任意引用层级都可 downcast。
- Go illegal-mix 使用 collate 包的专用错误模板；Rust目前构造普通 expression `Error` 文本。消息内容对齐，但错误身份未证明等价。
- Rust 的 `deriveCollation` 保留 Go 函数分派，但当前目标文件中未确认生产调用；实际 Rust builtin 构造还存在其他文件内的同名推导函数。因此它是已实现但尚不能据此证明接入主链的对照逻辑。
- `InferCollationMetadata([])` 固定 utf8mb4；表达式版和 Go `inferCollation()` 使用 `GetDefaultCharsetAndCollate()`。如果未来默认值可变化，需要决定纯元数据入口是否也应接收上下文。

`pkg/expression/collation_test.rs` 对照 Go `collation_test.go`，真正执行了 hash/equals、部分元数据推导和 compare-string 断言；大量 `TestDeriveCollation` Go case 仍以注释保存，并转调 `collation_aster_unit_test::run_collation_parity_suite`。`collation_33_aster_unit_test.rs` 进一步覆盖聚合矩阵；测试证据应区分“实际 Rust 断言”和“仅保留的 Go 注释”。

## 扩展指南

新增字符集或 collation 时，至少同步检查 `isUnicodeCollation`、`isBinCollation`、`getBinCollation`、两张严格性表以及 parser charset 常量。`isBinCollation` 表示 coercibility 聚合中的 `_bin` 语义，不等同于 util-collate 的“sort key 等于原始字节”；例如 `gbk_bin` 属于前者，而 binary charset 走单独分支。新增规则时需同时评估这两个概念，不能机械共用列表。

新增或修改 SQL 函数的 collation 规则时，应先确认生产构建路径实际使用本文件的哪个入口。若接入私有 `deriveCollation`，应在已有参数校验之后调用，并与 Go `collation.go::deriveCollation` 同步函数集合、参与参数位置、返回 `EvalType` 和特殊 coercibility/repertoire。若现有正式 builtin 走自己的局部推导，应避免只修改本文件而造成双轨漂移。

修改聚合算法时，应保持左右折叠顺序、binary 优先、显式冲突、ASCII/Unicode/utf8mb4 选择和 `unknownCS` 延迟失败语义；同时更新纯值入口与表达式入口，避免两套算法分叉。回归测试放在独立测试文件，不放回 `collation.rs`：核心表驱动用例更新 `pkg/expression/collation_test.rs` 和 Go `collation_test.go` 对照意图，细粒度矩阵更新 `collation_33_aster_unit_test.rs`；常量实际编码失败、JSON/BIT 规范化和 illegal-mix 文本需要走表达式版入口。

并发扩展必须维持构建期写、执行期读的生命周期。若新增复合可变状态，不能仅给其中一个字段加原子量就宣称整体一致；同时要更新 `Clone`、`Hash64`、`Equals` 和 cache snapshot 逻辑。性能上，`safeConvert` 可能对常量执行求值和编码扫描，新增调用点应留在构建/规划阶段，避免进入逐行执行热路径。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/expression/collation.rs` 分四段读取了全部 863 行，并报告该文件被 21 个文件使用。
- RustCodeGraph `explore "pkg/expression/collation.rs collation functions callers callees"`：定位了 `CheckAndDeriveCollationFromExprs` 的 Rust 调用者，包括 `util.rs::ColumnSubstituteImpl`、`planner_bridge.rs::InferType4ControlFuncsVariadic`、`extension.rs::getFunction`、`aggregation/base_func.rs::typeInfer4GroupConcat`，并确认其调用 `inferCollation`、`safeConvert`、`fixStringTypeForMaxLength`、`illegalMixCollationErr`。单独的 `callers` 命令在本地索引查询中无输出且长时间未完成，已中止，未把它当作否定证据。
- 精确源码调用边：用 RustCodeGraph `node --file` 读取了 `pkg/expression/aggregation/base_func.rs`、`planner_bridge.rs`、`extension.rs`、`util.rs`、`core_impl.rs` 的相关片段；用引用搜索区分了本文件私有 `deriveCollation` 与 `builtin.rs`、`distsql_builtin.rs` 的同名实现。
- crate/装配：读取 `pkg/expression/Cargo.toml` 和 `pkg/expression/lib.rs`，核对包名、库入口、path 依赖、Go porting metadata、模块装入与公开再导出；目标包不存在 `pkg/expression/doc.go`。
- Go 对照：读取 `pkg/expression/collation.go` 的类型、常量、函数分派、聚合、安全转换和错误路径；读取 `pkg/expression/collation_test.go` 的 `TestCollationHashEquals`、`TestInferCollation`、`TestDeriveCollation`、`TestCompareString` 测试矩阵。
- Rust 测试：用 RustCodeGraph 完整读取 `pkg/expression/collation_test.rs`，确认实际执行的 hash/equals、元数据聚合、空输入和字符串比较断言，以及仍以 Go 注释保留的分派矩阵；RustCodeGraph 调用边与引用搜索还定位了 `collation_33_aster_unit_test.rs`、`explicit_collation_test.rs` 和 `scalar_function_test.rs` 的相关覆盖。
- 本任务只新增说明文档，按计划未运行 Cargo。人工复核已覆盖“文件为何存在、当前如何接线、核心算法如何运行、失败边界、并发状态以及安全扩展位置”；交付时另运行固定十一章节的结构验证和文档变更范围检查。
