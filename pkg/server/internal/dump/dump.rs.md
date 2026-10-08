# `pkg/server/internal/dump/dump.rs`

## 文件定位

本文件是 `astersql-server-internal-dump` crate 的协议编码实现，提供 MySQL 线协议所需的基础字节拼接原语。crate 入口 [`lib.rs`](./lib.rs) 通过 `#[path = "dump.rs"] mod dump_impl` 装入本文件，并用 `pub use dump_impl::*` 导出全部公开函数；[`Cargo.toml`](./Cargo.toml) 将该 crate 定义为工作区库，运行时只直接依赖 `time = 0.3.44` 和本仓库的 `astersql-types-time`。

它位于 server 的底层编码边界，不负责网络发送、包头、行遍历或字符集转换。直接的 Rust 生产使用方是 [`../column/column.rs`](../column/column.rs)：该模块先决定列/行的协议布局和字段类型，再调用这里的函数写入单个整数、长度编码值或时间值。`../column/Cargo.toml` 以依赖名 `dump` 引用本 crate。

## 核心职责

- `LengthEncodedInt`、`LengthEncodedString` 实现 MySQL 长度编码整数及其上的字符串表示。
- `Uint16`、`Uint32`、`Uint64` 将定宽无符号整数按小端序追加到已有缓冲区。
- `BinaryTime` 将 `time::Duration` 编为 MySQL 二进制 `TIME` 载荷，保留符号、天、时、分、秒和可选微秒。
- `BinaryDateTime` 根据 `types::Time` 的 MySQL 类型及内容，选择零值、仅日期、精确到秒或带微秒的二进制时间形态。

所有函数只做确定性的内存编码，接受或创建 `Vec<u8>` 并返回编码后的字节；它们不解析输入、不校验上层字段语义，也不生成完整 MySQL packet。

## 主要符号

- `pub fn LengthEncodedString(buffer: Vec<u8>, bytes: &[u8]) -> Vec<u8>`：先调用 `LengthEncodedInt` 写入 `bytes.len()`，再原样追加内容。长度来自 Rust 切片长度并转换为 `u64`。
- `pub fn LengthEncodedInt(buffer: Vec<u8>, n: u64) -> Vec<u8>`：`0..=250` 直接写一字节；`251..=0xffff` 写 `0xfc` 加两字节；`0x10000..=0xffffff` 写 `0xfd` 加三字节；其余写 `0xfe` 加八字节。多字节值均为小端序。
- `pub fn Uint16/Uint32/Uint64(buffer, n) -> Vec<u8>`：分别通过 `to_le_bytes()` 追加 2、4、8 字节，不添加类型或长度标记。
- `pub fn BinaryTime(dur: time::Duration) -> Vec<u8>`：零值返回 `[0]`；非零值返回长度为 9 或 13 的新缓冲区。
- `pub fn BinaryDateTime(data: Vec<u8>, t: types::Time) -> Vec<u8>`：在调用者提供的缓冲区后追加时间载荷；支持 `TypeTimestamp`、`TypeDatetime` 和 `TypeDate`。
- 文件级 `#![allow(non_snake_case)]`：保留与 Go 导出 API 相同的 PascalCase 名称，降低逐调用点迁移差异。

本文件没有结构体、枚举、trait、impl、模块级可变状态或条件编译项。

## 执行流程

长度编码路径如下：调用者把已有 `Vec<u8>` 交给 `LengthEncodedString`；函数先按内容长度选择 1/3/4/9 字节的 `LengthEncodedInt` 形态，再追加内容。`../column/column.rs` 的列定义编码、文本行编码及字符串类二进制列编码都走这条路径。

定宽整数路径直接把数值的小端字节追加到原缓冲区。列定义中的字符集、列长度和标志使用 `Uint16`/`Uint32`；二进制行中的短整型、长整型、无符号长整型以及浮点数位模式使用相应的 `Uint16`/`Uint32`/`Uint64`。

`BinaryTime` 的步骤是：

1. 零 duration 立即返回单字节 `0`。
2. 将纳秒总数收窄到 Go `time.Duration` 对应的 `i64`，记录负号；负值用 `wrapping_neg` 取绝对量，从而对齐 `i64::MIN` 的 Go 回绕行为。
3. 依次扣除整天、小时、分钟和秒，初始构造 13 字节布局：长度 `12`、符号、四字节天字段、时分秒、四字节微秒。实现写入天字段最低字节 `data[2]`，其余三字节保持初始化的零值，与当前 Go 的 `byte(days)` 行为一致。
4. 若余数为零，把长度改为 `8` 并截断到 9 字节；否则将剩余纳秒除以 1000，按小端序写入微秒字段。亚微秒余数因此被截断。

