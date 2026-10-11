# 任务 8: 定位 RealTiKV 外键阶段

批次：【批次 8】 依赖批次 7

状态：未开始

目的：在不优化实现的前提下，固定外键用例完整状态流并测出倍增、更新、校验和 DDL 各阶段开销。

来源任务：14.982 秒的 `test_add_foreign_key_with_auto_create_index`。

预计会话范围：单个测试的回归断言、临时分段测量和真实 playground 生命周期。

## 文件

- 修改：`tests/realtikvtest/addindextest/add_index_test.rs`（只添加永久行为断言；临时计时代码完成后移除）
- 对照：`tests/realtikvtest/addindextest/add_index_test.go`
- 产物：`target/rust-performance/2026-10-11-addindex-stages.tsv`

## 上下文

- 必须保留 14 次倍增、131072 行、成功创建/删除、非法引用失败、再次扩容/修复/创建的完整顺序。
- 当前主机未发现 `tiup`；执行时先重新检查。没有真实 playground 不能声称取得阶段基线。
- 若触及 DDL 行为，先读 `pkg/ddl/doc.go` 和 `docs/agents/ddl/README.md`。

## Cargo 共享槽位规则

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

- 行为：最终 131072 行、自动索引存在/删除、失败 DDL 不残留、最终外键成功。
- 失败验证测试：添加索引生命周期和失败后 metadata 清理断言；旧测试缺少这些显式检查。
- 失败验证命令：真实 playground 下运行精确目标，新增断言在实现前应编译失败或缺少查询 helper。
- 通过验证命令：`cargo nextest run --locked --package astersql-tests-realtikvtest-addindextest --test add_index_test --profile diagnostic --test-threads 1 -E 'test(=test_add_foreign_key_with_auto_create_index)'`
- 模拟策略：必须真实 tikv-slim，不以 mock 替代。

## 步骤

1. 检查 tiup，使用唯一 tag 启动、探活并设置 trap；复核 testfailpoint 使用。
2. 添加生命周期断言并运行；临时测量各阶段三轮耗时，保存 TSV 后移除临时代码。
3. 格式化、复跑精确测试，停止进程并只清理已核实属于该 tag 的数据。

## 完成

取得真实三轮阶段数据和完整生命周期断言后标记 `已完成`，使用 `$git-commit` 提交断言变更并删除任务文件；缺少 tiup 时记录检查并标记 `已阻塞`。
