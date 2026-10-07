# `pkg/lightning/mydump/bytes.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-lightning-mydump`。包清单 `pkg/lightning/mydump/Cargo.toml` 以 `lib.rs` 为库入口，并通过 `package.metadata.porting.go-package = "pkg/lightning/mydump"` 标明对应 Go 包。`pkg/lightning/mydump/lib.rs` 用 `mod bytes; pub use bytes::*;` 编译并公开再导出本文件的 API，因此外部使用者可从 crate 根访问 `ByteSet`、`make_byte_set`、`makeByteSet`、`index_any_byte` 和 `IndexAnyByte`。

它是一个无 I/O 的字节扫描基础件，对照 `pkg/lightning/mydump/bytes.go` 中从 Go 标准库思路复制的 `byteSet`、`makeByteSet`、`contains` 与 `IndexAnyByte`。设计用途是先把一组特殊字节压成 256-bit 集合，再在线性扫描输入时快速判断每个字节是否命中。当前仓库的精确 Rust 引用搜索只找到本文件定义和 `lib.rs` 的模块再导出；Rust CSV 解析器 `pkg/lightning/mydump/csv_parser.rs` 当前直接比较分隔符字节序列，没有接入这些 API。因此本文件目前是已公开、可调用但尚未进入 Rust mydump 生产解析主链的移植基础件，不能把 Go 侧的实际接线误写成 Rust 现状。

## 核心职责

- `ByteSet` 用固定 `[u32; 8]` 表示全部 256 种 `u8` 值的成员关系，避免哈希表、排序或动态分配。
- `make_byte_set` 把任意字节切片编码为位图；重复字节通过按位或自然去重，空切片生成全零集合。
- `ByteSet::contains` 用同一套桶/位寻址规则完成常数时间成员测试。
- `index_any_byte` 从左到右查找输入中第一个属于集合的字节，并保留 Go API 的 `-1` 未命中哨兵。
- `makeByteSet` 和 `IndexAnyByte` 是 Go 风格兼容名，仅转发给 snake_case 实现，不维护第二套逻辑。

## 主要符号

- `pub struct ByteSet([u32; 8])`（`bytes.rs:27`）：公开类型、私有内部数组。派生 `Clone`、`Copy` 和 `Default`；默认值是八个零桶。其内存负载为 8 × 32 bit，即 32 字节，恰好覆盖 256 个 bit。
- `pub fn make_byte_set(chars: &[u8]) -> ByteSet`（`bytes.rs:30`）：规范 Rust 构造入口。对每个 `byte`，以 `byte >> 5` 选择 8 个桶之一，以 `byte & 31` 选择桶内 32 个 bit 之一。
- `pub fn makeByteSet(chars: &[u8]) -> ByteSet`（`bytes.rs:40`）：Go 命名兼容入口，唯一行为是调用 `make_byte_set`。
- `pub fn ByteSet::contains(&self, byte: u8) -> bool`（`bytes.rs:46`）：共享借用查询。方法本身公开，但由于元组字段私有，crate 外不能绕过构造函数直接填充桶。
- `pub fn index_any_byte(bytes: &[u8], set: &ByteSet) -> isize`（`bytes.rs:52`）：规范 Rust 扫描入口。返回首个命中的零基下标，或者 `-1`。
- `pub fn IndexAnyByte(bytes: &[u8], set: &ByteSet) -> isize`（`bytes.rs:62`）：Go 命名兼容入口，唯一行为是调用 `index_any_byte`。

文件没有模块级常量、trait、enum、条件编译项或内嵌测试模块。

## 执行流程

构造阶段由 `make_byte_set` 完成：先取得 `ByteSet::default()`，然后逐字节计算 `bucket = byte >> 5` 与 `bit = byte & 31`，最后执行 `bucket_value |= 1_u32 << bit`。例如字节 0 和 31 落在第 0 桶的最低位与最高位，字节 32 落在第 1 桶最低位，字节 255 落在第 7 桶最高位。由于索引完全由 `u8` 拆分而来，桶下标必在 `0..8`、位移必在 `0..32`。

查询阶段由 `contains` 复用相同映射：读取对应桶，把它与单 bit 掩码按位与，结果非零即表示存在。

扫描阶段由 `index_any_byte` 完成：`bytes.iter().enumerate()` 保持输入顺序；遇到第一个 `set.contains(byte)` 为真的元素时立即把 `usize` 下标转换成 `isize` 返回；遍历结束仍未命中则返回 `-1`。因此命中时不会继续检查后续输入，结果稳定地指向最左命中项。两个 Go 风格函数只增加一层同步转发，不改变上述流程。

## 数据与状态

