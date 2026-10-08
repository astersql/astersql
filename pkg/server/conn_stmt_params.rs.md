# `pkg/server/conn_stmt_params.rs`

## 文件定位

本文件位于 `astersql-server` crate（见 `pkg/server/Cargo.toml`），由 `pkg/server/lib.rs` 以公开模块 `pub mod conn_stmt_params` 挂载。它把 MySQL `COM_STMT_EXECUTE` 参数区拆成逐参数的 `BinaryParam`：识别 null bitmap、两字节类型描述、定长或 length-encoded 值，并优先采用 `COM_STMT_SEND_LONG_DATA` 已绑定的字节。

需要区分“模块存在”和“生产主链已接线”：RustCodeGraph 对 `conn_stmt_params.rs::parseBinaryParams` 的 Rust 调用者只找到 `pkg/server/conn_stmt_params_test.rs` 中的测试；当前 Rust `COM_STMT_EXECUTE` 主链是 `pkg/server/conn_stmt.rs::handleStmtExecute -> ParseExecuteParams -> parse_params`，使用该文件内另一套 `BinaryParam` 与解析逻辑。因此本文件是可公开调用、经过独立单测的 Go 逻辑移植，但目前不是 Rust 服务端执行预处理语句时实际经过的解析器。代码图把 Go `pkg/server/conn_stmt.go::handleStmtExecute` 列入同名符号查询结果，属于跨语言同名关联，不能视为 Rust 调用边。

## 核心职责

1. 定义本解析器使用的 MySQL 字段类型码 `TYPE_*`，包括整数、浮点、时间、字符串、BLOB、DECIMAL、ENUM/SET、BIT 和 GEOMETRY。`TYPE_DECIMAL`、`TYPE_UNSPECIFIED` 都是数值 `0`，匹配时由同一个 length-encoded 文本分支处理。
2. 以 `BinaryParam { tp, is_unsigned, is_null, val }` 保存协议层解码结果；这里只保留原始或字符集转换后的字节，不把数值、时间进一步转换成 SQL Datum/表达式。
3. 在 `parseBinaryParams` 中落实参数来源优先级：bound long-data 高于 null bitmap；未绑定参数再按 null bitmap、类型和参数值游标解析。
4. 对文本型值调用 `InputDecoder::DecodeInput` 做连接字符集到 UTF-8 的转换；BLOB、数值、时间和 NEWDECIMAL 保持协议字节。未提供 decoder 时用 `NewInputDecoder("utf8")`，其实现会直通字节。
5. 将截断、长度溢出和未知类型分别收敛为 `ParamError::MalformedPacket` 与 `ParamError::UnknownFieldType(u8)`，避免越界切片和整数加法溢出。

## 主要符号

- `TYPE_DECIMAL` 至 `TYPE_GEOMETRY`、`TYPE_UNSPECIFIED`：公开的 `u8` 协议类型码。这里没有依赖 parser crate 的 MySQL 常量，而是在模块内定义本解析器所需集合。
- `ParamError`：公开错误枚举。`MalformedPacket` 表示结构、长度或边界不合法；`UnknownFieldType(tp)` 保留未知类型码。它实现 `Display` 和 `std::error::Error`，但没有映射到服务端协议错误号。
- `BinaryParam`：公开、可克隆且有默认值的解码结果。`tp` 是类型码，`is_unsigned` 来自类型描述第二字节的最高位，`is_null` 来自 bitmap、`TYPE_NULL` 或 length-encoded NULL，`val` 拥有值字节。
- `takeBinaryParamValue(param_values, pos, length)`：公开的边界安全切片函数。先把 `u64` 长度转换为 `usize`，再用 `checked_add` 和 `slice.get` 检查范围；成功时返回借用切片及下一游标。
- `parseBinaryParams(params, bound_params, null_bitmap, param_types, param_values, decoder)`：公开主入口，原地填充调用方预分配的 `params`。
- `parse_length_encoded_int(input)`：私有 length-encoded integer 解码器。首字节 `0..=250` 为直接长度，`251` 为 NULL，`252/253/254` 分别读取后续 2/3/8 个小端字节，`255` 或截断输入报畸形包。
- `read_le(input, length)`：私有小端读取辅助函数，从前缀后的 `length` 个字节组装 `u64`。

