// Copyright 2026 AsterSQL.

// build 脚本：从官方 tipb protobuf 生成 Rust 绑定，供 RUv2 / TiFlash 统计使用。
//
// 通过 `go list`/`go env` 定位 tipb 与 gogo/protobuf，再用 protobuf-codegen-pure
// 生成 expression/schema/executor；并为 `TiFlashWaitSummary` 补兼容方法名。

use std::path::PathBuf;
use std::process::Command;

/// 查询指定 Go module 在本地模块缓存中的目录路径。
fn go_module_dir(module: &str) -> PathBuf {
    // `go list -m` can succeed with an empty `.Dir` in a cold module cache.
    let download = Command::new("go")
        .args(["mod", "download", module])
        .output()
        .expect("download Go module");
    assert!(
        download.status.success(),
        "go mod download failed for {module}: {}",
        String::from_utf8_lossy(&download.stderr)
    );
    let output = Command::new("go")
        .args(["list", "-m", "-f", "{{.Dir}}", module])
        .output()
        .expect("query Go module directory");
    assert!(output.status.success(), "go list failed for {module}");
    let directory = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
    assert!(
        directory.is_dir(),
        "Go module directory missing for {module}"
    );
    directory
}

/// 生成 tipb 绑定：定位依赖、运行 codegen，并补齐方法名兼容层。
fn main() {
    println!("cargo:rerun-if-env-changed=GOMODCACHE");

    // 官方 tipb schema 与 gogo 依赖路径。
    let tipb = go_module_dir("github.com/pingcap/tipb");
    let proto_dir = tipb.join("proto");
    let gogo = go_module_dir("github.com/gogo/protobuf@v1.3.2");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("tipb");
    std::fs::create_dir_all(&out).expect("create generated tipb directory");

    let inputs = ["expression.proto", "schema.proto", "executor.proto"]
        .into_iter()
        .map(|name| proto_dir.join(name))
        .collect::<Vec<_>>();
    let input_strings = inputs
        .iter()
        .map(|path| path.to_str().unwrap())
        .collect::<Vec<_>>();
    let include_dir = tipb.join("include");
    let includes = [&proto_dir, &include_dir, &gogo]
        .into_iter()
        .map(|path| path.to_str().unwrap())
        .collect::<Vec<_>>();
    protobuf_codegen_pure::run(protobuf_codegen_pure::Args {
        out_dir: out.to_str().unwrap(),
        includes: &includes,
        input: &input_strings,
        customize: Default::default(),
    })
    .expect("generate bindings from official tipb schemas");
    let executor = out.join("executor.rs");
    let mut executor_source = std::fs::read_to_string(&executor).expect("read generated executor");
    // protobuf-codegen 可能生成 camelCase；补 snake_case 别名以稳定调用。
    if executor_source.contains("fn get_minTSO_wait_ns")
        && !executor_source.contains("fn get_min_tso_wait_ns")
    {
        executor_source.push_str("\n// Stable spelling across protobuf-codegen 2.8 sources.\n");
        executor_source.push_str(
            "impl TiFlashWaitSummary {\n    pub fn get_min_tso_wait_ns(&self) -> u64 {\n        self.get_minTSO_wait_ns()\n    }\n}\n",
        );
        std::fs::write(&executor, executor_source)
            .expect("write TiFlashWaitSummary compatibility method");
    }
    std::fs::write(
        out.join("mod.rs"),
        "pub mod expression;\npub mod schema;\npub mod executor;\npub use executor::*;\n",
    )
    .expect("write generated tipb module list");
}
