# `pkg/dxf/example/proto.rs`

## 文件定位

本文件属于 `astersql-dxf-example` crate 的任务元数据协议层。crate 根模块 `pkg/dxf/example/lib.rs` 以私有 `mod proto` 装载它，再通过 `pub use proto::*` 导出两个公开结构体及其公开编解码方法。`pkg/dxf/example/Cargo.toml` 将该 crate 映射到 Go 包 `pkg/dxf/example`，当前可用依赖只有 DXF `taskexecutor` 和日志 crate；本文件自身只使用 Rust 标准库，没有直接依赖框架类型或第三方 JSON 库。

该协议位于示例应用的调度端和执行端之间：`schedulerImpl::Init` 从框架任务的 `Task.Meta` 解出 `taskMeta`，`schedulerImpl::OnNextSubtasksBatch` 为每个步骤生成 `subtaskMeta`，而 `stepExecutor::RunSubtask` 从 `Subtask.Meta` 解回消息。它是教学用 DXF 示例的轻量 JSON 适配层，不是通用 JSON 库，也不是生产级 protobuf 实现（依据：`proto.rs` 文件注释、`scheduler.rs`、`task_executor.rs`）。

## 核心职责

- 定义任务级 `taskMeta { SubtaskCount }`，在任务提交者和调度器之间传递“每个 step 生成多少个子任务”。
- 定义子任务级 `subtaskMeta { Message }`，在调度器和 follower 节点的 step executor 之间传递可读消息。
- 用 `taskMeta::Marshal`、`subtaskMeta::Marshal` 生成稳定的小型 JSON 字节串。
- 用 `JsonParser`、`last_field` 和 `field_name_matches` 复现该示例所需的 Go `encoding/json` 解码语义，包括未知字段可被完整解析后忽略、字段名折叠匹配、重复字段最后一个生效、缺失或 `null` 字段保留零值，以及字符串中的无效 UTF-8/不成对代理项替换为 U+FFFD。
- 用 `escape` 对子任务消息编码，覆盖 JSON 控制字符，并按 Go 默认 HTML 安全转义规则处理 `<`、`>`、`&`、U+2028 和 U+2029。

## 主要符号

- `pub struct taskMeta`：公开任务元数据；`SubtaskCount: i64` 是每个业务步骤的子任务数。命名刻意保持 Go 迁移形态，crate 根模块为此允许非 Rust 惯用命名。
- `pub struct subtaskMeta`：公开子任务元数据；`Message: String` 是 executor 记录到日志的文本。
- `taskMeta::Marshal(&self) -> Vec<u8>` / `taskMeta::Unmarshal(&[u8]) -> Result<Self, String>`：编码固定键 `subtask_count`，或从 JSON 对象解出有符号 64 位整数。
- `subtaskMeta::Marshal(&self) -> Vec<u8>` / `subtaskMeta::Unmarshal(&[u8]) -> Result<Self, String>`：编码固定键 `message`，或从 JSON 对象解出字符串。
- `enum JsonValue`：私有 JSON 语法树，能表示 null、布尔、数字文本、字符串、数组和保序对象字段。数字保留为字符串，直到目标字段执行类型转换。
- `struct JsonParser<'a>`：私有单遍字节解析器；`input` 借用输入，`pos` 保存当前游标。核心方法是 `parse_object`、`parse_value`、对象/数组/数字/字符串解析及字节级游标辅助方法。
- `parse_json_object`：两个公开 `Unmarshal` 的共同入口，只接受顶层对象或顶层 `null`，并拒绝尾随非空白内容。
- `last_field` / `field_name_matches`：从后向前选取匹配字段；匹配采用 ASCII 不区分大小写，并把 Kelvin sign `K` 折叠为 `k`、long s `ſ` 折叠为 `s`，对应 Go `encoding/json` 的字段折叠行为。
- `escape`：私有 JSON 字符串转义器，只被 `subtaskMeta::Marshal` 调用。

## 执行流程

任务级流程如下：提交者构造 `taskMeta` 并调用 `Marshal`；`pkg/dxf/example/app_test.rs::test_example_application` 将结果放入 `Task.Meta`；`schedulerImpl::Init` 调用 `taskMeta::Unmarshal`，把 `SubtaskCount` 保存到调度器；之后 `OnNextSubtasksBatch` 按该数量循环，为 StepOne/StepTwo 构造 `subtaskMeta` 并调用其 `Marshal`。

子任务到达执行端后，`stepExecutor::RunSubtask` 调用 `subtaskMeta::Unmarshal`。成功时取得 `Message` 并随 subtask ID 写日志；失败时把解析字符串错误包装成 `ExecutorError` 返回，当前子任务不会被当作成功完成。

