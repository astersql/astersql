# `pkg/types/pg_json.rs`

## 文件定位

PostgreSQL `json` 的独立已校验原文值。`lib.rs` 导出模块并单独挂载 `pg_json_test.rs`。该模块不是 MySQL BinaryJSON 或 jsonb，也不负责 SQL 定型和协议接线。

## 接口与契约

`PgJson` 私有保存 String，仅通过 `FromStr` / `from_bytes` 校验后构造；`as_str`、`as_bytes` 和 `Display` 逐字节返回原文，包括外部 JSON 空白、键序、重复键、转义拼写、数字尾零及指数。相等和 Hash 比较原文，不执行 JSON 语义比较。SQL NULL 使用外层 Option，与非 NULL 的四字节 `null` 区分。

`from_bytes` 先检查 UTF-8，两个入口均拒绝原始 NUL（`PgJsonError::InvalidEncoding`，22021）；非法词法和结构使用 `InvalidSyntax`（22P02）。错误消息不包含用户原文。允许所有根种类、任意大小的合法数字词法、合法四位 Unicode 转义，包括 u0000 和孤立代理转义；不解码字符串、不转浮点或 numeric。

## 实现与边界

Lexer 按字节扫描，显式 continuation 状态栈校验容器、成员分隔及完整根值，避免 Rust 调用栈随嵌套增长。UTF-8 在词法之前验证；字符串只检查控制字节和转义语法，因此不会误拒绝孤立代理转义。时间 O(输入字节数)，辅助空间 O(嵌套深度)，存量原文占 O(输入字节数)。未定义与 PostgreSQL 相同的资源上限；内存耗尽仍受进程资源约束，不声称无限资源。

该模块不提供 JSON 运算、JSONB 转换、隐式转换、参数/结果协议或 typed NULL 元数据；由后续批次消费此类型。没有不支持路径的有损回退入口，后续不支持的 SQL 转换应在对应层返回 0A000。

## 验证

领取 Cargo 共享槽位并设置绝对 CARGO_TARGET_DIR 后运行：`cargo test -p astersql-types --lib pg_json_test -- --nocapture --test-threads=1`。独立测试覆盖原文保真、数值边界、全部65536种 Unicode 转义、ASCII 转义/控制字节、非法输入与恢复、NULL、深层容器及完整文档截断。单元测试不代表后续 SQL/TCP 四方向或 PostgreSQL 18.6 客户端差分验收。

## 2026-10-27 交付证据

依赖批次5：`f931d70e22bb36c9fd9edcb8c17c83c454773500`。首次使用现有 serde_json 校验器，6项独立行为测试编译运行后4通过、2失败（孤立代理转义和深容器），退出101；换成词法状态栈后同6项通过。增加覆盖后最终8项通过，UUID回归5项通过，无忽略或失败。

Ready 在仓库根运行，下列命令均退出0（fmt先于Rust验证）：

    cargo fmt --all
    cargo fmt --all -- --check
    cargo test -p astersql-types --lib pg_json_test -- --nocapture --test-threads=1
    cargo test -p astersql-types --lib pg_uuid_test -- --nocapture --test-threads=1
    make lint
    git diff --check

两个阶段均动态领取槽位1，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`，结束释放自己的锁并保留缓存。日志位于本机 `/tmp/pg06-json-red.log`、`/tmp/pg06-json-green.log`、`/tmp/pg06-json-ready.log`，不纳入Git。没有Go源码、依赖或Bazel变更，不触发bazel_prepare；未运行Go单元测试，failpoint流程不适用。verify-profile技能目录不存在，直接执行根AGENTS与testing-flow规定的Ready检查。

额外运行 `python3 /tmp/pg06-reference.py`（退出0）：使用批次5已校验的 `/tmp/pg05-uuid-reference/install/bin`，新建临时UTF8数据库，核验实际server_version_num=180006及`PostgreSQL 18.6 on aarch64-apple-darwin25.1.0`；17项输入cast为json再输出text的UTF8 hex逐字节相等，40项非法输入返回22P02。包含大指数、重复键、外部空白、原始Unicode及孤立代理/零码点转义。脚本finally停止自己的服务并删除临时集群；结果保存在本机 `/tmp/pg06-reference.json`。该补充只核验词法契约，不是目标服务的四方向协议差分，不用于代替后续批次验收。

未验证：SQL类型身份、typed NULL元数据、参数/结果四方向（后续批次）、完整资源上限、性能基准和全仓测试。正确性风险集中于自有词法器，使用全转义/截断/容器组合测试降低风险；不改变MySQL行为或依赖。算法线性扫描，尚无基准数据。
