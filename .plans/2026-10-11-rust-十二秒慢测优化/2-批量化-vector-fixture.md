# 任务 2: 批量化 vector fixture

批次：【批次 2】 依赖批次 1

状态：未开始

目的：只优化 `setup_vector_search_with_pk` 的 6000 行写入，保持行数、值分布、ANALYZE 与 Domain 隔离。

来源任务：三个 vectorsearch 热点共享的 fixture 写入阶段。

预计会话范围：单一 helper 与其等价性测试；不处理 golden 查询执行或最终 12 秒验收。

## 文件

- 修改：`pkg/planner/core/casetest/vectorsearch/vector_index_test.rs`
- 测试：`pkg/planner/core/casetest/vectorsearch/vector_index_test.rs`
- 对照：`pkg/planner/core/casetest/vectorsearch/vector_index_test.go`

## 上下文

- 当前每种 Cascades 模式执行 2000 条三行 INSERT。
- 可改为有上限的多值批次，但必须仍写入 6000 行，三组 id/vector/a/b 分布各 2000，并执行真实 ANALYZE。
- 该 crate 是测试专用 crate，测试逻辑继续留在独立 `vector_index_test.rs`，不移入生产源文件。

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

- 行为：批量写入后仍有 6000 行，三个 id 区间和向量值各 2000 行，统计信息已生成。
- 失败验证测试：先添加 fixture 等价性测试，要求批次边界覆盖首尾 id、总数和分组数；若旧 helper 无法单独验证，先提取仅供测试文件使用的窄 helper。
- 失败验证命令：`cargo nextest run --locked --package astersql-planner-core-casetest-vectorsearch -E 'test(=vector_index_test::vector_pk_fixture_preserves_go_distribution)'`
- 预期失败原因：新等价性测试或 helper 尚不存在。
- 通过验证命令：同上，并运行三个现有目标一次。
- 模拟策略：真实 mock store/TestKit/ANALYZE，不 mock SQL。

## 步骤

1. 添加失败的 fixture 分布测试并确认红灯。
2. 将逐条 INSERT 改成受控批量 INSERT；不得减少 6000 行。
3. 运行 `cargo fmt --all`、新测试和三个现有目标一次，记录 fixture 阶段前后耗时。

## 完成

等价性测试和三个现有目标通过，fixture 分段耗时下降且 Go 规模未变后标记 `已完成`，使用 `$git-commit` 提交并删除本任务文件。
