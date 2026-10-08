# `pkg/util/schemacmp/charset_collation.rs`

## 文件定位

本文件属于 `astersql-util-schemacmp` crate（入口为 [`lib.rs`](lib.rs)，清单为 [`Cargo.toml`](Cargo.toml)），实现字符集和排序规则两个 `Lattice` 元素。它不解析 SQL，也不查询字符集注册表；它把已经存在于字段类型或表选项中的名称编码为小型偏序格，供 schema 兼容性比较和合并使用。

直接接线有两处：[`type.rs`](type.rs) 的 `encodeFieldTypeToLattice` 把 `FieldType::GetCollate()` 包装为 `Collation`，其 `Compare`/`Join` 会随列类型元组一起执行；[`table.rs`](table.rs) 的 `encodeTableInfoToLattice` 把 `TableOptions::Collate` 包装为 `Collation`，其结果由 `EncodeWithOptions` 纳入整表格。`lib.rs` 通过私有模块 `charset_collation` 和 `pub use charset_collation::*` 把本文件的公开项再导出。

## 核心职责

1. `Charset` 对输入做 ASCII/Unicode 小写化，并把 MySQL 的 `utf8mb3` 别名规范化为 `utf8`。
2. `charsetLattice::Compare` 表示有限偏序：相同名称相等；`utf8mb4` 大于 `utf8` 和 `latin1`；其他不同名称不可比较。
3. `charsetLattice::Join` 返回可比较两者中的较大者，并为不可直接比较但存在已知公共上界的 `utf8`/`latin1` 返回 `utf8mb4`。
4. `Collation` 按第一个下划线拆分字符集和 suffix；`collationLattice` 要求 suffix 完全相同，再把比较或合并委托给字符集格。
5. 两种格均实现动态类型擦除、克隆和 `Unwrap`，从而能嵌入 `Tuple`、`Map` 等通用格结构。

该模型有意是白名单式兼容关系，不等价于完整的 MySQL 字符集转换能力。未知字符集仅在规范化名称相同时相等；不同未知值不会自动求公共上界。

## 主要符号

- `CHARSET_UTF8MB3`、`CHARSET_UTF8`、`CHARSET_UTF8MB4`、`CHARSET_LATIN1`：本文件支持的特殊名称常量。`utf8mb3` 只作为输入别名存在，存储时变为 `utf8`。
- `pub struct charsetLattice { pub value: String }`：字符集格元素。类型和字段均公开，是 `Charset` 的返回类型，也供 [`type.rs`](type.rs) 解码字段类型时读取规范化名称。
- `pub fn Charset(cs: impl AsRef<str>) -> charsetLattice`：字符集构造入口；接受字符串借用或拥有值，统一小写并处理别名。
- `impl Lattice for charsetLattice`：提供 `Unwrap`、`Compare`、`Join`、`clone_box` 以及 `Any` 下转型入口。
- `pub struct collationLattice`：排序规则格元素，内部保存私有的 `charset: charsetLattice` 和 `suffix: String`，只能通过 `Collation` 构造或经格操作产生。
- `pub fn Collation(collation: impl AsRef<str>) -> collationLattice`：按首个 `_` 拆分；字符集部分交给 `Charset`，suffix 转小写。没有 `_` 时整串作为字符集部分，suffix 为空。
- `collationLattice::unwrapString`：把内部两段重组为规范字符串；空 suffix 不添加下划线。
- `impl Lattice for collationLattice`：先检查动态类型与 suffix，再复用字符集格的比较/合并规则。

虽然名称保留了 Go 风格大小写，crate 根在 `lib.rs` 中显式允许 `non_camel_case_types`、`non_snake_case` 和相关 lint，以维持移植 API 对齐。

## 执行流程

字符集路径如下：

1. `Charset` 将输入转小写；若结果是 `utf8mb3`，改存 `utf8`。
2. `Compare` 先通过 `Any::downcast_ref::<charsetLattice>` 验证对端类型。
3. 名称相同返回 `0`；左侧为 `utf8mb4` 且右侧为 `utf8`/`latin1` 返回 `1`；反向返回 `-1`；其余返回 `incompatibleCharsetError`。
4. `Join` 再次验证类型并调用 `Compare`。可比较时，比较值非负就克隆左侧，否则克隆右侧；若比较失败但组合恰为 `utf8` 与 `latin1`，构造 `utf8mb4`；否则原样传播比较错误。

排序规则路径如下：

1. `Collation` 用 `split_once('_')` 只在第一个下划线处分割。因此 `utf8mb4_0900_ai_ci` 的 suffix 是完整的 `0900_ai_ci`，不会继续拆段。
2. 字符集段经 `Charset` 规范化，suffix 独立转小写；`unwrapString` 在需要对外返回或构造错误时重新拼接。
3. `Compare` 验证对端为 `collationLattice`。suffix 不同立即返回 `incompatibleCollationError`；suffix 相同才调用 `charsetLattice::Compare`。
4. `Join` 对相等或有序值直接选择较大者。比较失败且 suffix 不同，保留排序规则错误；suffix 相同则尝试字符集 `Join`。成功时下转回 `charsetLattice` 并沿用原 suffix，得到如 `utf8mb4_bin` 的结果；字符集合并失败时仍返回最初的比较错误。

