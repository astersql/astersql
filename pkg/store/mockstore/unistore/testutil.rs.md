# `pkg/store/mockstore/unistore/testutil.rs`

## 文件定位

本文件属于 `astersql-store-mockstore-unistore` crate。crate 入口 `pkg/store/mockstore/unistore/lib.rs` 以 `pub mod testutil` 声明该模块，并以 `pub use testutil::*` 将其公开项提升到 crate 根；`pkg/store/mockstore/unistore/Cargo.toml` 则把 `lib.rs` 指定为库入口，并用 `package.metadata.porting.go-package` 表明该 crate 对应 Go 包 `pkg/store/mockstore/unistore`。

它是 UniStore mock RPC 的 TopSQL 资源标签诊断辅助层：面对本 crate 自己的 `crate::rpc::Request`，找出可代表目标表的起始键，识别 TiDB record/index 表键中的 `table_id`，并在 TopSQL 已启用、标签为空且表 ID 为正时生成诊断错误。它不处理请求、不写存储，也不负责生成资源标签。

当前接线必须与能力本身区分：Rust 的 `lib.rs` 会编译和再导出本模块，但仓库精确引用搜索只找到 `pkg/store/mockstore/unistore/testutil_test.rs` 调用 snake_case API；Rust `rpc.rs` 尚未在 RPC 分发链中调用本文件。相对地，Go 对照实现由 `pkg/store/mockstore/unistore/rpc.go` 的 `checkResourceTagForTopSQL` 调用点接入。因此，当前 Rust 代码提供的是可调用的校验能力和一项最小回归测试，不能描述为已经覆盖 Rust UniStore 的每次 RPC。

## 核心职责

1. `get_request_start_key` 按 `Request` 变体选取可代表请求目标的键。单键请求直接借用键切片；批量请求取第一项；`Prewrite`/`Flush` 取第一条 mutation；Raw、MPP、元数据和当前明确跳过的事务命令返回 `None`。
2. `decode_table_id` 只识别 `t` + 8 字节 comparable int64 + `_r`/`_i` 的表键前缀。它翻转编码整数首字节的最高位，再按大端有符号整数还原 `table_id`。
3. `check_resource_tag_for_top_sql` 组合开关、标签、取键和解码结果。只有 TopSQL 开启、标签为空、请求有可检查键、键能解出正表 ID 时才返回 `ResourceTagError`。
4. `get_stack` 在真正报错时强制捕获当前 Rust 调用栈，以便定位漏设标签的调用来源。
5. `checkResourceTagForTopSQL`、`getReqStartKey`、`getStack` 是保留 Go 命名习惯的兼容别名，分别委托给 snake_case 实现。

## 主要符号

- `ResourceTagError(pub String)`：公开的新类型错误。它派生 `Clone`、`Debug`、`Eq`、`PartialEq`，便于比较和测试；`Display` 原样输出内部字符串，`std::error::Error` 实现允许作为标准 Rust 错误使用。元组字段公开，因此调用者也能直接读取或构造消息。
- `check_resource_tag_for_top_sql(request: &Request, resource_group_tag: &[u8], top_sql_enabled: bool) -> Result<(), ResourceTagError>`：主要校验入口。开关和标签由调用方显式传入，本模块没有全局 TopSQL 状态。
- `get_request_start_key(request: &Request) -> Result<Option<&[u8]>, ResourceTagError>`：公开取键入口。返回切片的生命周期来自传入的 `Request`，没有复制键数据。当前所有匹配分支都构造 `Ok(...)`，错误类型是为统一接口或后续扩展保留的；当前实现本身不会产生 `Err`。
- `decode_table_id(key: &[u8]) -> Option<i64>`：私有表键解码器。长度不足、首字节不是 `t`、第 9..11 字节不是 `_r` 或 `_i` 时返回 `None`。
- `get_stack() -> String`：通过 `std::backtrace::Backtrace::force_capture` 返回格式化栈文本，即使通常的 backtrace 环境配置没有启用也主动捕获。
- `checkResourceTagForTopSQL`、`getReqStartKey`、`getStack`：公开兼容门面；前两个不改变底层结果，`getStack` 额外把 `String` 转成 UTF-8 `Vec<u8>`，贴近 Go `[]byte` 返回形态。