`BinaryDateTime` 先读取年月日，再按 `t.Type()` 分支。DATETIME/TIMESTAMP 的优先级是零值（长度 0）、微秒非零（长度 11）、任一时分秒非零（长度 7）、仅日期（长度 4）；DATE 只有零值与长度 4 两种。年份通过 `Uint16` 写入，微秒通过 `Uint32` 写入。

## 数据与状态

主要状态是调用链中转移所有权的 `Vec<u8>`。除 `BinaryTime` 总是新建结果外，其余函数接收已有缓冲区并在尾部追加，因此前缀必须保持不变；`migration_aster_unit_test.rs` 明确验证了带前缀的字符串、整数和日期时间追加行为。

长度字节描述的是其后时间载荷的字节数，不包含长度字节自身：零时间为 `0`，DATE 为 `4`，含时分秒为 `7`，含微秒为 `11`；TIME 的非零长度为 `8` 或 `12`。所有协议整数采用小端序。

本文件没有全局缓存、计数器、锁、事务或持久化状态。输入 `types::Time` 按值传入；编码过程只调用其只读访问器 `Year`、`Month`、`Day`、`Hour`、`Minute`、`Second`、`Microsecond`、`Type` 和 `IsZero`。

## 依赖与调用关系

文件内调用边为：`LengthEncodedString -> LengthEncodedInt`；`BinaryDateTime -> Uint16`，且带微秒分支还有 `BinaryDateTime -> Uint32`。`BinaryTime` 仅依赖 `time::Duration` 的 `is_zero` 与 `whole_nanoseconds`，定宽函数依赖标准整数的 `to_le_bytes`。

上游生产链由 [`../column/column.rs`](../column/column.rs) 给出：

- `Info::dump` 用长度编码字符串及定宽整数组装 `ColumnDefinition41`。
- `DumpTextRow` 用 `LengthEncodedString` 写入经字符集编码后的非 NULL 文本值。
- `DumpBinaryRow` 按列类型调用定宽整数函数、`BinaryDateTime`、`BinaryTime` 或 `LengthEncodedString`，再由更上层 server 路径发送完整结果行。

Cargo 边界证据是本 crate 的 `Cargo.toml` 和 `../column/Cargo.toml`；工作区根 `Cargo.toml` 还将其列为成员并提供 `facade_server_internal_dump` 工作区依赖别名。RustCodeGraph 对目标文件识别出 8 个节点（文件加 7 个函数），并确认上述文件内调用边；针对部分公开函数的 callers 结果存在跨语言误配或空结果，因此生产调用点以精确 Rust 文本引用和 Cargo 依赖交叉核验。

## 错误处理与边界

这些 API 不返回 `Result`，也没有显式错误分支；调用者必须保证输入类型和值已符合协议语义。`Vec` 扩容失败只会沿用 Rust 分配失败行为，不在本层恢复。

关键边界包括：

- 长度编码整数的 `251` 不是单字节值，而是 `0xfc fb 00`；三个切换上界是 `250`、`0xffff`、`0xffffff`。
- `LengthEncodedInt` 只编码非 NULL 整数。MySQL 长度编码中的 `0xfb` NULL 标记由上层（如 `column.rs`）处理，不能把本函数的数值 `251` 输出误当作 NULL。
- `BinaryTime` 的天数只保留低 8 位，这是与当前 Go 源码一致的兼容行为；修改为完整四字节天数会改变现有跨语言结果，应先明确兼容目标并补充测试。
- `BinaryTime` 将纳秒精度截断到微秒；负一纳秒仍会携带负号，但微秒字段为零并保留 13 字节形态。
- `i64::MIN` 使用回绕取负，结果仍为负的位模式；补充测试锁定了当前 Go 兼容输出。
- `BinaryDateTime` 对三种已支持类型之外的 `t.Type()` 走 `_ => {}`，原样返回缓冲区而不报错。上层的字段类型分派负责避免错误类型进入此函数。
- 年月日、时分秒和微秒在本层通过整数转换写出，不做范围校验；合法性依赖 `types::Time` 的构造/解析层。

## 并发与资源生命周期

