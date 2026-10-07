# 任务 9: 清理 parquet 不可达模式警告

批次：【批次 1】 无

状态：已完成，待回归验证

目的：在保持 PhysicalType 显式穷尽检查的前提下消除 testutils 的 unreachable_patterns 警告。

来源任务：无

预计会话范围：单一诊断、单一模块和一组聚焦验证；不扩展到相邻功能重构。

## 文件

- 修改：`pkg/dumpformat/testutils/parquet_writer.rs`
- 测试：`pkg/dumpformat/testutils` 相邻测试

## 上下文

- `PhysicalType` 当前八个变体已全部逐项匹配，尾部 `unsupported` 分支没有可达值；应依靠枚举穷尽性让未来新增变体在编译期失败。

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

- 行为：所有当前物理类型仍映射到正确值类型，crate 在 deny-warnings 下通过。
- 失败验证测试：先运行下述精确目标并保存当前诊断。
- 失败验证命令：`RUSTFLAGS='-D warnings' cargo check -p astersql-dumpformat-testutils --all-targets --locked`
- 预期失败原因：`unsupported` 分支触发 unreachable_patterns。
- 通过验证命令：`cargo test -p astersql-dumpformat-testutils --locked -- --nocapture && RUSTFLAGS='-D warnings' cargo check -p astersql-dumpformat-testutils --all-targets --locked`
- 模拟策略：使用现有真实类型和测试夹具，不新增行为 mock。

## 步骤

1. 记录 warning 并确认枚举全部变体已覆盖。
2. 删除不可达兜底而不添加 allow；如缺少映射断言，扩展同目录独立测试文件。
3. 运行 fmt、目标测试和 deny-warnings check。

## 验证

- 运行：`cargo test -p astersql-dumpformat-testutils --locked -- --nocapture && RUSTFLAGS='-D warnings' cargo check -p astersql-dumpformat-testutils --all-targets --locked`
- 预期：警告消失，所有当前 PhysicalType 行为测试通过。
- 所需证据：失败与通过输出、非零有效测试数、确切退出码、实际槽位和绝对 CARGO_TARGET_DIR、已审查差异。

## 完成

不得用 lint allow 压制警告，Rust 测试不得放回生产源文件。完成后使用 `$git-commit` 仅提交本任务变更。

## 回归记录

- 共享槽位：2；`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-2`。
- 失败证据：`cargo check -p astersql-dumpformat-testutils --all-targets --locked` 报告 `parquet_writer.rs:442` 的 `unreachable pattern`。
- 行为验证：`cargo test -p astersql-dumpformat-testutils --locked -- --nocapture` 通过，7 passed，0 failed。
- 聚焦检查：`cargo check -p astersql-dumpformat-testutils --all-targets --locked` 通过；`cargo rustc -p astersql-dumpformat-testutils --lib --locked -- -D warnings` 通过。
- 待回归项：计划规定的 `RUSTFLAGS='-D warnings' cargo check -p astersql-dumpformat-testutils --all-targets --locked` 在依赖 `astersql-errors` 阶段因 73 个既有命名/死代码警告而失败，未进入目标 crate；需在该无关警告清理后重跑原命令。
