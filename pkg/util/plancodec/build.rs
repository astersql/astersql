// Copyright 2026 AsterSQL.

// `plancodec` crate 的构建脚本。
//
// 从 Go 模块缓存中的 tipb 读取 Explain 协议，关闭 lite runtime 后生成完整的
// Rust protobuf 绑定，供二进制执行计划的编码与解码逻辑使用。

use std::path::PathBuf;
use std::process::Command;

/// 执行查询路径的外部命令，并将其标准输出解析为路径。
fn command_path(program: &str, args: &[&str]) -> PathBuf {
    let output = Command::new(program)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("run {program}: {error}"));
    assert!(output.status.success(), "{program} failed");
    PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
}

fn main() {
    // 通过 Go 工具链定位版本锁定后的模块目录，避免依赖机器上的固定缓存路径。
    let tipb = command_path(
        "go",
        &["list", "-m", "-f", "{{.Dir}}", "github.com/pingcap/tipb"],
    );
    let upstream_source = tipb.join("proto/explain.proto");
    let gogo = command_path("go", &["env", "GOMODCACHE"]).join("github.com/gogo/protobuf@v1.3.2");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let source = out_dir.join("explain.proto");
    // 完整运行时会生成后续解码流程依赖的字段访问器；修改副本以保持上游文件不变。
    let proto = std::fs::read_to_string(&upstream_source)
        .expect("read tipb explain.proto")
        .replace(
            "(rustproto.lite_runtime_all) = true",
            "(rustproto.lite_runtime_all) = false",
        );
    std::fs::write(&source, proto).expect("write normalized explain.proto");
    let out = out_dir.to_string_lossy().into_owned();
    let source_string = source.to_string_lossy().into_owned();
    let source_dir = out_dir.to_string_lossy().into_owned();
    let tipb_include = tipb.join("include").to_string_lossy().into_owned();
    let gogo_string = gogo.to_string_lossy().into_owned();
    let gogo_protobuf = gogo.join("protobuf").to_string_lossy().into_owned();
    // tipb 与 gogo/protobuf 的目录共同组成 explain.proto 的依赖搜索路径。
    protobuf_codegen_pure::run(protobuf_codegen_pure::Args {
        out_dir: &out,
        includes: &[&source_dir, &tipb_include, &gogo_string, &gogo_protobuf],
        input: &[&source_string],
        customize: Default::default(),
    })
    .expect("generate explain protobuf binding");
    // 生成文件通过 `include!` 嵌入子模块，需去掉只允许出现在模块开头的内部属性和文档。
    let generated_path = out_dir.join("explain.rs");
    let generated = std::fs::read_to_string(&generated_path)
        .expect("read explain binding")
        .lines()
        .filter(|line| !line.starts_with("#!") && !line.starts_with("//!"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(generated_path, generated).expect("normalize explain binding");
    println!("cargo:rerun-if-changed={}", upstream_source.display());
}
