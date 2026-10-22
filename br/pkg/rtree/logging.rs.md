# `br/pkg/rtree/logging.rs`

## 文件定位

提供 `ZapRanges`，把一组 `KeyRange` 转为键名固定为 `"ranges"` 的 `Field`。

## 核心职责

本文件只有两项职责：

1. 为 `crate::rtree::KeyRange` 实现 `std::fmt::Display`，把范围稳定地表示为半开区间 `[start, end)`，且两端的原始字节都先经过十六进制转换，避免直接输出键的明文字节。
2. 提供 `ZapRanges`，把一组 `KeyRange` 转为键名固定为 `"ranges"` 的 `Field`。它复用 `astersql-br-pkg-logutil::AbbreviatedStringers`：0–3 个区间全部记录，4 个及以上只记录首项、`(skip N)` 和末项。

这一层的价值是统一可观测性格式并限制大范围列表的日志体积；它不验证范围是否有序、重叠或合法。

## 主要符号

- `fn redact_key(key: &[u8]) -> String`：文件内私有辅助函数。它逐字节使用两位小写十六进制格式 `format!("{b:02x}")` 并拼接为 `String`。空切片得到空字符串；非 UTF-8 键也可无损表示。它与 `crate::stubs::redact_key` 是不同符号，在本文件内只服务于 `KeyRange::fmt`。
- `impl std::fmt::Display for KeyRange`：对外可见的 trait 实现。`fmt` 写出 `[{}, {})`，起点和终点分别来自 `redact_key(&self.StartKey)` 和 `redact_key(&self.EndKey)`。右侧圆括号明确表示终点不属于范围。
- `pub fn ZapRanges(ranges: &[KeyRange]) -> Field`：本文件惟一的命名公开 API。它接受借用切片，先 `to_vec()` 克隆整组 `KeyRange`，然后调用 `AbbreviatedStringers("ranges", ...)`。返回的 `Field` 可由日志编码器输出为 `{"ranges": [...]}`。

文件中没有常量、自定义类型、trait 声明、异步函数或条件编译项。

## 执行流程

`ZapRanges` 的完整路径如下：

1. 调用者传入 `&[KeyRange]`；函数不修改该切片。
2. `ranges.to_vec()` 克隆每个 `KeyRange` 及其内部的起止 `Vec<u8>`，形成拥有所有权的列表。
3. `AbbreviatedStringers` 根据长度分支：长度小于 4 时对每项调用 `Display::to_string`；长度大于等于 4 时只对第一项和最后一项调用 `to_string`，中间生成 `(skip len-2)`。
4. `KeyRange::fmt` 分别将 `StartKey` 和 `EndKey` 十六进制化，得到例如 `[30, 31)` 的字符串。
5. logutil 将字符串包装成 `Field::array("ranges", ...)`；本文件不直接写日志，实际编码发生在调用者将 `Field` 交给日志系统之后。

`Display` 也可被独立调用：任何通用格式化 API 对 `KeyRange` 请求 `Display` 时，都直接执行第 4 步，不经过 `ZapRanges`。

## 数据与状态

`KeyRange` 定义在 `br/pkg/rtree/rtree.rs`，其 `StartKey` 与 `EndKey` 都是 `Vec<u8>`，表示 `[StartKey, EndKey)`。本文件只读取这两个字段，不改变原范围、不缓存格式化结果，也不持有跨调用的状态。

`ZapRanges` 返回的 `Field` 拥有已格式化的字符串数组，不再借用输入切片。空输入保留键名并产生空数组，而不是省略字段。长列表的中间项不进入结果，但 `skip` 计数精确等于总数减 2。

## 依赖与调用关系

- 上游组装：`br/pkg/rtree/lib.rs` 引入 `logging.rs` 并再导出其公开 API；`logging_test.rs` 也从 crate 根导入 `KeyRange` 和 `ZapRanges`。
- 数据依赖：`crate::rtree::KeyRange` 提供起止键；`Display` 实现让它满足 `AbbreviatedStringers<T: fmt::Display>` 的类型约束。
- 日志依赖：`astersql_br_pkg_logutil::{AbbreviatedStringers, Field}` 负责列表缩写和字段容器；对应实现位于 `br/pkg/logutil/logging.rs` 的 `AbbreviatedStringers`。
- RustCodeGraph 对 `br/pkg/rtree/logging.rs` 报告 4 个符号，并没有找到非测试 Rust 文件中对 `ZapRanges` 的直接调用。`rg` 同样只找到定义、`lib.rs` 的模块接线和 `logging_test.rs` 的测试调用。因此当前 Rust 产品树中该 API 已暴露但尚未被其他产品模块直接接线；不应将 Go 侧的存量调用者自动当作 Rust 调用者。