两个 `Unmarshal` 共用同一条解析路径：`parse_json_object` 创建 `JsonParser`；解析器递归消费一个 JSON 值并确认输入耗尽；`last_field` 反向查找目标键；缺失或 `null` 映射为字段零值；存在且类型正确时转换/克隆值；类型不匹配则报目标字段错误。未知字段仍必须是语法有效的 JSON，但其值在目标字段提取阶段被忽略。

## 数据与状态

`taskMeta` 和 `subtaskMeta` 都派生 `Clone`、`Debug`、`Default`、`Eq`、`PartialEq`。默认值分别是 `SubtaskCount == 0` 和空 `Message`，这也是字段缺失、字段为 `null` 或顶层为 `null` 时的结果。

解析期间唯一可变状态是一次调用私有 `JsonParser` 所持的 `pos`。对象以 `Vec<(String, JsonValue)>` 保存，而不是 map，因此字段顺序和重复字段均被保留，`last_field` 才能实现“最后一个匹配值获胜”。数组和未知字段的值会被递归构造，只为验证完整 JSON 语法；公开结果不会保留它们。

编码结果是新分配的 `Vec<u8>`。`taskMeta::Marshal` 通过格式化整数构造字节串；`subtaskMeta::Marshal` 先建立至少与原消息等容量的 `String`，再产生最终 JSON 字节串。源码没有缓存、全局变量或跨调用共享状态。

## 依赖与调用关系

直接上游调用点为：

- `pkg/dxf/example/scheduler.rs::schedulerImpl::Init` → `taskMeta::Unmarshal`。
- `pkg/dxf/example/scheduler.rs::schedulerImpl::OnNextSubtasksBatch` → 构造 `subtaskMeta` → `subtaskMeta::Marshal`。
- `pkg/dxf/example/task_executor.rs::stepExecutor::RunSubtask` → `subtaskMeta::Unmarshal`。
- `pkg/dxf/example/app_test.rs::test_example_application` → `taskMeta::Marshal`，并验证两个 meta 类型在两步调度/执行中的往返。
- `pkg/dxf/example/proto_test.rs` 直接覆盖字段折叠、重复键、无效字符串编码和顶层 `null`。

内部下游关系为：两个 `Unmarshal` → `parse_json_object` → `JsonParser::parse_object`/`parse_value`；字段选择再经过 `last_field` → `field_name_matches`。`subtaskMeta::Marshal` 还调用 `escape`。RustCodeGraph 的文件节点显示 `proto.rs` 被 `scheduler.rs`、`task_executor.rs`、`app_test.rs` 和 `proto_test.rs` 等文件使用；对重名 `taskMeta`/`subtaskMeta` 执行精确 `callers`/`callees` 未输出静态边，因此上述函数级关系以这些已索引调用现场为证，而不把空图结果解释为“没有调用”。

## 错误处理与边界

解析错误使用 `String`，不保留偏移或错误链。语法层可报告无效 JSON 值/字面量/数字/转义、未终止字符串、未转义控制字符、无效 Unicode 转义、非对象顶层值和尾随内容；字段层把不能解析为 `i64` 的数值统一报为 `invalid subtask_count`，把目标字段的类型不匹配报为 `invalid subtask_count` 或 `invalid message`。

`taskMeta::Unmarshal` 只接受 JSON 数字并要求最终适配 `i64`；浮点数、指数形式若不能被 Rust `i64::from_str` 接受也会失败。`subtaskMeta::Unmarshal` 只接受 JSON 字符串。布尔、数组、对象等值可以出现在未知字段中，但不能作为这两个目标字段的值。

字符串解析会拒绝未转义的 U+0000 到 U+001F 字节。普通字符串字节通过 `String::from_utf8_lossy` 转换，因此无效 UTF-8 被替换；Unicode 转义支持有效代理对，不成对的高/低代理项替换为 U+FFFD。编码端没有失败返回：Rust `String` 本身保证有效 UTF-8，`escape` 覆盖需要转义的字符。

该文件不验证业务约束，例如 `SubtaskCount` 是否非负。负数能完成协议编解码，随后 `schedulerImpl::OnNextSubtasksBatch` 通过断言模拟 Go `make` 对负长度的失败；该边界属于调度器而非协议层（由 `app_test.rs::scheduler_negative_subtask_count_matches_go_make` 覆盖）。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务、文件或网络资源。所有解析器和中间分配都局限在一次函数调用中，返回或报错时由 Rust 所有权机制释放；输入字节只在调用期间被 `JsonParser` 借用，结果结构体拥有自己的整数或字符串。

