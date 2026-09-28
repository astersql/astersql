# Rust 测试入口

从仓库根目录运行 `./tests/run-rust-tests.sh list` 查看可用套件。无参数时运行
`local`：globalkill、graceshutdown、readonly、llm 和 RealTiKV 根 crate 的
Cargo 测试。也可以指定单个套件，例如：

```bash
./tests/run-rust-tests.sh realtikv ddltest
./tests/run-rust-tests.sh realtikv all
```

这些 Cargo 测试不等同于真实 TiKV 集群验收；是否连接真实 TiKV 取决于具体用例。

## SQL 集成测试

`integration` 调用 `tests/integrationtest/run-rust-tests.sh`。它检查现有 PD
的健康状态及至少一个 Up 状态的 TiKV store，编译 Rust tidb-server，随后通过
`tests/integrationtest/run-tests.sh` 启动服务并执行 mysql-tester 用例。
它不会启动或停止 PD/TiKV。

需要显式选择用例；原来的 `rust_integration_smoke_v1` 用例已不在仓库中：

```bash
./tests/run-rust-tests.sh integration -t select
./tests/integrationtest/run-rust-tests.sh -t topn_pushdown
```

用例位于 `tests/integrationtest/t/`，预期结果位于 `r/`。传入的参数直接交给
`run-tests.sh`。默认 PD 地址是 `127.0.0.1:2379`，可用 `TIKV_PATH` 和
`PD_HTTP_URL` 覆盖。`PROTOC` 默认指向
`/opt/homebrew/opt/protobuf@21/bin/protoc`，也可自行设置。
首次运行时需要 Go 来构建 mysql-tester。运行前还需要 Cargo、curl、lsof、
unzip 和可访问的 PD/TiKV。

`tests/integrationtest2/run-rust-tests.sh` 是独立入口，未纳入统一命令。
它调用 `tests/integrationtest2/run-tests.sh`，后者会删除该目录下的 `data`
和 `logs`，再启动上下游 PD/TiKV 集群。至少需要事先提供
`tests/integrationtest2/third_bin/pd-server` 和 `tikv-server`；具体用例还可能
需要 BR、Dumpling 或 TiCDC。使用前显式指定用例，例如：

```bash
./tests/integrationtest2/run-rust-tests.sh -t br_integration
```

以上命令仅说明入口和前置条件，不能推断现有 Go TiDB 用例均已与 Rust 服务兼容。
