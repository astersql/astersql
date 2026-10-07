// Copyright 2026 AsterSQL.

// `resourcegroup` crate 的 build 脚本。
//
// 从 Go 模块 `github.com/pingcap/kvproto` 拉取 `resource_manager.proto`
// 及其直接依赖 `apipb.proto`，
// 关闭 lite_runtime 选项后用 `protobuf_codegen_pure` 生成 Rust 绑定，
// 供资源组（Resource Group）与 Resource Manager 的 protobuf 交互使用。

use std::path::PathBuf;
use std::process::Command;

fn main() {
    // Prefer an already-cached module; only treat download failure as fatal when
    // `go list` still cannot resolve the directory (avoids proxy EOF flakes).
    // 优先使用本地缓存的 kvproto 模块，避免代理偶发 EOF 导致构建失败。
    let _ = Command::new("go")
        .args(["mod", "download", "github.com/pingcap/kvproto"])
        .status();
    let output = Command::new("go")
        .args(["list", "-m", "-f", "{{.Dir}}", "github.com/pingcap/kvproto"])
        .output()
        .expect("query kvproto Go module directory");
    assert!(
        output.status.success(),
        "go list failed for kvproto (download/cache unavailable)"
    );

    let kvproto = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
    let schema_dir = kvproto.join("proto");
    let source = schema_dir.join("resource_manager.proto");
    let apipb_source = schema_dir.join("apipb.proto");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let normalized = out_dir.join("resource_manager.proto");
    let normalized_apipb = out_dir.join("apipb.proto");
    let schema = std::fs::read_to_string(&source).expect("read resource_manager.proto");
    let apipb_schema = std::fs::read_to_string(&apipb_source).expect("read apipb.proto");
    // 关闭 rustproto.lite_runtime_all，以便生成完整字段访问器。
    std::fs::write(
        &normalized,
        schema.replace(
            "option (rustproto.lite_runtime_all) = true;",
            "option (rustproto.lite_runtime_all) = false;",
        ),
    )
    .expect("write normalized resource_manager.proto");
    std::fs::write(
        &normalized_apipb,
        apipb_schema.replace(
            "option (rustproto.lite_runtime_all) = true;",
            "option (rustproto.lite_runtime_all) = false;",
        ),
    )
    .expect("write normalized apipb.proto");

    // gogo/protobuf 与 kvproto include 目录作为 proto 依赖搜索路径。
    let gogo = PathBuf::from(
        String::from_utf8(
            Command::new("go")
                .args(["env", "GOMODCACHE"])
                .output()
                .expect("query Go module cache")
                .stdout,
        )
        .unwrap()
        .trim(),
    )
    .join("github.com/gogo/protobuf@v1.3.2");
    let include = kvproto.join("include");
    let gogo_protobuf = gogo.join("protobuf");

    protobuf_codegen_pure::run(protobuf_codegen_pure::Args {
        out_dir: out_dir.to_str().unwrap(),
        includes: &[
            out_dir.to_str().unwrap(),
            schema_dir.to_str().unwrap(),
            include.to_str().unwrap(),
            gogo.to_str().unwrap(),
            gogo_protobuf.to_str().unwrap(),
        ],
        input: &[
            normalized.to_str().unwrap(),
            normalized_apipb.to_str().unwrap(),
        ],
        customize: Default::default(),
    })
    .expect("generate resource_manager protobuf bindings");

    // 去掉生成文件顶部的 #! / //! 属性，避免作为子模块 include 时冲突。
    for generated_file in ["resource_manager.rs", "apipb.rs"] {
        let generated_path = out_dir.join(generated_file);
        let generated = std::fs::read_to_string(&generated_path)
            .unwrap_or_else(|_| panic!("read generated {generated_file} bindings"))
            .lines()
            .filter(|line| !line.starts_with("#!") && !line.starts_with("//!"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(generated_path, generated)
            .unwrap_or_else(|_| panic!("normalize generated {generated_file} module attributes"));
    }

    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-changed={}", apipb_source.display());
}