`ByteSet` 的全部持久状态是私有 `[u32; 8]`。集合建立后，本文件没有修改集合的 API；`contains` 和两个扫描入口都只接收共享借用。因此复制一个 `ByteSet` 会复制固定的 32 字节位图，不共享可变状态，也不需要堆分配。

集合语义只面向原始字节而非 Unicode 字符或码点：多字节 UTF-8/其他编码序列会被逐字节处理。输入 `chars` 的顺序和重复次数不会改变最终集合。扫描函数借用输入切片与集合，不保留引用，不改变任一参数。

返回类型选择 `isize` 是为了表达与 Go `int` 相同的 `-1` 哨兵约定。命中路径从 `usize` 转为 `isize`；理论上若切片下标大于 `isize::MAX`，Rust 的 `as` 转换会截断/回绕为负值。现实进程无法在可寻址平台上构造超过 `isize::MAX` 的有效切片，但扩展 API 时仍应把这一类型边界视为兼容约束。

## 依赖与调用关系

本文件只依赖 Rust 核心/标准能力：数组默认值、切片迭代、位运算与整数转换；`pkg/lightning/mydump/Cargo.toml` 中的 `encoding_rs`、`hex`、`libm`、`percent-encoding`、`regex`、`thiserror` 都不是本文件的直接依赖。

已验证的内部调用边为：`makeByteSet -> make_byte_set`、`index_any_byte -> ByteSet::contains`、`IndexAnyByte -> index_any_byte`。`make_byte_set` 还调用派生的 `ByteSet::default`。RustCodeGraph 将该文件识别为 7 个符号，并显示它被包含测试文件在内的多个 crate 文件“使用”；但精确的 `callers/callees` 查询没有返回可归属的调用边，仓库级 `rg` 进一步确认除定义外没有 Rust 调用点。因此不能依据文件级“used by”关系宣称具体生产调用者。

Go 侧的真实主链不同：`pkg/lightning/mydump/csv_parser.go::NewCSVParser` 用 `makeByteSet` 构造 `quoteByteSet`、`unquoteByteSet`、`newLineByteSet`，`CSVParser.readUntil` 再调用 `IndexAnyByte` 查找缓冲区中的第一个候选特殊字节；候选字节之后仍由 CSV 解析器验证完整的多字节分隔符/引号语义。Rust 侧 `pkg/lightning/mydump/csv_parser.rs::read_field` 当前逐位置比较完整 `comma`、`quote`、`newline` 序列，`CsvParser::readUntil` 使用 `find_subslice`，并不调用本文件。

## 错误处理与边界

本文件没有 `Result`、错误类型、panic 分支或 I/O 错误传播。空集合始终不命中；空输入或完全不命中的输入返回 `-1`；若多个集合成员出现在输入中，只返回最左侧位置；字节 `0x00` 与 `0xFF` 和其他字节使用相同规则。

位索引不会越界：`u8 >> 5` 的范围是 0 到 7，`u8 & 31` 的范围是 0 到 31。重复设置同一个 bit 是幂等操作。该 API 不验证字符编码，也不识别多字节 token；它只筛选“可能是特殊 token 首字节”的位置。若调用者把多字节分隔符的所有字节都加入集合、或者命中后不验证完整 token，可能产生额外候选或错误解析，这属于调用方契约而不是本文件的错误处理。

当前没有 `bytes_test.rs` 或本文件内嵌测试。`pkg/lightning/mydump/csv_parser_test.rs` 测试的是未接入本模块的 Rust CSV 实现，不能算本 API 的直接回归证据。Go 的 `pkg/lightning/mydump/csv_parser_test.go` 通过实际解析路径间接覆盖字节集合的使用，例如转义符为正则元字符 `*`、引号内分隔符、多字节分隔符和 EOF 等场景，但它不能证明 Rust 实现自身已被执行。

## 并发与资源生命周期

文件不创建线程、任务、锁、原子、通道、事务、文件句柄或网络资源。构造阶段只在栈上建立定长值；扫描阶段只借用调用者持有的切片和集合，函数返回时没有待释放的外部资源或后台工作。

`ByteSet: Copy` 且内部只有 `[u32; 8]`，类型按字段性质可在线程间安全传递和共享；并发读取无需同步。由于内部字段私有且没有变更方法，正常公开 API 下不存在读写竞争。性能上构造复杂度为 O(`chars.len()`)，成员查询为 O(1)，扫描为 O(`bytes.len()`)，额外空间固定为 32 字节；Go 风格转发函数不引入分配。

## 与 Go 版本的对应关系

