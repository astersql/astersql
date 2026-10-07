# `pkg/ingestor/globalsort/kvgroup.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-ingestor-globalsort`，由同目录的 [`lib.rs`](./lib.rs) 通过 `pub mod kvgroup;` 暴露为 `astersql_ingestor_globalsort::kvgroup`。它定义全局排序产物的 KV 分组命名协议：表行数据使用保留名 `"data"`，索引数据使用索引 ID 的十进制文本。该协议让调用方能够以字符串作为对象存储目录、元数据映射键或冲突处理参数，同时保留索引 ID 的往返转换能力。

[`Cargo.toml`](./Cargo.toml) 将本目录声明为独立 library crate，并用 `package.metadata.porting.go-package = "pkg/ingestor/globalsort"` 标明对应的 Go 包。当前文件自身只依赖 Rust 标准库；crate 的对象存储、SST、worker pool 等依赖并未进入这里的转换逻辑。

## 核心职责

- `DataKVGroup` 给表数据组提供唯一的固定名称 `"data"`。
- `IndexID2KVGroup` 把有符号 64 位索引 ID 格式化为十进制字符串，作为索引 KV 组名。
- `KVGroup2IndexID` 执行反向解析，并把格式非法或超出 `i64` 范围的输入交给标准库以 `ParseIntError` 报告。

文件不负责创建分组、写入排序文件、验证索引是否存在，也不负责区分唯一索引与普通索引；它只是命名协议的无状态转换层。`"data"` 不是合法索引 ID，因此应先按 `DataKVGroup` 分流，再对索引组调用反向解析。

## 主要符号

- `pub const DataKVGroup: &str = "data"`：进程静态字符串切片，不分配内存。命名沿用 Go API；crate 根的 `#![allow(non_upper_case_globals)]` 允许这种非常量式命名。
- `pub fn IndexID2KVGroup(indexID: i64) -> String`：调用 `i64::to_string()`，为每次调用分配一个拥有所有权的十进制字符串。负值会保留负号，零得到 `"0"`，没有前导零。
- `pub fn KVGroup2IndexID(kvGroup: &str) -> Result<i64, ParseIntError>`：借助 `str::parse::<i64>()` 解析十进制有符号整数。返回标准库错误而非本 crate 的 `Error`/`Result`，因此调用方可以保留原始解析错误类型。

本文件没有私有函数、类型、trait、宏、条件编译项或内嵌测试。

## 执行流程

写入或构造元数据时，调用方应先判断 KV 来源：表行数据直接使用 `DataKVGroup`；索引数据把 index ID 传给 `IndexID2KVGroup`，获得可用作映射键或路径片段的字符串。Go 生产路径 `pkg/dxf/importinto/planner.go` 的 `getSortedKVMetas` 正是把数据元数据放在 `"data"` 键下，并把每个索引元数据放在其十进制 ID 键下。

消费索引分组时，调用方先排除 `DataKVGroup`，再把其余分组名传给 `KVGroup2IndexID`。成功后得到 `i64` 索引 ID，失败则必须停止依赖该 ID 的后续处理。Go 对照路径 `pkg/dxf/importinto/conflictedkv/handler.go` 的 `IndexKVHandler.PreRun` 会解析组名、查找目标索引，并在解析失败或索引不存在时返回错误。

Rust 当前接线范围较窄：仓库搜索未找到目标文件以外的 Rust 生产代码直接调用这三个符号；`IndexID2KVGroup` 已被冲突收集、删除和处理器的 Rust 独立测试用来构造索引组名。因而上述完整生产流程是 Go 端已验证行为，不能据此宣称 Rust 生产链已完成迁移。

## 数据与状态

模块唯一的静态数据是不可变的 `&'static str` 常量 `DataKVGroup`。两个函数都是纯函数：不读取全局配置，不修改输入，不保存缓存，也不接触文件、网络或数据库。

命名空间的不变量是“数据组使用非数字保留名，索引组使用可解析为 `i64` 的十进制文本”。正向函数产生的任意结果都能被反向函数解析回原值，即 `KVGroup2IndexID(&IndexID2KVGroup(id)) == Ok(id)`。反向函数本身不会拒绝负数、正号或具有合法十进制语法的其他 `i64` 文本；索引 ID 的业务合法性应由上层元数据检查负责。

## 依赖与调用关系

下游依赖仅为标准库：`IndexID2KVGroup -> i64::to_string`，`KVGroup2IndexID -> str::parse::<i64>`。这两个调用未在 RustCodeGraph 中形成可展示的外部调用边，但可由函数体直接核验。

上游模块入口是 [`lib.rs`](./lib.rs) 的 `pub mod kvgroup`。RustCodeGraph 对 `IndexID2KVGroup`、`KVGroup2IndexID` 和 `DataKVGroup` 的精确 callers 查询没有返回 Rust 生产调用者；`rg` 复核也只发现测试代码调用 `IndexID2KVGroup`。直接测试使用点包括：

- `pkg/dxf/importinto/conflict_resolution_test.rs`：把索引 `2` 的冲突文件放入组名映射。
- `pkg/dxf/importinto/conflictedkv/collector_test.rs`：用索引 `1`、`7` 的组名覆盖索引冲突收集路径。
- `pkg/dxf/importinto/conflictedkv/deleter_test.rs`：用索引 `2` 的组名覆盖索引冲突删除路径。
- `pkg/dxf/importinto/conflictedkv/handler_test.rs`：把目标 index ID 转为 handler 的组名。

`pkg/dxf/importinto/Cargo.toml` 与 `pkg/dxf/importinto/conflictedkv/Cargo.toml` 均声明了对 `astersql-ingestor-globalsort` 的路径依赖，与上述测试调用关系一致。