由于没有全局可变状态，公开方法可由不同线程并行调用；实际能否跨线程传递由字段类型自然决定，这两个结构体只包含 `i64` 或 `String`。本文件也不负责 DXF 任务/子任务的持久化和重试生命周期，那些职责在框架及 `scheduler.rs`、`task_executor.rs` 中。

## 与 Go 版本的对应关系

Go 对照文件 `pkg/dxf/example/proto.go` 只声明 `taskMeta.SubtaskCount int`（JSON 键 `subtask_count`）和 `subtaskMeta.Message string`（JSON 键 `message`）；实际编解码由 `encoding/json` 在 `app_test.go`、`scheduler.go` 和 `task_executor.go` 中完成。Rust 把这部分隐式库行为显式移植为两个 `Marshal`/`Unmarshal` 方法和私有解析器。

字段与主链语义保持一致：Go 测试提交 `SubtaskCount: 3`；Go scheduler 解码任务 meta、每步生成对应数量的消息；Go step executor 解码消息并记录日志。Rust `app_test.rs::test_example_application` 用已移植 scheduler/executor 和内存 `TaskTable` 跑通相同的两步、每步三个子任务，但其注释明确说明真实 scheduler/handle/storage/testkit crate 尚以 `cfg(any())` 依赖占位，因此这不是完整 Go 框架环境的等价集成测试。

需要注意类型差异：Go 字段是平台相关的 `int`，Rust 固定为 `i64`；在当前协议示例中，Rust 的有效范围由 `i64` 解析决定。可见性也不同：Go 两个结构体及字段都是包内符号，Rust 结构体和字段经 crate 根再导出为公开 API，这是 Rust crate 间调用和测试所需的接线差异。

## 扩展指南

新增 task 字段时，应同时修改 `taskMeta`、`taskMeta::Marshal` 和 `taskMeta::Unmarshal`，明确缺失/`null` 默认值、允许的 JSON 类型、重复键规则及数值范围；同步 `pkg/dxf/example/proto.go` 的标签语义，并在独立的 `pkg/dxf/example/proto_test.rs` 增加正常、错误类型、零值和重复字段用例。不要把 Rust 单元测试内嵌回 `proto.rs`。

新增 subtask 字段或按 step 使用不同 meta 时，应修改/新增对应结构和编解码入口，并同步 `schedulerImpl::OnNextSubtasksBatch` 的生成逻辑、`stepExecutor::RunSubtask` 的消费逻辑及 `app_test.rs` 的端到端断言。若继续复用本解析器，应先确认新增类型已被 `JsonValue` 表达且与 Go `encoding/json` 的目标类型转换一致。

若协议复杂度继续增长，优先评估采用 workspace 已批准的 JSON 实现，而不是继续扩张教学用解析器；替换时必须保留现有 Go 兼容测试所证明的字段折叠、重复键、null、无效 UTF-8、代理项和 HTML 安全转义语义。性能风险主要来自未知对象/数组的完整中间树分配和多次字符串分配；兼容风险主要来自 Go `encoding/json` 的边缘规则，而不是 DXF 框架接口。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、7032 个 Rust 文件；目标 `pkg/dxf/example/proto.rs` 已索引，共 361 行、35 个符号。
- 已通过 RustCodeGraph `node --file` 阅读：`pkg/dxf/example/proto.rs`、`proto_test.rs`、`lib.rs`、`scheduler.rs`、`task_executor.rs`、`app_test.rs` 以及包契约 `pkg/dxf/example/doc.go`。
- 已通过 RustCodeGraph 精确名称查询 `taskMeta`、`subtaskMeta`、`Unmarshal`，并运行 `callers`/`callees`；重名结构体查询未产生可用函数级边，因此又以已索引的直接调用现场核验调用关系。
- 已核对 crate/移植边界：`pkg/dxf/example/Cargo.toml`；已核对 Go 对照与调用现场：`proto.go`、`scheduler.go`、`task_executor.go`、`app_test.go`。
- 独立 Rust 测试证据：`proto_test.rs` 覆盖折叠字段名和最后值获胜、无效 UTF-8、不成对 UTF-16 代理项、顶层 null；`app_test.rs` 覆盖任务/子任务 meta 的主链往返、未知字段、字符串转义、缺失字段及负子任务数的下游边界。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务规定的命令确认目标文档存在且恰好包含上述 11 个固定二级标题，并人工复核未把尚未移植的完整 DXF 集成描述为当前已支持行为。
