# 任务 34: TTL 删除通知对抗审查

批次：【批次 7】 依赖：无；同批任务仅读取源码，各自只删除自己的编号任务文件。

状态：未开始

目的：核实一条 Go/Rust 差异断言，不修复代码，给后续单行为修复留下可复查的证据。

来源任务：`inventory.tsv` 的 `pkg/ttl/client/notification.rs.md` 行。

预计会话范围：一个 `.rs.md`、一对同路径 Go/Rust 源文件与最近的测试和调用者；只判断一条断言，不审完整子系统。

## 文件

- 阅读：`pkg/ttl/client/notification.rs.md`
- 阅读：`pkg/ttl/client/notification.go`、`pkg/ttl/client/notification.rs`
- 参考：同目录最近的 `*_test.go`、`*_test.rs`，以实际文件为准。

## 待反驳断言

Go etcd watch 暴露 Delete；Rust 转换层忽略 Delete，核实生产消费者是否依赖它。

这是一条文档候选，不是已经证明的缺陷。把语言表示差异、生产可达性差异和真实输出差异分开报告。

## 上下文与边界

- 从仓库根目录执行命令。代码索引存在；优先使用 RustCodeGraph 定位 `Delete` 的定义与调用者，文档/测试发现可用 `rg`。
- 若源码或文档已变，先核对当前版本。核对同目录 `doc.go`（存在时）和 `AGENTS.md`，不得把 `.rs.md` 单独当权威。
- 此任务是只读审查，不运行 Cargo、不修改生产代码、测试、文档或其他任务文件。若需要动态反例才能断言成立，明确写出最小失败测试的输入/预期与目标 crate，作为后续任务建议。

## 验证计划

- 基线存在：`test -f pkg/ttl/client/notification.rs.md && test -f pkg/ttl/client/notification.go && test -f pkg/ttl/client/notification.rs`。
- 代码入口：`~/.rustcodegraph/bin/rustcodegraph node --file pkg/ttl/client/notification.go --offset 1 --limit 240`；`~/.rustcodegraph/bin/rustcodegraph node --file pkg/ttl/client/notification.rs --offset 1 --limit 240`。
- 符号定位：`rg -n -F -- 'Delete' pkg/ttl/client/notification.go pkg/ttl/client/notification.rs`。若一侧无同名符号，阅读文档中的实际对应关系，并记录缺失证明所用的搜索范围。
- 测试盘点：`rg --files pkg/ttl/client -g '*test.go' -g '*test.rs'`，只阅读与断言相邻的用例。
- 生产可达性：在找到确切符号后用 `rustcodegraph callers <symbol>` 或索引的 `node --file --offset` 核对调用路径；重名时在报告中写明语言和文件，不能把测试调用当生产接线。
- 测试驱动开发不适用：本任务不改变行为，验收是可引用的源码和测试证据，不以编译成功充数。

## 步骤

1. 读 `plan.md`、本任务、文档 Go 对照段落及两端源码；确认候选断言的确切输入和输出。
2. 尝试找出反例：检查另一条分支、调用者或现有测试能否推翻文档叙述。
3. 用 Go/Rust 源码行、测试行和生产调用链给出结论：`已证实`、`已反驳`、`仅接口/实现形态差异` 或 `证据不足`。
4. 在最终回复中给出最小复现输入、两端预期/实际或静态可推导结果、验证命令与退出码、缺少的运行证据。若需要后续编码，建议单一行为的失败测试和目标文件，不在本任务中编码。
5. 证据充分时把本文件标为 `已完成` 并删除；证据不足但无法完成核实时在本文件标为 `已阻塞` 并记录具体阻碍。不要修改 `plan.md`。

## 完成

本任务唯一交付是最终回复中的对抗审查证据；不产生代码、测试或文档变更，因此不调用 `$git-commit`，也不要求 Cargo 共享槽位。只读审查无需 WIP/Ready/Heavy 构建配置。必须说明未运行 Cargo/Go 测试的限制。