## 错误处理与边界

正向转换不返回错误；对全部 `i64` 值（包括 `i64::MIN`、`i64::MAX`、零和负数）都能生成字符串。其唯一隐含资源风险是极小的字符串分配失败，Rust 标准分配器通常以进程级失败处理，函数签名不暴露该错误。

反向转换会在空串、`"data"`、含非数字字符、带空白、或数值超出 `i64` 范围时返回 `ParseIntError`。它不添加上下文、不记录日志，也不会把错误转换为 crate 的 `Error`；上层如需指出具体 KV 组或任务，应在传播时补充上下文。不要把任意非 `"data"` 字符串都假设为有效索引组，必须检查解析结果，并在需要时继续验证索引是否存在。

当前独立 Rust 测试通过真实调用间接覆盖若干正向转换结果，但没有针对正反往返、极值或非法字符串的本文件专属测试。Go 同目录也没有 `kvgroup_test.go`；边界结论来自实现及标准解析契约，而非现有专属回归用例。

## 并发与资源生命周期

模块没有锁、原子变量、通道、异步任务、事务或可变静态状态。所有输入均为按值 `i64` 或调用期间借用的 `&str`；返回的 `String` 由调用方拥有，`ParseIntError` 也不借用输入。因此函数可被多线程并发调用，不需要初始化或关闭阶段。

生命周期方面，`DataKVGroup` 存活整个进程；`KVGroup2IndexID` 只在调用栈内借用 `kvGroup`，返回后不保留引用。性能成本主要是正向转换的一次小字符串分配，反向转换则不分配持久结果。

## 与 Go 版本的对应关系

直接对照文件是 [`kvgroup.go`](./kvgroup.go)。三项 API 一一对应：Go `DataKVGroup` 与 Rust 常量值相同；Go `fmt.Sprintf("%d", indexID)` 与 Rust `i64::to_string()` 都输出有符号十进制；Go `strconv.ParseInt(kvGroup, 10, 64)` 与 Rust `parse::<i64>()` 都返回 64 位有符号整数或解析错误。

接口差异在所有权和错误类型：Go 接受拥有值语义的 `string` 并返回 `(int64, error)`；Rust 借用 `&str`，返回 `Result<i64, std::num::ParseIntError>`。Rust 使用 Go 风格的公开符号与参数名，依靠 crate 根的 lint allow 保持迁移 API 对齐。

Go 的实际调用面比 Rust 更完整：`pkg/dxf/importinto/planner.go` 使用常量和正向转换组织全局排序元数据，`pkg/dxf/importinto/conflictedkv/handler.go` 使用反向转换准备索引处理器；Rust 当前仅在相关独立测试中直接使用正向转换，反向转换尚无仓库内 Rust 调用者。扩展 Rust 生产链时应以这些 Go 调用点为语义基线，但需逐项核验 Rust 上层类型和错误传播，不能只复制调用形态。

## 扩展指南

若新增分组类别，应先决定它是新的保留非数字名称还是索引 ID 的变体，并确保不会与现有 `"data"` 或合法十进制索引名冲突。修改协议会影响持久化对象路径和任务元数据，必须检查旧产物兼容性；不要仅在解析函数中静默接受新格式。

若把本模块接入 Rust 生产流程，最可能修改的是创建排序元数据映射的规划代码和索引冲突 handler 的预运行逻辑：数据分支使用 `DataKVGroup`，索引分支使用 `IndexID2KVGroup`，消费索引组时显式处理 `KVGroup2IndexID` 错误并验证索引存在。应同步新增独立测试文件，而不是把测试写进 `kvgroup.rs`；至少覆盖 `i64` 极值往返、零、负数、`"data"`、空串、空白、非数字和溢出输入，并保留冲突处理的端到端分流用例。

兼容风险主要是组名格式变化导致旧对象或元数据无法发现；正确性风险是把数据组误当索引组，或解析成功后未验证 index ID；性能风险较低，但高频路径若重复格式化同一 ID，可在调用层复用已拥有的组名，避免改变本模块的无状态职责。

## 验证依据

- RustCodeGraph `status` 确认仓库存在可用索引；`node --file pkg/ingestor/globalsort/kvgroup.rs --offset 1 --limit 500` 返回完整 32 行源码，并列出冲突处理测试使用关系。
- RustCodeGraph `query IndexID2KVGroup` 同时定位 Go 与 Rust 定义；对 Rust 文件限定的 `callers`/`callees` 查询未返回生产调用边。随后用 `rg` 搜索 Rust 生产文件与测试文件，确认生产侧没有直接引用，测试侧有上述四类直接使用点。
- 已核对源码 [`kvgroup.rs`](./kvgroup.rs)、crate 清单 [`Cargo.toml`](./Cargo.toml)、模块入口 [`lib.rs`](./lib.rs) 与 Go 对照 [`kvgroup.go`](./kvgroup.go)。目标目录没有 `doc.go`，也没有同名 Rust 或 Go 专属测试文件。
- 已阅读直接证据位置：`pkg/dxf/importinto/conflict_resolution_test.rs`、`pkg/dxf/importinto/conflictedkv/collector_test.rs`、`deleter_test.rs`、`handler_test.rs`，以及 Go 生产调用点 `pkg/dxf/importinto/planner.go`、`pkg/dxf/importinto/conflictedkv/handler.go`。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验证应确认本文恰好包含任务规定的十一个二级标题；行为说明通过源码、调用搜索、Cargo 边界和 Go 对照交叉复核。