文件没有模块级常量、trait、业务状态结构、`unsafe`、条件编译项或异步函数。

## 执行流程

`check_resource_tag_for_top_sql` 的流程如下：

1. 若 `top_sql_enabled == false`，或 `resource_group_tag` 非空，立即返回 `Ok(())`；此路径不解析请求，也不捕获回溯。
2. 调用 `get_request_start_key`。有键的请求借用对应字段；空 mutations/keys 通过 `.first()` 得到 `None`，不会发生 Go 对照实现中 `[0]` 式的越界；无适用键的命令也返回 `None`。
3. 没有起始键时直接返回 `Ok(())`。
4. 调用 `decode_table_id`。不能识别的键用 `unwrap_or_default()` 视为表 ID `0`，即忽略标签检查而不是把畸形键升级为错误。
5. 仅当 `table_id > 0` 时构造 `ResourceTagError`。消息包含 `request.command_type()` 的调试表示、表 ID 和 `get_stack()` 的回溯文本；随后立即返回 `Err`。
6. 非正表 ID返回 `Ok(())`。

取键映射的关键分组是：`Get`/`Cleanup` 取 `key`；`Scan`/`ScanLock` 取 `start`；`Prewrite`/`Flush` 取第一条 mutation 的 key；`Commit`、`BatchGet`、`BatchRollback`、`CheckSecondaryLocks`、`BufferBatchGet` 取第一把 key；`PessimisticLock` 取 `primary_lock`；`Cop`/`CopStream` 取已经存入 Rust `Request` 的 `start_key`。其余当前 `Request` 变体显式映射为 `None`，所以新增变体会受穷尽 `match` 的编译约束，必须在这里作出检查或跳过决定。

## 数据与状态

本文件自身无持久状态或全局可变状态。输入均为不可变借用：`Request` 决定候选键，`resource_group_tag` 只通过是否为空参与判定，`top_sql_enabled` 是调用方提供的即时布尔值。返回的起始键切片直接指向 `Request` 内部的 `Vec<u8>` 或嵌套请求字段，不能超过原请求生命周期。

表键解码依赖固定字节布局：偏移 0 是 `t`，偏移 1..9 是 comparable int64，偏移 9..11 是 `_r` 或 `_i`。解码只读取前 11 字节，后续 record handle 或 index 内容不参与判断。最高位异或 `0x80` 是 TiDB comparable signed-int 编码的逆变换；还原后只接受正数作为需校验的表 ID。

错误状态只存在于一次调用的 `ResourceTagError(String)` 中。回溯字符串可能较大，但仅在最终确认“正表 ID 且标签缺失”后创建。

## 依赖与调用关系

上游边界：

- `pkg/store/mockstore/unistore/lib.rs` 声明并再导出本模块；同文件用 `#[path = "testutil_test.rs"] mod testutil_test` 挂接独立测试。
- RustCodeGraph 文件关系显示 `testutil.rs` 被 `testutil_test.rs` 使用；对 `check_resource_tag_for_top_sql`、`get_request_start_key`、`decode_table_id` 的精确查询均定位到本文件。
- 当前仓库的 Rust 精确引用只有 `testutil_test.rs` 调用两个 snake_case 函数；`rpc.rs` 没有调用边。若未来需要与 Go 一样在请求发送/分发前强制校验，应在 RPC 边界显式接线并提供开关与标签来源，而不能假设 `pub use` 会自动执行检查。

下游依赖：

- `crate::rpc::Request` 及其 `command_type()` 提供请求数据和错误消息中的命令类型；`Request` 定义在 `pkg/store/mockstore/unistore/rpc.rs`。
- `std::backtrace::Backtrace` 提供诊断栈；格式化和标准错误 trait 也仅来自标准库。
- 主函数内部调用关系是 `check_resource_tag_for_top_sql -> get_request_start_key`、`check_resource_tag_for_top_sql -> decode_table_id`，错误分支再调用 `get_stack`；三个 Go 风格别名分别单向委托对应实现。