## 执行流程

`parseBinaryParams` 首先验证 `bound_params` 至少有一个槽位对应每个目标参数，且 `null_bitmap` 的位数足以覆盖全部参数；任一不足立即返回 `MalformedPacket`。随后创建 UTF-8 直通 decoder 作为默认值，并令共享的值区游标 `pos = 0`。

对每个参数按索引执行：

1. 若 `bound_params[index]` 为 `Some`，先按 BLOB 写入并克隆 long-data。若类型数组同时含完整的该参数类型描述，VARCHAR、VAR_STRING、STRING、BIT 会经 decoder 转码，四种 BLOB 类型保留字节和具体类型；其他类型维持兼容性默认 BLOB。该分支直接结束本参数处理，所以即使客户端同时设置 null bitmap，long-data 仍获胜。
2. 否则检查 `null_bitmap[index >> 3]` 的相应 bit；置位时写入 `TYPE_NULL`、`is_null = true` 的默认参数，不消费类型和值字节。
3. 非 bitmap NULL 必须存在两个类型字节。第一个字节是 `tp`，第二个字节最高位产生 `is_unsigned`。
4. 类型决定值长：TINY/SHORT/YEAR/INT24/LONG/FLOAT/LONGLONG/DOUBLE 使用固定 1/2/4/8 字节；DATE/TIMESTAMP/DATETIME/DURATION 先消费一个长度字节；NEWDECIMAL 和 BLOB 家族读取 length-encoded 长度但不转码；文本、ENUM、SET、GEOMETRY、BIT 读取 length-encoded 长度并设置转码标志；`TYPE_NULL` 长度为零；其他类型返回 `UnknownFieldType`。
5. `takeBinaryParamValue` 从当前 `pos` 取得恰好 `length` 个字节。文本分支经 decoder 生成新字节，其余分支复制原字节；之后以返回的 `next` 推进共享游标。

函数只要求按参数依次读够所需字节；完成后不会检查 `param_values` 是否还有尾随字节。

## 数据与状态

解析输入由五组互相关联的切片组成：`params.len()` 决定参数个数；`bound_params[index]` 表示是否已有 long-data；`null_bitmap` 每位描述一个参数；`param_types` 正常情况下每参数两个字节；`param_values` 是所有未绑定、非 bitmap NULL 参数串联的值区。`decoder` 是只读借用。

函数自身唯一的跨参数状态是局部游标 `pos`。bound long-data 和 bitmap NULL 都不推进它；日期/时间类型先推进一个长度前缀，length-encoded 类型先推进 1、3、4 或 9 字节前缀，然后所有普通值再推进数据长度。

输出 `BinaryParam::val` 总是拥有数据：bound long-data 使用 `clone`，普通二进制值使用 `to_vec`，文本转换也返回新 `Vec<u8>`。因此输出不借用网络包，调用结束后可独立保存。相反，`takeBinaryParamValue` 的返回值只在输入切片生命周期内有效。

失败不是事务性的：循环前已经写入的 `params` 槽位不会在后续参数报错时回滚。调用者应只在 `Ok(())` 后消费整组输出；重用输出缓冲区时不能依赖错误后的部分内容。

## 依赖与调用关系

- crate 边界：`pkg/server/Cargo.toml` 声明包名 `astersql-server`、库入口 `lib.rs`，并通过路径依赖 `astersql-server-internal-util` 提供 `InputDecoder` 和 `NewInputDecoder`。
- 模块入口：`pkg/server/lib.rs` 公开 `conn_stmt_params`，并在 `#[cfg(test)]` 下把 `pkg/server/conn_stmt_params_test.rs` 作为独立测试模块挂载，符合测试不内嵌源文件的仓库约定。
- 本文件内部调用边：`parseBinaryParams -> parse_length_encoded_int -> read_le`，以及 `parseBinaryParams -> takeBinaryParamValue`、`parseBinaryParams -> NewInputDecoder/InputDecoder::DecodeInput`。
- RustCodeGraph 的精确文件限定查询显示，Rust 测试函数调用 `parseBinaryParams`，`takes_binary_values_with_checked_bounds` 调用 `takeBinaryParamValue`；没有发现 Rust 生产调用者。
- 当前生产替代链：`pkg/server/conn_stmt.rs::handleStmtExecute -> ParseExecuteParams -> parse_params -> read_length_encoded_int`。这条链与本模块职责重叠，但类型、错误和字符集处理接口不同，扩展时不能假定修改本文件会改变线上 `COM_STMT_EXECUTE` 行为。
- Go 主链：`pkg/server/conn_stmt.go::handleStmtExecute -> pkg/server/conn_stmt_params.go::parseBinaryParams -> takeBinaryParamValue`；随后 Go 侧会把 `param.BinaryParam` 交给 `expression.ExecBinaryParam` 做语义类型转换。