## 数据与状态

两个格元素都是拥有数据的值对象：内部字符串由构造函数分配并持有，不借用调用者缓冲区。`charsetLattice` 和 `collationLattice` 均派生 `Clone`、`Debug`、`PartialEq`、`Eq`；真正供通用框架使用的语义比较是 `Lattice::Compare`，而不是只比较 Rust 结构的派生 `PartialEq`。

`Unwrap` 返回 `AnyValue`：字符集返回规范化的字符集字符串，排序规则返回重组后的规范字符串。`Join` 返回 `LatticeBox`（`Box<dyn Lattice>`），所以结果可作为元组格中的异构维度继续参与上层合并。没有全局缓存、注册表、可变静态变量或持久化状态。

格关系的重要不变量是：规范化后自反比较为 `0`；有序关系仅包含 `utf8/latin1 < utf8mb4`；`utf8` 与 `latin1` 虽不可比较，却有显式 join `utf8mb4`。排序规则只有 suffix 相同时才继承这些关系。

## 依赖与调用关系

本文件的直接语言级依赖只有 `std::any::Any` 与 crate 内格接口：`AnyValue`、`Lattice`、`LatticeRef`、`LatticeBox`、`IncompatibleError` 及三个错误构造函数。这些接口定义在 [`lattice.rs`](lattice.rs)。本文件没有直接使用 [`Cargo.toml`](Cargo.toml) 中的 `meta-model`、`parser-types` 或 `tidb-types`；这些 crate 依赖由同一 schemacmp crate 的字段类型、表模型和格式化层使用。

经 RustCodeGraph 与源码交叉核对的上游边包括：

- `type.rs::encodeFieldTypeToLattice -> Collation`：把列排序规则放入字段类型元组的第 8 个维度；该编码函数又由 `Type`、`typ::Compare`、`typ::Join` 调用。
- `type.rs::decodeFieldTypeFromLattice -> Charset`：从解包的 collation 推导并规范化字段 charset。
- `table.rs::encodeTableInfoToLattice -> Collation`：把表级 `TableOptions::Collate` 放入表元组；该函数由 `EncodeWithOptions` 调用。
- `Collation -> Charset`、`collationLattice::{Compare,Join} -> charsetLattice::{Compare,Join}`：构造与格运算的内部委托边。

因此本文件位于 schema 表示层而非请求入口：上层把列/表元数据编码成复合格，复合格逐维调用这里的规则来判断 DDL schema 是否兼容或求合并结果。

## 错误处理与边界

- 与另一种 `Lattice` 实现比较或合并时，`downcast_ref` 失败并返回 `typeMismatchError`，不会把异构值当作字符集处理。
- 字符集名称不同且不在显式偏序/特殊 join 中时，返回 `incompatibleCharsetError`，消息包含规范化后的两侧名称。
- collation suffix 不同优先返回 `incompatibleCollationError`，消息包含重组后的完整名称；suffix 相同但字符集不兼容时返回字符集错误。这一区分被 Go/Rust 测试明确断言。
- 无下划线的值（如 `binary`）合法，表现为 charset=`binary`、空 suffix；它只与相同规范化值相等，不能与 `utf8mb4_bin` 比较。
- 分割仅发生一次，所以包含多个下划线的 suffix 必须整体相等；例如 `general_ci` 与 `0900_ai_ci` 不相容。
- 构造函数不校验空串、未知名称或真实 MySQL 注册情况。未知名称可被保存，相同值可比较；不同值报不相容。
- `collationLattice::Join` 在字符集 join 成功后使用 `downcast_ref::<charsetLattice>().unwrap()`。该结果来自本文件的 `charsetLattice::Join`，按当前实现类型不变量不会失败；若未来修改该实现返回别的动态类型，这里会 panic，扩展时必须同步维护该不变量。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道、事务或 I/O。构造、比较和合并均为同步纯值计算；`Compare` 只借用两侧，`Join` 克隆选中的值或创建新值，不修改输入。因字段都是 `String` 且没有内部可变性，类型会自动具备常规的跨线程值传递/共享能力，但本文件不主动建立并发执行模型。

资源生命周期由 Rust 所有权管理：构造时拥有规范字符串，临时借用限于一次调用；`LatticeBox` 在离开上层格结构时自动释放。主要成本是输入小写化、错误消息参数或 join 结果所需的字符串分配，以及 trait object 装箱；没有需要显式关闭或回滚的资源。

## 与 Go 版本的对应关系