`pkg/lightning/mydump/bytes.go` 是逐项权威对照：Go `type byteSet [8]uint32` 对应 Rust `ByteSet([u32; 8])`；`makeByteSet`、`(*byteSet).contains`、`IndexAnyByte` 的桶选择、bit 选择、首命中返回和 `-1` 语义均保持一致。Rust 用 `&self` 替代 Go 指针接收者，因为查询不修改状态；用 `&[u8]`/`&ByteSet` 明确只读借用；用私有元组字段封装原始桶；同时额外提供 snake_case 入口以符合 Rust 命名习惯。

两侧的关键差异是接线状态，而不是位图算法：Go `csv_parser.go` 持有三个 `byteSet` 并在 `readUntil` 中调用 `IndexAnyByte`；Rust `CsvParser` 没有这些字段，且采用完整 token 比较/子切片搜索。另一个接口差异是 Go `IndexAnyByte` 返回平台宽度 `int`，Rust 返回 `isize`；两者都能表示 `-1`。Rust crate 根允许 `non_snake_case`，使兼容别名可以存在而不触发命名 lint。

## 扩展指南

若只调整集合表示或扫描算法，应保持三项可观察契约：覆盖全部 256 个字节、返回最左命中下标、未命中返回 `-1`。修改 `make_byte_set` 时必须同步检查 `contains` 的桶/位映射；修改规范 snake_case 入口时必须让 Go 风格别名继续单向转发，避免两套实现漂移。若要新增批量查询、迭代或 SIMD 扫描，先用基准证明确有收益，并保留短输入、空输入与极端字节的正确性。

若计划把该模块接入 Rust CSV 主链，最可能修改的位置是 `pkg/lightning/mydump/csv_parser.rs` 的 `CsvParser` 派生状态、`NewCSVParser` 初始化和 `read_field`/`readUntil` 扫描路径。必须继续在命中候选首字节后验证完整的多字节 separator/delimiter/terminator，不能把 byte-set 命中等同于完整 token 命中。兼容风险包括引号/转义优先级、跨 token 的多字节匹配、EOF、空行与行长度限制；性能风险包括每字节调用成本和短输入上优化开销。

测试必须放在独立文件，不能内嵌进 `bytes.rs`。建议新增同目录 `bytes_test.rs`，并在 `pkg/lightning/mydump/lib.rs` 以 `#[cfg(test)] #[path = "bytes_test.rs"] mod bytes_test;` 挂载；直接覆盖空集合、重复字节、0/31/32/255 桶边界、首命中、无命中和兼容别名等价性。若接入 CSV 主链，还应同步扩展 `pkg/lightning/mydump/csv_parser_test.rs`，并以 `pkg/lightning/mydump/csv_parser_test.go` 的正则元字符转义、多字节分隔符、quoted separator、EOF 等场景核对移植语义。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标文件已索引；`files --filter pkg/lightning/mydump/bytes.rs` 报告该文件 7 个符号；`node --file ... --offset 1 --limit 260` 返回完整 64 行源码。
- RustCodeGraph `query --json`：确认 `ByteSet` 位于第 27 行、`make_byte_set` 位于第 30 行、`index_any_byte` 位于第 52 行，并区分同名 Go/Rust 符号。对限定符运行 `callers/callees` 未得到可用边，因此调用结论由下述精确搜索补证。
- 源码：`pkg/lightning/mydump/bytes.rs`，完整读取并核对所有类型、函数、方法、派生项与注释。
- crate 边界：`pkg/lightning/mydump/Cargo.toml`、`pkg/lightning/mydump/lib.rs`、根 `Cargo.toml`；确认包名、Go 包映射、workspace 成员、模块声明和公开再导出。
- Rust 引用：仓库级 `rg` 搜索 `ByteSet|make_byte_set|makeByteSet|index_any_byte|IndexAnyByte` 只命中 `bytes.rs` 定义；模块搜索只命中 `lib.rs` 的 `mod bytes`/`pub use bytes::*`。因此“当前无 Rust 调用者”是当前工作树事实，不是对未来接线的推断。
- Go 对照与调用链：`pkg/lightning/mydump/bytes.go`、`pkg/lightning/mydump/csv_parser.go`；确认等价位运算，以及 `NewCSVParser -> makeByteSet`、`readUntil -> IndexAnyByte -> contains` 的实际使用。
- 测试证据：同目录没有同名 Rust 独立测试；读取 `pkg/lightning/mydump/csv_parser_test.rs` 与 `pkg/lightning/mydump/csv_parser_test.go`。前者只验证当前 Rust CSV 行为，不直接调用本模块；后者通过 Go CSV 主链间接覆盖特殊字节扫描。任务为纯文档分析，按计划不运行 Cargo 或代码测试。