## 错误处理与边界

以下情况返回 `MalformedPacket`：bound 槽位不足；null bitmap 位数不足；非 bitmap NULL 参数缺少完整的两字节类型；日期/时间缺长度字节；length-encoded 头缺失、使用保留前缀 `255` 或其后续 2/3/8 字节截断；`u64` 长度不能转换为当前平台 `usize`；`pos + length` 溢出；值区不足。

未知且不在匹配集合内的类型返回 `UnknownFieldType(tp)`。错误文本与 Go 的“stmt unknown field type”语义一致，但 Rust 枚举本身不包含 Go `dbterror.ClassServer` 的错误码/堆栈包装；若把本模块接入协议主链，需要在边界层显式映射。

`TYPE_NULL` 和 length-encoded 前缀 `251` 都能设置 `is_null`。length-encoded NULL 的长度为零，仍会经过安全切片并产出空 `val`。bitmap NULL 更早返回，并把类型规范化为 `TYPE_NULL`。long-data 又先于 bitmap，因此兼容 MariaDB 等会同时设置两者的客户端行为。

默认 UTF-8、utf8mb4、binary 等 decoder 直通原字节；显式 GBK/GB18030 等可能转换。`InputDecoder::DecodeInput` 在字符解码报告错误时退回原始字节而不是让参数解析失败，这是字符集边界的重要兼容语义。

## 并发与资源生命周期

本模块没有全局可变状态、锁、原子变量、线程、任务、通道或 I/O。所有解析状态都在函数栈或调用方提供的切片中；并发安全性取决于不同线程不同时可变访问同一 `params` 缓冲区，Rust 的借用规则会在安全代码中阻止这种别名。