Cargo 边界方面，本文件没有直接使用 `Cargo.toml` 中列出的 `astersql-util`、config/server/tikv 子 crate或 `fail`；它依赖同 crate 的 `rpc` 模块和标准库。`testutil_test.rs` 使用的 `Request` 同样来自本 crate，未引入额外 dev dependency。

## 错误处理与边界

- TopSQL 关闭或标签非空属于明确的成功短路，不检查键格式。
- 无键、空 mutation 列表、空 key 列表、跳过类别均返回 `Ok(())`。Rust 用 `.first()` 避免了空集合索引 panic，这是相对 Go 对照实现的重要安全差异。
- 非 TiDB 表键、少于 11 字节的键、后缀不是 `_r`/`_i` 的键，以及解码得到 `table_id <= 0` 的键，都被忽略。这里的策略是诊断性、保守放行，避免标签检查破坏 mock RPC。
- `decode_table_id` 是局部、手工的前缀解码，不等同于 Go `tablecodec.DecodeRecordKey`/`DecodeIndexKey` 的完整校验，也没有显式处理额外 keyspace 前缀。扩展键格式时必须用真实 codec 契约和测试验证，不能只放宽长度或偏移。
- `get_request_start_key` 的返回类型允许 `ResourceTagError`，但当前穷尽分支不返回错误；不同于 Go `default` 分支会报告未知请求，Rust 新增 enum 变体会先导致此 `match` 编译不通过，从而强制维护者选择语义。
- 错误消息是诊断接口的一部分，当前包含命令类型、表 ID 与完整栈。若日志或断言依赖该文本，修改格式可能带来兼容影响；回溯也可能暴露内部调用路径，应只在测试/诊断边界使用。

## 并发与资源生命周期

所有函数都只读访问输入且不保存引用到调用结束之后，没有锁、原子变量、线程、任务、通道、事务或 I/O；因此本文件不引入共享状态竞争。多个线程可各自调用这些函数，实际可共享性由 `Request` 内部字段的 trait 决定，但函数本身没有可变别名。

