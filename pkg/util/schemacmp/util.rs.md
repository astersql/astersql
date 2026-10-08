# `pkg/util/schemacmp/util.rs`

## 文件定位

本文件属于 `astersql-util-schemacmp` crate，是 MySQL 整数类型编号和 BLOB 类型编号的语义排序辅助层。crate 入口 `pkg/util/schemacmp/lib.rs:39-42` 以私有模块 `mod util` 装载它，再将两个函数公开再导出；因此外部可以从 crate 根调用这两个函数，但不能直接访问 `util` 模块。crate 的整体职责是用格（lattice）比较和合并 schema，本文件只提供其中最小、无状态的三态比较原语。

`pkg/util/schemacmp/Cargo.toml` 将库入口设为 `lib.rs`，并通过 `meta-model`、`parser-types`、`tidb-types` 三个本地依赖接入元数据、解析器类型与 TiDB 类型。对本文件而言，直接使用的只有 crate 根在 `pkg/util/schemacmp/lib.rs:23` 再导出的 `mysql` 命名空间。

## 核心职责

- `compareMySQLIntegerType` 把整数类型按语义宽度排列为 `TypeTiny < TypeShort < TypeInt24 < TypeLong < TypeLonglong`，修正 `TypeInt24 == 9` 与宽度顺序不一致的问题（`pkg/util/schemacmp/util.rs:27-54`；常量编号见 `pkg/parser/mysql/type.rs:25-41`）。
- `compareMySQLBlobType` 把 BLOB 类型按容量排列为 `TypeTinyBlob < TypeBlob < TypeMediumBlob < TypeLongBlob`，修正 `TypeBlob == 0xfc` 在数值上大于其余 BLOB 编号、但语义上位于 Tiny 与 Medium 之间的问题（`pkg/util/schemacmp/util.rs:60-87`；编号见 `pkg/parser/mysql/type.rs:67-73`）。
- 两个函数都返回 Go 风格的 `-1/0/1`，分别代表左值小于、等于或大于右值。它们比较的是约定好的类型族内语义序，不负责验证输入是否真的属于对应类型族。

## 主要符号

- `pub fn compareMySQLIntegerType(a: u8, b: u8) -> i32`（`pkg/util/schemacmp/util.rs:27`）：公开的整数类型三态比较函数。相等时快速返回 `0`；任一侧是 `TypeInt24` 时执行特殊分支；否则按原始 `u8` 编号比较。
- `pub fn compareMySQLBlobType(a: u8, b: u8) -> i32`（`pkg/util/schemacmp/util.rs:60`）：公开的 BLOB 类型三态比较函数。相等时快速返回 `0`；任一侧是 `TypeBlob` 时执行特殊分支；否则按原始编号比较。
- 本文件没有常量、结构体、枚举、trait、`impl` 或条件编译项。函数名保留 Go 命名风格，crate 根通过 `#![allow(non_snake_case)]` 接受这种移植命名（`pkg/util/schemacmp/lib.rs:8-13`）。

## 执行流程

整数比较流程如下（`compareMySQLIntegerType`）：

1. 若 `a == b`，返回 `0`。
2. 若 `a == TypeInt24`，仅当 `b <= TypeShort` 时返回 `1`，否则返回 `-1`。在预期的整数类型集合内，这把 MEDIUMINT 放在 SMALLINT 与 INT 之间。
3. 否则若 `b == TypeInt24`，对称地在 `a <= TypeShort` 时返回 `-1`，否则返回 `1`。
4. 两侧都不是 `TypeInt24` 时，原始编号对其余预期整数类型恰好保持语义序；故 `a < b` 返回 `-1`，否则返回 `1`。

BLOB 比较流程与之同构（`compareMySQLBlobType`）：先处理相等，再把 `TypeBlob` 特判为仅大于 `TypeTinyBlob`、小于 `TypeMediumBlob` 和 `TypeLongBlob`，最后对不含 `TypeBlob` 的组合按编号比较。

两个流程均为固定分支数的 O(1) 计算。`pkg/util/schemacmp/type_2_aster_unit_test.rs:67-100` 对 5 个整数类型的 25 个有序组合和 4 个 BLOB 类型的 16 个有序组合逐一验证结果。

## 数据与状态

输入与 MySQL 类型常量一样是 `u8`，输出是 `i32` 三态标记。函数只读取参数和编译期常量，不修改全局状态，不缓存结果，也不持有任何借用。

对预期类型集合，比较关系满足全序所需的反对称方向与传递顺序，且返回值严格限制为 `-1`、`0`、`1`。不过这种保证依赖调用者先把输入限制在对应类型族：例如整数函数对一个非 `TypeInt24` 的未知编号仍按数值排序，BLOB 函数遇到 `TypeBlob` 与未知编号时也会走特判，而不会拒绝输入。

## 依赖与调用关系

下游依赖只有 `crate::mysql` 中的类型编号常量（`pkg/util/schemacmp/util.rs:21`）。函数内部不调用其他函数；RustCodeGraph 对两个目标符号的 `callees` 查询均为空。

上游分为公开面和当前内部接线两层：