默认 decoder 只活到一次 `parseBinaryParams` 调用结束，传入 decoder 仅被共享借用。每个成功参数会分配或克隆一个 `Vec<u8>`，大 long-data 与普通值因此有与参数大小成正比的额外内存成本；本文件不负责 `max_allowed_packet`、long-data 内存配额、网络缓冲区释放或语句执行后的绑定清理，这些生命周期应由上层 prepared-statement/connection 状态管理。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/server/conn_stmt_params.go`，主要结构高度一致：bound 参数优先；bitmap NULL 次之；两字节类型含 unsigned 标志；相同类型宽度表；日期/时间单字节长度；BLOB/NEWDECIMAL 与文本类的 length-encoded 长度；文本字符集转换；未知类型与畸形包分流。`takeBinaryParamValue` 的 Rust `usize::try_from + checked_add + get` 对应 Go 的“剩余长度先校验再切片”，并覆盖 Go 回归测试所关注的超大 length-encoded 长度不得 panic。

存在以下可见差异：

- Rust 入口额外预检 `bound_params.len()` 和 null bitmap 容量，避免 Go 版本依赖上层保证时可能出现的索引越界。
- Go 输出类型是共享 `pkg/param.BinaryParam`，错误使用 `mysql.ErrMalformPacket` 和带标准错误码的 `errUnknownFieldType`；本文件定义局部 `BinaryParam`/`ParamError`，尚无协议错误映射。
- Go 的普通参数值通常引用输入包切片；Rust 输出统一拥有 `Vec<u8>`。Rust 更易脱离输入生命周期保存，但会复制数据。
- Go length-encoded 解码复用 `internal/util.ParseLengthEncodedInt`；Rust 在本文件私有实现 `parse_length_encoded_int/read_le`。
- Go `pkg/server/conn_stmt_params_test.go` 的 `TestParseExecArgs` 继续调用 `expression.ExecBinaryParam`，覆盖整数符号解释、日期/时间和 duration 到 SQL 值的转换及警告；Rust 独立测试只验证本文件的协议层输出，不证明下游 SQL 类型转换。
- 当前 Rust 生产链使用 `pkg/server/conn_stmt.rs::parse_params` 而不是本文件。两者对 long-data 类型修正、字符集转换、错误类型和数据结构并非同一实现，后续对齐应先决定单一真实入口，避免两套解析器继续漂移。

## 扩展指南

新增或调整协议类型时，首先修改 `TYPE_*` 与 `parseBinaryParams` 的类型分组，明确该类型是固定宽度、单字节长度前缀还是 length-encoded，并明确是否应经连接字符集 decoder。同步扩展 `pkg/server/conn_stmt_params_test.rs`，至少覆盖正常值、截断前缀/值、unsigned、NULL 与字符集语义；测试必须继续放在独立文件，不能嵌入生产源文件。

修改 long-data 时要保持“bound 优先于 bitmap”的兼容不变量，并分别测试类型数组完整和缺失的情况。引入零拷贝或减少克隆前，应评估 `BinaryParam` 生命周期是否会越过输入包，并考虑当前调用方持有输出的需求。修改错误时，应保持 `MalformedPacket` 与未知类型的可区分性，并在真正接入服务端时补充到 MySQL 错误码的显式转换。

若目标是改变实际 Rust `COM_STMT_EXECUTE` 行为，仅修改本文件不足：必须同时审查 `pkg/server/conn_stmt.rs::{ParseExecuteParams, parse_params, read_length_encoded_int}` 及其独立测试。更稳妥的演进方向是让生产链复用一个解析实现和一个 `BinaryParam` 定义，但这属于跨文件重构，不是本说明任务的范围；在实施前需要验证 long-data 清理、旧类型数组复用、游标关闭和字符集来源等上层契约。

性能风险主要来自每参数 `Vec` 分配/复制和 bound long-data 克隆；兼容风险集中在类型码分组、NULL 优先级、decoder 适用类型及错误映射。任何接线变更都应增加一条从 `COM_STMT_EXECUTE` 数据包到执行参数的集成回归，而不能只依赖本文件单测。

## 验证依据

- 源码与装配：`pkg/server/conn_stmt_params.rs`、`pkg/server/lib.rs`、`pkg/server/Cargo.toml`。
- Rust 直接测试：`pkg/server/conn_stmt_params_test.rs`，覆盖安全切片、定宽数值与 unsigned、bitmap/类型 NULL、字符串/BLOB/NEWDECIMAL、bound long-data、UTF-8/GBK decoder、截断包和未知类型。
- 当前 Rust 生产入口：`pkg/server/conn_stmt.rs` 中 `handleStmtExecute`、`ParseExecuteParams`、`parse_params`、`read_length_encoded_int`。
- Go 对照与调用者：`pkg/server/conn_stmt_params.go`、`pkg/server/conn_stmt.go::handleStmtExecute`。
- Go 测试：`pkg/server/conn_stmt_params_test.go` 中 `TestParseExecArgs`、`TestParseExecArgsMalformedLengthEncodedParam`、`TestParseExecArgsAndEncode`，用于核对类型转换边界、超大/截断长度不得 panic 以及 GBK 转换。
- decoder 实现：`pkg/server/internal/util/util.rs::{InputDecoder, NewInputDecoder, DecodeInput}`，用于确认直通字符集、GBK/GB18030/latin1 映射和解码失败回退原字节。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点；限定 `--file pkg/server/conn_stmt_params.rs` 的 `query/node/callers/callees` 验证了公开符号、内部调用边和仅测试侧 Rust callers；对 `pkg/server/conn_stmt.rs` 的查询验证当前生产链走 `ParseExecuteParams -> parse_params`。跨语言同名 caller 结果已与源码区分，未作为 Rust 接线证据。
- 本任务是纯文档分析，按计划不运行 Cargo。交付时以固定十一章节结构检查、路径/符号人工复核和 Git diff 自审作为验证。