唯一明显的临时资源是 `Backtrace::force_capture()` 创建的回溯及其格式化 `String`；`getStack` 还会把字符串转为新分配的字节向量。这些对象都由返回值或错误值拥有，并按 Rust 所有权正常释放。由于捕获和格式化回溯可能昂贵，保持它只在缺标签错误路径执行是重要的性能不变量。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/store/mockstore/unistore/testutil.go`：

- Go `checkResourceTagForTopSQL` 自行读取进程级 `topsqlstate.TopSQLEnabled()` 和 `req.GetResourceGroupTag()`；Rust 主函数把 `top_sql_enabled` 与 `resource_group_tag` 作为参数。这降低了本模块对全局状态和 client-go 请求类型的耦合，但要求未来调用点正确传值。
- Go 通过 `tablecodec.IsRecordKey`/`IsIndexKey` 和对应 Decode 函数取得表 ID；Rust `decode_table_id` 只实现当前固定前缀的最小解码。因此两者对扩展编码、带前缀键或畸形键的接受范围不应未经测试就视为完全等价。
- Go 的 `getReqStartKey` 对多项字段直接访问 `[0]`，Rust 对 mutations/keys 使用 `.first()`，空集合时返回 `None`；Rust `Cop`/`CopStream` 直接读取其简化 `Request` 中的 `start_key`，Go 从第一个 range 读取 start。
- Go 对若干类别分组返回 `nil, nil`，并在未知命令的 `default` 返回错误；Rust `Request` 是封闭 enum，所有现有变体显式处理，新增变体由编译器迫使更新匹配。
- Go `getStack` 使用固定 64 KiB 缓冲区与 `runtime.Stack(..., false)` 捕获当前 goroutine；Rust 使用 `Backtrace::force_capture()`，返回的格式和截断特性不同。Rust 的 `getStack` 别名再把字符串转换成字节以接近 Go 签名。
- Go 生产接线位于 `pkg/store/mockstore/unistore/rpc.go`：仅当 `CheckResourceTagForTopSQLInGoTest` 为真时，在请求分发前调用校验；多个 `pkg/server/**/main_test.go` 会启用该测试开关。Rust `rpc.rs` 当前没有等价开关或调用点，因此迁移状态是“核心辅助逻辑已存在、生产/测试主链接线尚未对齐”。

## 扩展指南

- 新增或修改 `crate::rpc::Request` 变体时，先在 `get_request_start_key` 明确它应取哪个代表键或为何跳过；若是批量字段，继续使用 `.first()` 保持空集合无 panic。同步更新 `pkg/store/mockstore/unistore/testutil_test.rs`，逐类覆盖有值与空集合。
- 支持新的 TiDB 键格式时，优先复用仓库已有 codec，或至少为前缀、正负/零/边界 table ID、record/index 后缀、短键、畸形键和 keyspace 前缀增加独立测试，再修改 `decode_table_id`。手工偏移变更具有静默漏检或误报风险。
- 若把检查接入 Rust RPC 主链，应修改最靠近实际发送/分发边界的符号，明确 TopSQL 开关及 resource tag 的真实来源，并验证检查发生在存储副作用之前。还应增加类似 Go 测试开关的隔离机制，避免诊断策略意外改变普通 mockstore 测试。
- 扩展错误信息时保持 `ResourceTagError` 的标准错误语义，避免在成功或不可识别键路径捕获回溯；若错误文本被外部断言，需同步兼容性测试。
- 兼容别名只是迁移门面。新 Rust 调用优先使用 snake_case API；删除别名前必须先通过全仓引用搜索确认没有下游依赖。
- 测试必须继续放在独立的 `pkg/store/mockstore/unistore/testutil_test.rs`，不要把测试模块内嵌回生产文件。当前测试只覆盖 `Request::Empty`；安全扩展至少应补齐成功短路、合法 record/index 键缺标签时报错、已有标签放行、非表键放行和各个集合为空的情形。

## 验证依据

- 源码与符号：RustCodeGraph `node --file pkg/store/mockstore/unistore/testutil.rs --offset 1 --limit 500` 读取了完整 143 行；`query` 唯一定位 `check_resource_tag_for_top_sql`、`get_request_start_key`，并在多项同名结果中定位本文件的 `decode_table_id`。
- 调用图：RustCodeGraph `callees check_resource_tag_for_top_sql` 确认其调用 `get_request_start_key`、`decode_table_id` 并构造 `ResourceTagError`；`callees get_request_start_key` 与 `callees decode_table_id` 辅助确认集合首项和长度检查。图的 callers 查询为空，因此又以精确 `rg` 核对全仓引用，确认 Rust 侧只有独立测试直接使用，未把缺失调用边臆测成已接线。
- crate 与模块：读取 `pkg/store/mockstore/unistore/Cargo.toml`、`pkg/store/mockstore/unistore/lib.rs` 和 `pkg/store/mockstore/unistore/rpc.rs`，核对 crate 名、Go 包映射、模块再导出、测试挂接、`Request` 全部变体及 `command_type()`。
- Go 对照：读取 `pkg/store/mockstore/unistore/testutil.go` 和 `pkg/store/mockstore/unistore/rpc.go` 的调用段，并搜索 `CheckResourceTagForTopSQLInGoTest` 的启用位置，核对全局开关、codec、请求映射、空集合行为、回溯实现和 Go 侧真实接线。
- 独立测试：读取 `pkg/store/mockstore/unistore/testutil_test.rs`；现有唯一测试 `empty_request_is_ignored_like_go_cmd_empty` 证明 `Request::Empty` 同时在取键层返回 `Ok(None)`、在主校验层返回 `Ok(())`。它不证明合法表键报错、标签短路或所有变体映射，本文已明确该验证边界。
- 本任务是纯文档分析；按计划不运行 Cargo。交付前另运行任务指定的 11 章节结构检查，并人工复核唯一新增产物、无源码或总计划修改。