直接对照文件是 [`charset_collation.go`](charset_collation.go)，核心规则逐项一致：Go 的 `strings.ToLower`/`strings.Cut` 对应 Rust 的 `to_lowercase`/`split_once`；两者都规范化 `utf8mb3`，都只允许 `utf8`、`latin1` 向 `utf8mb4` 的偏序边，都把 `utf8` 与 `latin1` 的 join 提升为 `utf8mb4`，并且都要求 collation suffix 相等。

实现形态上的差异来自语言边界：Go 的 `Lattice` 通过接口类型断言并按值返回；Rust 的 `Lattice` 通过 `Any` 下转型、`AnyValue` 解包和 `Box<dyn Lattice>` 返回。Go 直接引用 `pkg/parser/charset` 常量，Rust 在本文件内定义四个字符串常量；因此新增或改名时必须人工保持两边一致。Rust `Charset`/`Collation` 接受 `impl AsRef<str>`，调用形式比 Go 的固定 `string` 参数更宽，但不改变规范化结果。

[`charset_collation_test.go`](charset_collation_test.go) 与 [`charset_collation_test.rs`](charset_collation_test.rs) 覆盖同名核心用例；[`charset_collation_1_aster_unit_test.rs`](charset_collation_1_aster_unit_test.rs) 还把这些规则放入更广的格与表编码路径中验证。当前读取范围内未发现 Rust 版本有意删减 Go 的本文件行为。

## 扩展指南

- 新增字符集兼容边时，首先在 `charsetLattice::Compare` 明确两个方向的 `-1/1`，再判断不可比较组合是否有唯一公共上界并相应扩展 `Join`。不要仅让 `Join` 成功却遗漏偏序关系，也不要依据字符集名称猜测转换能力。
- 新增别名时修改 `Charset` 的规范化逻辑；同时检查 [`type.rs`](type.rs) 中从 collation 反推 charset 的路径，确保 round-trip 得到期望规范名。
- 调整 collation 拆分或 suffix 规则时同时修改 `Collation`、`unwrapString`、`Compare` 和 `Join`，重点保护“首个下划线之后整体作为 suffix”以及“suffix 不同不得合并”的契约。
- 若改变 `charsetLattice::Join` 的返回动态类型，必须消除或更新 `collationLattice::Join` 中的具体类型下转型；否则会引入 panic。
- 测试必须保留在独立文件中。核心对齐用例同步更新 [`charset_collation_test.rs`](charset_collation_test.rs) 与 Go 的 [`charset_collation_test.go`](charset_collation_test.go)；涉及复合格/表编码的回归放入现有独立 Rust 测试（例如 [`charset_collation_1_aster_unit_test.rs`](charset_collation_1_aster_unit_test.rs)、[`type_test.rs`](type_test.rs) 或 [`table_test.rs`](table_test.rs)），不要嵌入生产源文件。
- 兼容风险集中在现有 schema 比较结果和错误分类；性能风险主要是扩大规则后增加分支或额外字符串规范化。修改后应验证比较的反对称方向、join 的交换性/上界性质、未知值行为及 Go/Rust 一致性。

## 验证依据

- 生产实现：[`charset_collation.rs`](charset_collation.rs) 的常量、两个构造函数、两个格类型及其 `Lattice` 实现。
- crate 边界：[`lib.rs`](lib.rs) 的模块声明/再导出与测试模块接线；[`Cargo.toml`](Cargo.toml) 的 `astersql-util-schemacmp` 包、`lib.rs` 入口和依赖声明。
- 上游接线：[`type.rs`](type.rs) 的 `encodeFieldTypeToLattice`、`decodeFieldTypeFromLattice`、`Type`/`typ`；[`table.rs`](table.rs) 的 `encodeTableInfoToLattice`、`EncodeWithOptions`。
- 通用契约与错误：[`lattice.rs`](lattice.rs) 的 `Lattice`、`LatticeRef`、`LatticeBox`、`typeMismatchError`、`incompatibleCharsetError`、`incompatibleCollationError`。
- Go 对照：[`charset_collation.go`](charset_collation.go)；Go 测试 [`charset_collation_test.go`](charset_collation_test.go)。
- Rust 测试：[`charset_collation_test.rs`](charset_collation_test.rs)；补充集成覆盖 [`charset_collation_1_aster_unit_test.rs`](charset_collation_1_aster_unit_test.rs)。
- RustCodeGraph 索引状态：项目索引包含 11,467 个文件；精确节点查询确认 `Charset` 位于本文件第 45 行、`Collation` 位于第 123 行，`Charset` 被本文件 `Join`/`Collation` 与 `type.rs::decodeFieldTypeFromLattice` 调用；`encodeFieldTypeToLattice` 被 `Type`、`Compare`、`Join` 调用，`encodeTableInfoToLattice` 被 `EncodeWithOptions` 调用。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前另运行任务指定的 11 章节结构检查，并人工复核所有行为陈述均可由上述源码、调用边或测试定位。