- `pkg/util/schemacmp/lib.rs:42` 在 crate 根公开再导出两个函数。`pkg/util/schemacmp/type_2_aster_unit_test.rs:21,67-100` 通过 `use super::*` 使用这些再导出，并直接验证目标函数。
- RustCodeGraph 对 `pkg/util/schemacmp/util.rs` 两个精确符号的 `callers` 查询均为空；源码检索也未发现生产 Rust 代码调用 `util.rs` 的函数。
- 当前 `fieldTp::Compare` 的实际生产接线位于 `pkg/util/schemacmp/lattice.rs:537-589`：该文件在 `:542-564` 定义了自己的同名私有 rank 实现，并在 `:583-587` 调用私有版本。`fieldTp::Join` 再通过 `self.Compare(other)` 选取较大类型（`:591-600`）。因此 `util.rs` 当前是公开兼容辅助 API 和直接测试对象，而不是 Rust 字段类型格生产路径的被调用实现。
- Go 版本不存在这份重复接线：`pkg/util/schemacmp/lattice.go:337-390` 的 `fieldTp.Compare` 与 `fieldTp.Join` 都直接调用 `util.go` 的两个包内函数。

## 错误处理与边界

本文件不返回 `Result`、不构造错误，也不会 panic；任何 `u8` 输入都会得到一个三态结果。类型族检查与跨族错误属于上层职责：Rust 的 `fieldTp::Compare` 先通过 `mysql::IsIntegerType` 或 `types::IsTypeBlob` 选择比较器，否则返回 `incompatibleTypeError`（`pkg/util/schemacmp/lattice.rs:576-589`）。

边界风险在于公开函数本身不做这种选择。调用者若把浮点、字符串或未知类型编号传给整数/BLOB 比较器，仍会收到看似有效的次序，而不是错误。扩展类型编号时也不能假设数值顺序自动等于语义顺序；`TypeInt24` 与 `TypeBlob` 正是反例。

## 并发与资源生命周期

两个函数都是只依赖值参数和常量的纯计算：无堆分配、无 I/O、无锁、无原子变量、无线程/异步任务、无通道、无事务，也没有需要释放的资源。它们可被并发调用，生命周期在单次函数栈帧返回时结束；文件中不存在共享可变状态导致的竞态面。

## 与 Go 版本的对应关系

`pkg/util/schemacmp/util.rs` 逐分支对应 `pkg/util/schemacmp/util.go`：Go 的 `byte` 映射为 Rust `u8`，Go 的 `int` 三态结果在 Rust 固定为 `i32`；相等快速路径、`TypeInt24`/`TypeBlob` 两个方向的特判以及默认编号比较均保持一致。

Go 中函数是包内非导出符号，而 Rust 函数声明为 `pub` 并从 crate 根再导出，这是可见性差异。更重要的迁移差异是：Go 的 `fieldTp.Compare` 和 `fieldTp.Join` 直接复用 `util.go`；Rust 的 `lattice.rs` 当前重复定义 rank 版本。对预期类型矩阵，两种 Rust 实现与 Go 测试意图一致：`pkg/util/schemacmp/type_2_aster_unit_test.rs:67-100` 直接覆盖本文件，`pkg/util/schemacmp/lattice_test.rs:428-461` 覆盖实际字段类型格路径，Go 的完整预期案例见 `pkg/util/schemacmp/lattice_test.go:344-535`。

## 扩展指南

- 新增或调整整数/BLOB 类型时，先确认 MySQL 编号与语义宽度序是否一致；若不一致，应修改相应特殊分支或改为显式 rank 表，而不能仅扩大上层类型族判断。
- 当前存在 `util.rs` 与 `lattice.rs` 双实现。任何顺序变更必须同步两处，并同步 `pkg/util/schemacmp/type_2_aster_unit_test.rs`（直接 API）和 `pkg/util/schemacmp/lattice_test.rs`（生产格路径）；还应对照更新 Go 的 `util.go`、`lattice_test.go`，否则公开 API 与 schema 合并行为可能漂移。
- 若未来消除重复实现并让 `fieldTp::Compare` 调用本文件函数，应先保持类型族检查仍位于上层，避免把“不兼容类型应报错”退化成按编号排序；测试逻辑仍应放在独立的 `*_test.rs` 文件中，不嵌入生产源文件。
- 性能风险很低，但此比较位于 schema 格的组合比较内；扩展时宜保持无分配 O(1) 路径。兼容性风险主要是排序变化会改变 `Join` 选择的字段类型，继而影响 schema 合并结果。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件，目标目录的 21 个 Go/Rust 文件均在索引中；目标 `util.rs` 被识别为 87 行、3 个图节点（文件节点加两个函数）。
- RustCodeGraph 源码/符号查询：`node --file pkg/util/schemacmp/util.rs`；`query compareMySQLIntegerType --kind function`；`query compareMySQLBlobType --kind function`；两个精确目标符号的 `callers` 和 `callees` 查询均返回空数组。图查询同时揭示 `lattice.rs` 与 `util.go` 的同名定义，随后用精确文件节点核对实际接线。
- 已读生产与配置路径：`pkg/util/schemacmp/util.rs`、`pkg/util/schemacmp/lib.rs`、`pkg/util/schemacmp/lattice.rs`、`pkg/util/schemacmp/Cargo.toml`、`pkg/parser/mysql/type.rs`。
- 已读 Go 对照：`pkg/util/schemacmp/util.go`、`pkg/util/schemacmp/lattice.go`。
- 已读测试证据：`pkg/util/schemacmp/type_2_aster_unit_test.rs`、`pkg/util/schemacmp/lattice_test.rs`、`pkg/util/schemacmp/lattice_test.go`。目标文件没有同名独立 `util_test.rs`；直接 API 测试位于前述 `type_2_aster_unit_test.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验证要求文档存在且恰含规定的 11 个二级章节；交付前另行执行任务文件指定的精确命令。