## 错误处理与边界

所有函数都是无 `Result`/无 `Option` 返回的确定性格式化路径，没有显式错误分支。`fmt` 将底层 formatter 的 `std::fmt::Result` 原样返回；通常对 `String`/`Field` 编码时不会触发格式写入失败。

需要保持的边界包括：

- 0 个范围输出 `{"ranges": []}`。
- 3 个范围仍全量输出；4 个范围已进入缩写分支，输出首项、`(skip 2)` 和末项。该阈值是 `AbbreviatedStringers` 的契约，不是同文件中 `AbbreviatedArrayMarshaler` 的“长度小于等于 4 时全量”规则。
- 空起点或终点被格式化为空文本，例如两端均空时为 `[, )`；本层不将空终点解释为“正无穷”或额外标记。
- 任意字节都会转换为小写十六进制，因此不依赖 UTF-8 合法性。这是稳定编码，但是否满足运行时全局的动态脱敏策略取决于上层契约；本文件未读取 `NeedRedact` 之类的全局开关。
- `ZapRanges` 的 `to_vec()` 会先克隆全部输入，即使最终日志只保留首尾；大列表会带来与键总字节数成正比的临时分配和拷贝。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件或网络资源。`redact_key` 只在单次调用内分配一个 `String`；`Display::fmt` 在格式化期间借用 `KeyRange`；`ZapRanges` 借用输入，然后通过克隆生成与输入生命期解耦的 `Field`。

因为不使用全局可变状态，并发调用之间没有本文件引入的竞态。能否跨线程移动返回的 `Field` 由 `astersql-br-pkg-logutil::Field` 的 trait 实现决定，本 API 未额外声明或绕过这些约束。

## 与 Go 版本的对应关系

- Go `(KeyRange).String()` 通过 `fmt.Sprintf("[%s, %s)", redact.Key(...), redact.Key(...))` 实现；Rust 用 `Display::fmt` 提供同样的半开区间文本。Rust 当前的本地 `redact_key` 固定做小写 hex，而不是直接调用 Go `pkg/util/redact.Key`；两侧现有金标测试对 ASCII 键的结果一致。
- Go `ZapRanges([]KeyRange)` 返回 `logutil.AbbreviatedStringers("ranges", ranges)`；Rust `ZapRanges(&[KeyRange])` 返回自己 logutil crate 的同名抽象。差异是 Rust 为满足所有权参数而显式克隆切片。
- `br/pkg/rtree/logging_test.go::TestLogRanges` 与 `br/pkg/rtree/logging_test.rs::test_log_ranges` 共享同一组 0、1、2、3、4、5、6、1024 长度及完全相同的 JSON 期望值。Rust 测试使用十进制字符串的字节构造键，对齐 Go `fmt.Appendf(nil, "%d", j)`。

## 扩展指南

- 如果要修改单个范围的文本形式，修改 `Display for KeyRange::fmt` 或其私有辅助函数，并同步更新独立的 `br/pkg/rtree/logging_test.rs`。不要把测试内嵌到 `logging.rs`。
- 如果要改变键名或列表缩写策略，优先确认 `br/pkg/logutil/logging.rs::AbbreviatedStringers` 的通用契约；修改共享辅助函数会影响其他 crate，而在 `ZapRanges` 内特判则会使 rtree 与 Go 通用 logutil 语义分叉。
- 任何可见格式变更都应与 `br/pkg/rtree/logging.go` 和 `br/pkg/rtree/logging_test.go` 对照，保留双语言日志的键名、空格、括号、十六进制大小写与 `skip` 计数兼容性。
- 若优化大切片的克隆开销，需要调整 `AbbreviatedStringers` 的入参/所有权设计或增加借用版 API，而不是在不保留 `Field` 所有权安全的情况下简单删除 `to_vec()`。性能修改应补充大列表测量，同时保留 1024 项金标断言。
- 若要对齐更完整的脱敏策略，必须先核对 Go `redact.Key` 在各 redact 模式下的契约，再决定复用 `astersql-br-pkg-logutil` 中的现有脱敏类型还是扩展共享 API；应增加非 UTF-8、空键和各脱敏模式的回归测试。
