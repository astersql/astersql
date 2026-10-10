# 任务 9: 复用Parser生成产物

批次：【批次 2】 依赖批次 1

状态：未开始

目的：避免两条 parser 防漂移测试重复构建/渲染完整 LR 表，同时保持独立基线与提交产物校验。

来源任务：用户提供的 `target/rust-test.Aw4Dhb` 慢测日志。

预计会话范围：一个聚焦会话可完成该测试族的基线、热点修复与定向验证。

## 文件

- 修改：`pkg/parser/parsergen_baseline_aster_unit_test.rs`
- 修改：`pkg/parser/parsergen/generate.rs`
- 修改：`pkg/parser/parsergen/generate_aster_unit_test.rs`

## 上下文

- 两个测试各超时约 10 秒；`assert_render_is_stable` 渲染两次，`check_generated_outputs` 再生成完整 main/hint 输出。

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

- 行为：grammar metadata、trace、确定性、checked-in bytes 与版权标记均验证，昂贵构建在单进程内只做必要次数。
- 失败验证测试：上述来源目标及其一秒性能门槛。
- 失败验证命令：`tools/check/rust-test-performance.sh --max-seconds 1 --runs 1 -- --package astersql-parser --package astersql-parsergen -E 'test(/(parsergen_main_tables_match_rust_baseline|committed_outputs_match_current_grammars)$/)'`
- 预期失败原因：完整 grammar build/render 重复执行并超 10 秒。
- 通过验证命令：同一命令改为 `--runs 3`，并运行 `make parser_fmt` 与 `make parser_unit_test`。
- 模拟策略：读取真实 grammar 与 committed outputs，不 mock 解析表。

## 步骤

1. 运行失败验证并保存退出码、有效测试数、三阶段耗时或采样。
2. 分段计时 parse/build/render/check。
3. 增加一次生成同时返回结构与渲染产物的内部接口或测试缓存。
4. 保持独立 trace oracle，运行 parser 专用 Make targets。
5. 运行 `cargo fmt --all`、通过验证、适用周边测试、`make lint` 与 diff 自审。

## 验证

- 运行：同一命令改为 `--runs 3`，并运行 `make parser_fmt` 与 `make parser_unit_test`。
- 预期：2 个目标通过；生成文件无漂移，优先 ≤1 秒并报告 build/render 分段。
- 所需证据：修复前后每个目标的耗时、退出码、有效测试数、保留的规模/矩阵/断言，以及未达一秒项的可复现下界。

## 完成

如修改 Rust 源文件，确认顶部保留 PingCAP Apache License 并增加 `// Copyright 2026 AsterSQL.`；记录确切修改文件和符号、目标测试的三次耗时及 Ready 验证。获得证据后状态改为 `已完成` 并使用技能 `$git-commit` 独立提交；无关环境阻断才可标记 `已完成，待回归`，真实未解决热点不得误标完成。

