# UUID 真实客户端四方向差分证据

2026-10-27，批次 5；依赖批次 4 完整 SHA：
`2a27a07efdc6a1ad62dcf060b42e62b4a609ca3a`。
本批只新增真实客户端测试、双协议夹具和本证据文档，没有生产代码修改。

## 精确参考构建

从 [PostgreSQL 官方 18.6 目录](https://ftp.postgresql.org/pub/source/v18.6/)
下载 `postgresql-18.6.tar.bz2`，按同目录 `.sha256` 校验通过：
`555610c24d53e4316da5b7d3fc25c279d96856d5e0e23ee308c328c5fa881d9f`。
首次网络下载不完整，断点续传恢复后校验，未改版本字符串。
独立安装于 `/tmp/pg05-uuid-reference/install`，构建命令为：

```sh
cd /tmp/pg05-uuid-reference
shasum -a 256 -c postgresql-18.6.tar.bz2.sha256
tar -xjf postgresql-18.6.tar.bz2
cd postgresql-18.6
./configure --prefix=/tmp/pg05-uuid-reference/install --without-icu --without-readline --without-zlib
make -j6
make install
```

全部退出码 0。SQL `version()` 实际返回：
`PostgreSQL 18.6 on aarch64-apple-darwin25.1.0, compiled by Apple clang version 16.0.0 (clang-1600.0.26.6), 64-bit`；
`SHOW server_version_num` 为 **180006**。客户端为系统 libpq **180000**，不把客户端版本当服务器版本。
数据库及客户端编码 UTF8，DateStyle=`ISO, MDY`，TimeZone=`Asia/Shanghai`；
UUID 不受日期或时区配置影响。完整配置（含编译器环境）保存在本轮 `identity.json` / `summary.json`。

## 覆盖与结果

`pg05_uuid_libpq_four_directions` 在真实共享执行消费者的同一 Server
启动 PG/MySQL 两个临时端口，Python ctypes 直接调用 libpq，参考端独立 initdb。
**1 个 Rust 行为测试通过，包含 1,212 项逐项断言的 libpq 差分**：

| 覆盖 | 项数 | 断言 |
| --- | ---: | --- |
| 260 个位模式及 SQL NULL × 四方向 | 1044 | IT→OT、IT→OB、IB→OT、IB→OB；原始输入、完整结果字节与元数据 |
| 推导 OID、显式 OID 0、空集、首行 NULL × 值/NULL × 四方向 | 32 | 参数描述 2950，结果 2950/16/-1，格式 0/1；含全 NULL |
| 四种文本拼写 × text/binary 输出 | 8 | 大小写、花括号、无连字符、每四位分组归一 |
| 32 个非法输入 × 两种结果格式 | 64 | 错误 SQLSTATE、无结果列及无 DataRow |
| 每次拒绝后的同语句/连接恢复 | 64 | libpq Sync 后重新绑定另一 UUID 成功 |

位模式包括全零、全 ff、顺序字节及每个单独置位/清位，覆盖任意 version/variant 位。
空 UUID 字节不等于 NULL，binary 是从左到右的 16 字节，无 GUID 混合端序。
准备语句连续重绑定不同值、NULL 和结果格式，每次仍保留 UUID 身份。
每个成功项 SQLSTATE 记录为 `N/A`。

非法输入包括空串、空白、URN、非十六进制、缺/多位、错位/尾部连字符和不配对花括号：
参考及目标均 **22P02**；非法 UTF8 均 **22021**；binary 长度 0–15 均 **08P01**，
长度 17/32 均 **22P03**。参数拒绝发生在 Bind，使用既有 TCP 测试核对阶段；
libpq 结果记录完整可观察的错误、元数据和行，未把缺字段默认成相等。
PG 语句仅 SELECT，无 DML 副作用；同 Server MySQL 的原值也在前、中、后检查中保持一致。

statement Describe 使用 `PQdescribePrepared` 断言参数 OID 与结果描述。
libpq 不暴露的原始 portal Describe、混合列格式、重复 Execute、嵌入 NUL 的文本参数、
忽略错误后的 Execute 直到 Sync，由本轮复跑的既有 TCP 测试补证。
未声称 libpq 暴露了这些字段。标量 UUID 不适用数组下界/维数。

MySQL 使用同一个 Server 的原协议，保存 CHAR(36)、BINARY(16) 和 NULL。
三个检查点断言值、MySQL STRING(254) 列描述及原始行载荷一致，未将它们重解释为 PG UUID。

首次 18.4 补充运行在所有 1212 项差分之后，因夹具使用目标端不支持的 SQL
`DEALLOCATE ALL` 返回 42601 而失败；这是夹具选择错误，不是 UUID 行为红灯。
改为协议级 `PQclosePrepared` 后通过；既有生产实现正确，因此没有人为制造生产失败。
18.4 最终补充结果为 1212 项相等，summary 明确 `passed=false, supplemental=true`；
本批完成依据为随后当前代码对 **18.6** 的严格通过。

## 本轮验证命令

所有 Cargo 命令前均按任务规则动态原子领取共享槽位 **1**，设置：

```sh
export CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1
export PG18_BIN=/tmp/pg05-uuid-reference/install/bin
unset PG05_UUID_SUPPLEMENTAL
cargo fmt --all
cargo fmt --all -- --check
cargo test -p astersql-server --lib pg05_uuid_libpq -- --nocapture --test-threads=1
cargo test -p astersql-server --lib pg05_uuid_input -- --nocapture --test-threads=1
cargo test -p astersql-server --lib pg05_uuid_output -- --nocapture --test-threads=1
make lint
git diff --check
```

Ready：上述退出码均 0；测试数依次为 **1、2、3**（无忽略/失败）。
额外命令 `cargo test -p astersql-server --lib pg05_uuid_sql -- --nocapture --test-threads=1`
退出码 0、1 项通过，补证 SQL/wire 类型身份、NULL、错误和恢复。首次 Rust 修改后已先运行 `cargo fmt --all`。
没有 Go、Cargo manifest 或 Bazel 变更，不触发 `make bazel_prepare`；未运行昂贵广扫。
`.agents/skills/tidb-verify-profile` 不存在，使用根 AGENTS.md 与 testing-flow 规定的 Ready 检查。

18.6 原始证据保存在仓库忽略目录 `target/pg05-uuid-evidence/47353-54344/`：
`identity.json`、`cases.jsonl`、`summary.json`、`mysql-before.json`、`mysql-during.json`、
`mysql-after.json`、`rust-cleanup.json`。本机日志 `/tmp/pg05-uuid-ready.log` 和
`/tmp/pg05-uuid-lint.log`；缓存证据不纳入 Git，以上摘要保留可审阅结果。
脚本每次运行自动创建新的证据目录。

参考客户端和 prepared statements 正常关闭；异常路径 finally 关闭客户端并停止自有参考进程，
TemporaryDirectory 删除自有集群；SIGTERM 转为异常走同样清理。
Rust guard 关闭双协议 Server 并删除唯一测试数据库；子进程 guard 先关闭 stdin、再等待 Python。
本轮确认参考端口、PG/MySQL 端口均关闭，临时集群目录删除，数据库删除。
Cargo 阶段结束后仅删除自己的槽位锁，保留编译缓存。

## 风险与限制

正确性与兼容性：本轮 UUID 四方向及错误/恢复契约无已观察差异；不承诺完整 PostgreSQL
隐式转换矩阵或其他类型。SQL `DEALLOCATE ALL` 支持不属于本任务，使用协议关闭避免扩大范围。
没有改变生产行为或依赖，无新增运行时性能开销。未运行性能基准、全仓测试、
持久 TiKV 或 RealTiKV；内存共享消费者的 TCP 差分不代表持久化验收。

重跑时先动态领取共享槽位，再指定真实 18.6 的 `PG18_BIN` 并运行上面命令。
默认模式拒绝其他版本；只有明确设置 `PG05_UUID_SUPPLEMENTAL=1` 才允许补充运行，
补充模式不满足 18.6 验收。