全部函数无共享可变状态，可由不同线程并发调用；线程安全来自每次调用独占自己的 `Vec<u8>` 和按值/共享只读输入，而不是锁或原子操作。

缓冲区所有权在函数间移动并最终返还调用者，已有容量通常可以复用；扩容时可能重新分配。`BinaryTime` 不接收外部缓冲区，因此会独立分配 1、9 或 13 字节结果；`DumpBinaryRow` 随后用 `extend_from_slice` 将它复制进结果缓冲区。这里没有后台任务、通道、文件句柄、网络连接、事务或需要显式关闭的资源。

## 与 Go 版本的对应关系

直接对照文件是 [`dump.go`](./dump.go)，七个 Rust 公开函数与 Go 同名函数一一对应，分支顺序和字节布局保持一致。主要语言映射是 Go `append`/`binary.LittleEndian` 对应 Rust `Vec::push`、`extend_from_slice` 与 `to_le_bytes`；Go `time.Duration` 对应 `time::Duration`，但 Rust 显式收窄到 `i64` 并使用 `wrapping_neg` 复现 Go 最小整数回绕。

[`dump_test.rs`](./dump_test.rs) 移植了 Go [`dump_test.go`](./dump_test.go) 的三组测试：零值及日期时间形态、四类长度编码整数、`Uint64` 小端往返。[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 是 AsterSQL 的额外独立测试，补齐全部长度切换点、已有缓冲区追加、三种定宽整数、TIME 正负/跨天/微秒/`i64::MIN` 和 DATETIME 各长度形态。

当前可见差异主要是 Rust `BinaryTime` 接受范围更宽的 `time::Duration` 类型后主动收窄为 `i64`，以 Go 语义为准；另外 Rust 对未知 `types::Time` 类型显式使用空匹配臂，而 Go `switch` 自然落空，外部行为相同。

## 扩展指南

若增加新的长度编码或定宽原语，应在本文件新增最小纯函数，并在独立测试文件 `migration_aster_unit_test.rs`（或与 Go 新测试对齐时的 `dump_test.rs`）覆盖每个字节边界、非空前缀和小端序；不要把测试内嵌进 `dump.rs`。若 API 需要用于结果行，再在 `../column/column.rs` 的相应类型分支接线，并同步该目录的独立测试。

修改时间编码时应优先保持 `dump.go` 的真实分支和字节级输出，尤其是长度标记、负号、天字段、纳秒到微秒截断、零日期和未知类型原样返回。任何修正 Go 既有兼容行为的设计都可能影响 MySQL 客户端协议兼容，必须先用明确的协议依据决定是否同时修改 Go，并增加跨边界回归用例。

性能上应继续允许调用者传入并复用 `Vec`；在热路径增加中间字符串或临时缓冲区会放大逐行编码成本。若要消除 `BinaryTime` 在 `DumpBinaryRow` 中的临时分配，可考虑新增“追加到已有缓冲区”的 API，但应保留或有计划地迁移现有签名，并用等价测试证明所有字节形态不变。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、目标目录 6 个已索引源码/测试文件；`node --file pkg/server/internal/dump/dump.rs --offset 1 --limit 260` 读取了完整 165 行实现；`query` 核对 `LengthEncodedString`、`LengthEncodedInt`、`BinaryTime`、`BinaryDateTime` 的 Rust/Go 定义；`node LengthEncodedInt` 确认 `LengthEncodedString -> LengthEncodedInt` 调用边；`callers` 查询用于检查上游，但其跨语言结果不稳定，故未单独作为生产接线结论。
- 已读生产与装配文件：`pkg/server/internal/dump/dump.rs`、`pkg/server/internal/dump/lib.rs`、`pkg/server/internal/dump/Cargo.toml`、`pkg/server/internal/column/column.rs`、`pkg/server/internal/column/Cargo.toml`，以及工作区根 `Cargo.toml` 的成员/依赖声明。目标目录不存在 `doc.go`。
- 已读 Go 对照与测试：`pkg/server/internal/dump/dump.go`、`dump_test.go`；已读独立 Rust 测试：`dump_test.rs`、`migration_aster_unit_test.rs`。
- 精确引用搜索确认 Rust 生产调用集中在 `pkg/server/internal/column/column.rs`，并核对了列定义、文本行和二进制行三条使用路径。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验收使用任务指定命令，要求目标文件存在且恰有 11 个固定二级标题。
