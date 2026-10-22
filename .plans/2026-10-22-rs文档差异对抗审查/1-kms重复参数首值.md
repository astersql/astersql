# 任务 1: KMS 重复 query 参数取首值

批次：【批次 1】 依赖：无

状态：未开始

目的：对同一 AWS KMS URL 中重复的 REGION 参数，令 Rust 与 Go 一样选择首值。

来源任务：无；候选来自 `inventory.tsv` 中相应 `.rs.md`，Go 源码为最终基线。

预计会话范围：仅一项输入/输出合同，最多一个直接生产路径及相邻测试；若发现更大缺口，按 Go commit 对齐边界把无关基础设施列为范围外建议，不递归补建。

## 文件

- 修改：`br/pkg/task/encryption.rs`
- 测试：`br/pkg/task/encryption_test.rs`
- 文档：`br/pkg/task/encryption.rs.md`

## 上下文

Go `br/pkg/task/encryption.go:82-85` 用 `url.Values.Get` 取首值；Rust `encryption.rs:102-105` 先收集 HashMap，后值覆盖前值。只处理参数重复语义；检查 AWS、Azure、GCP 三个解析器共享的 query 收集，测试用 AWS 的 REGION 反例。生产修改依据为上述 Go 差异。

## Cargo 与验证入口

- 工作目录：仓库根目录；crate：`astersql-br-pkg-task`；过滤条件：见下方命令；features：默认。
- Cargo 工作区：根 `Cargo.toml`；本任务所在 crate 的 `Cargo.toml` 已存在。Rust 测试逻辑保留在独立测试文件。
- 若本任务只修正失实文档且不调用 Cargo，槽位规则不适用；只做 `rg`、`git diff --check` 与文档证据审查。

## Cargo 共享槽位规则（仅 Rust/Cargo 任务）

1. 同一仓库的所有计划和任务共用仓库根目录下的 `target/rust-slot-1` 至 `target/rust-slot-10`，不得为任务、会话或计划另建构建目录，也不得按任务编号取模固定分配。
2. 每次构建/验证阶段开始前，创建仓库根目录下的 `target/rust-slot-locks`；依次用原子命令 `mkdir target/rust-slot-locks/slot-N.lock` 尝试领取槽位，`N` 为 1–10。仅 `mkdir` 成功者获得该槽位；这些相对路径命令从仓库根目录执行。
3. 领取成功后，在自己的锁目录中记录计划、任务、会话标识和持有进程 PID。同一槽位同时只允许一个持有者；已占用时尝试下一个，仅全部 10 个槽位都占用时等待并重试。
4. 将 `CARGO_TARGET_DIR` 设置为领取槽位的绝对路径，例如领取槽位 3 后，从仓库根目录执行 `export CARGO_TARGET_DIR="$PWD/target/rust-slot-3"`。示例编号不代表固定分配。
5. 所有 Cargo 构建、检查、测试以及调用 Cargo 的脚本和子进程均继承该设置，不得通过 `--target-dir` 或其他配置绕过槽位；生成的二进制也从该槽位定位。
6. 该阶段的 Cargo、脚本以及使用槽位产物的子进程全部结束后，删除自己写入的占用记录，并用 `rmdir` 释放自己的锁目录；保留槽位编译缓存供后续任务复用。
7. 正常结束、失败和中断均须清理自己的锁；在后台进程或其他使用槽位产物的进程仍运行时不得提前释放。规划清理步骤时覆盖这三种退出路径。
8. 遗留锁须先核实持有会话及相关进程已结束；无法确认则保持占用，不得删除其他活跃任务的锁、进程或缓存。
9. 生成或更新计划时，统一移除与这些规则冲突的单一 Rust 共享构建空间、逐任务独立目录和 Cargo 缓存全局独占窗口要求；仍须遵守源码文件占用、真实业务依赖和其他共享验证资源的互斥，不能仅因缓存分槽就把任务判定为可并行。
10. 最终验证记录包含实际槽位、绝对路径 `CARGO_TARGET_DIR`、确切命令、退出码和有效测试数量；Rust 代码修改后先运行 `cargo fmt --all`，再运行相应验证。编译成功或零测试不能替代行为验收证据。

## 测试计划

- 行为：`REGION=us-east-1&REGION=bad` 的解析结果保留首个 region。
- 失败验证测试：在 `br/pkg/task/encryption_test.rs` 扩展 `test_parse_aws_kms_config` 的重复 REGION 案例。
- 失败验证命令：`cargo test -p astersql-br-pkg-task --lib test_parse_aws_kms_config`
- 预期失败原因：现有 HashMap `collect` 保留最后一个值，断言首值失败。
- 通过验证命令：`cargo test -p astersql-br-pkg-task --lib test_parse_aws_kms_config`
- 模拟策略：优先复用本 crate 既有内存存储或 SQL mock，只模拟外部对象存储/数据库边界；输入和副作用仍经生产入口。验证同一参数大小写仍按原有精确键匹配；不打印 URL 或密钥。

## 步骤

1. 阅读本文件、`plan.md`、对应 `.rs.md`、Go/Rust 源码及同目录独立测试；用 RustCodeGraph 核对生产调用路径。执行前确认当前代码仍存在该差异。
2. 先在独立 Rust 测试文件中加入本任务的可观察反例。
3. 运行失败验证命令并记录预期失败及有效测试数；若未失败，检查反例是否真正触及生产路径。
4. 使用 URL 现有解析器的有序 query pairs，按第一次出现写入；保持缺参数及凭据成对验证的原顺序。随后只做通过该测试所需的最小生产修改；保留 PingCAP 注释，修复后顶部有 `// Copyright 2026 AsterSQL.`，同步 `.rs.md`。
5. 先运行 `cargo fmt --all`，再运行通过验证命令及 `make lint`；检查 `git diff --check` 和变更范围。

## 验证

- 运行：`cargo fmt --all`；`cargo test -p astersql-br-pkg-task --lib test_parse_aws_kms_config`；`make lint`；`git diff --check`。
- 预期：新增回归在修复前因指定行为失败，修复后至少 1 个匹配测试通过且退出码为 0；Ready 适用检查通过。
- 所需证据：Go 对照行、Rust 生产调用行、失败及通过输出、测试数量、准确槽位与 `CARGO_TARGET_DIR`、命令退出码、diff 自审。若 Go 既有测试失败，记录为既有回归。若 `make lint` 或环境不可用，按 `AGENTS.md` 和执行技能记录未验证项及状态。

## 完成

只在上述单一行为和必要局部接线得到证据后标记 `已完成`。如当前文档叙述已失实而代码行为已对齐，更新文档并以 Go/Rust 源码与现有行为测试为证。若有实际代码、测试、配置或文档修改，用 `$git-commit` 仅提交本任务变更；只有任务文件状态变更则不提交。完成后删除编号任务文件；阻塞时仅在本文件写明原因、检查和状态，不修改只读 `plan.md`。
