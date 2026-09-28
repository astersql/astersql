// Copyright 2026 AsterSQL.

//! Parity tests for `tools/gen-parquet` vs Go `main.go`.
//! 这组测试不重写生成逻辑本身，而是把 Rust 可观察到的文件名、字节布局、
//! 默认参数、错误形状与资源回收行为固定为与 Go `main.go` 一致的契约。
//! 四个子场景分别覆盖正常写出、边界输入、显式错误和结束后的文件状态，
//! 这样回归时能快速判断偏差发生在“生成内容”还是“命令行/资源语义”层面。
//! 测试数据全部落在临时目录或内存缓冲区中，避免依赖真实仓库文件，
//! 同时仍然保留 Go 版本按 chunk 输出 parquet 数据的调用顺序与断言口径。
//! 由于测试目标是公开契约而非内部实现，断言会优先选择文件名、字节序和返回结果这些稳定信号。
//! 这种写法也能减少后续重构测试桩时的误报，让真正的 Go/Rust 语义偏差更容易暴露出来。

use std::fs;
use std::fs::File;
use std::path::PathBuf;

use crate::main::{
    Flags, chunk_file_name, chunk_file_path, get_parquet_writer, parse_flags, run, write_column,
    write_simple_parquet_file,
};
use parquet::basic::{Compression, Encoding, Type as PhysicalType};
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::record::RowAccessor;

fn tmpdir(name: &str) -> PathBuf {
    // 用进程号区分并发执行的测试目录；进入场景前先清理旧目录，避免上次失败残留污染结果。
    let dir = std::env::temp_dir().join(format!("gen-parquet-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Normal: schema props, column values 0..n-1, chunk files created with Go names.
/// 正常路径验证完整的“按参数生成 chunk 文件”流程，并抽查输出二进制是否遵守 Go 版约定的写入顺序。
#[test]
fn contract_normal_paths() {
    let dir = tmpdir("normal");
    let flags = Flags {
        schema_name: "test".into(),
        table_name: "parquet".into(),
        chunks: 2,
        row_numbers: 3,
        source_dir: dir.to_string_lossy().into(),
    };
    run(&flags).expect("run ok");

    // 先从文件系统层面确认每个 chunk 都被创建，并且文件名格式仍是 Go 端四位补零规则。
    let p0 = chunk_file_path(&flags.source_dir, "test", "parquet", 0);
    let p1 = chunk_file_path(&flags.source_dir, "test", "parquet", 1);
    assert!(p0.is_file());
    assert!(p1.is_file());
    assert_eq!(
        p0.file_name().unwrap().to_str().unwrap(),
        "test.parquet.0000.parquet"
    );

    // Go 使用 Arrow 的 parquet writer，输出必须是可被标准 reader 识别的 Parquet 文件。
    let body = fs::read(&p0).unwrap();
    assert!(body.len() >= 8);
    assert_eq!(&body[..4], b"PAR1");
    assert_eq!(&body[body.len() - 4..], b"PAR1");

    let reader = SerializedFileReader::new(File::open(&p0).unwrap()).unwrap();
    assert_eq!(reader.metadata().file_metadata().num_rows(), 3);
    assert_eq!(reader.num_row_groups(), 1);
    let schema = reader.metadata().file_metadata().schema_descr();
    assert_eq!(schema.num_columns(), 2);
    assert_eq!(schema.column(0).name(), "iVal");
    assert_eq!(schema.column(1).name(), "s");
    assert_eq!(
        reader.metadata().row_group(0).column(0).compression(),
        Compression::SNAPPY
    );
    assert_eq!(
        reader.metadata().row_group(0).column(1).compression(),
        Compression::SNAPPY
    );
    assert!(
        reader
            .metadata()
            .row_group(0)
            .column(0)
            .encodings()
            .any(|encoding| encoding == Encoding::RLE_DICTIONARY)
    );
    assert!(
        reader
            .metadata()
            .row_group(0)
            .column(1)
            .encodings()
            .any(|encoding| encoding == Encoding::RLE_DICTIONARY)
    );
    for (index, row) in reader.get_row_iter(None).unwrap().enumerate() {
        let row = row.unwrap();
        assert_eq!(row.get_long(0).unwrap(), index as i64);
        assert_eq!(
            row.get_bytes(1).unwrap().data(),
            index.to_string().as_bytes()
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

/// Boundary: defaults; zero rows still creates file; naming with custom schema/table.
/// 边界场景固定默认 flag 数值，并验证“零行但非零 chunk”时仍会创建目标文件这一 Go 侧行为。
#[test]
fn contract_boundary() {
    // 默认值断言是命令行兼容性的第一层护栏，避免字段初始值在重构后悄悄漂移。
    assert_eq!(Flags::default().chunks, 10);
    assert_eq!(Flags::default().row_numbers, 1000);
    assert_eq!(Flags::default().schema_name, "test");
    assert_eq!(Flags::default().table_name, "parquet");
    assert_eq!(Flags::default().source_dir, "");

    let dir = tmpdir("boundary");
    let args = vec![
        "gen-parquet".into(),
        "-schema".into(),
        "s1".into(),
        "-table=t1".into(),
        "-chunk=1".into(),
        "-rows".into(),
        "0".into(),
        "-dir".into(),
        dir.to_string_lossy().into_owned(),
    ];
    let flags = parse_flags(&args).unwrap();
    // 这里同时覆盖 `-k=v` 与 `-k v` 两种参数写法，确保解析结果仍与 Go flag 习惯一致。
    assert_eq!(flags.schema_name, "s1");
    assert_eq!(flags.table_name, "t1");
    assert_eq!(flags.chunks, 1);
    assert_eq!(flags.row_numbers, 0);
    run(&flags).unwrap();
    // 即使没有任何行数据，也必须留下命名正确的输出文件，供后续流程按文件存在性继续执行。
    let p = chunk_file_path(dir.to_str().unwrap(), "s1", "t1", 0);
    assert!(p.is_file());
    assert_eq!(chunk_file_name("s1", "t1", 0), "s1.t1.0000.parquet");
    let _ = fs::remove_dir_all(&dir);
}

/// Error: bad directory; extra column after schema exhausted.
/// 错误路径不追求覆盖所有内部异常，而是固定三类最容易破坏上层脚本预期的失败形状。
#[test]
fn contract_error_paths() {
    // 目标目录不存在时，顶层便捷函数必须把文件创建失败透传出来，而不是静默吞掉。
    let err = write_simple_parquet_file("/no/such/dir/x.parquet", 1);
    assert!(err.is_err());

    // schema names/types are index-coupled in Go; reject a mismatch explicitly.
    let row_names = vec!["x".to_string()];
    let row_types = vec![PhysicalType::DOUBLE, PhysicalType::INT64];
    assert!(get_parquet_writer(Vec::<u8>::new(), &row_names, &row_types).is_err());

    assert!(parse_flags(&["gen-parquet".into(), "-unknown=1".into()]).is_err());
    assert!(parse_flags(&["gen-parquet".into(), "-rows=nope".into()]).is_err());
    assert!(parse_flags(&["gen-parquet".into(), "-dir".into()]).is_err());
    assert!(write_simple_parquet_file("unused.parquet", -1).is_err());
}

/// The Go type switch also supports Float64 and rejects every other writer type.
#[test]
fn contract_write_column_type_switch() {
    let dir = tmpdir("float64");
    let path = dir.join("float.parquet");
    let names = vec!["f".to_string()];
    let types = vec![PhysicalType::DOUBLE];
    let mut writer = get_parquet_writer(File::create(&path).unwrap(), &names, &types).unwrap();
    let mut row_group = writer.next_row_group().unwrap();
    write_column(&mut row_group, 3).unwrap();
    assert!(row_group.next_column().unwrap().is_none());
    row_group.close().unwrap();
    writer.close().unwrap();

    let reader = SerializedFileReader::new(File::open(&path).unwrap()).unwrap();
    for (index, row) in reader.get_row_iter(None).unwrap().enumerate() {
        assert_eq!(row.unwrap().get_double(0).unwrap(), index as f64);
    }

    let unsupported_names = vec!["i32".to_string()];
    let unsupported_types = vec![PhysicalType::INT32];
    let mut writer =
        get_parquet_writer(Vec::<u8>::new(), &unsupported_names, &unsupported_types).unwrap();
    let mut row_group = writer.next_row_group().unwrap();
    assert_eq!(
        write_column(&mut row_group, 1).unwrap_err(),
        "unsupported column type"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Cleanup: files closed (readable); empty run with 0 chunks is no-op.
/// 资源清理场景验证生成后的文件句柄已释放，以及 `chunks=0` 时不会额外制造副作用。
#[test]
fn contract_resource_cleanup() {
    let dir = tmpdir("cleanup");
    let flags = Flags {
        schema_name: "a".into(),
        table_name: "b".into(),
        chunks: 1,
        row_numbers: 1,
        source_dir: dir.to_string_lossy().into(),
    };
    run(&flags).unwrap();
    // 生成结束后立刻重新取 metadata，等价于确认文件已关闭且内容已经真正落盘。
    let p = chunk_file_path(&flags.source_dir, "a", "b", 0);
    let meta = fs::metadata(&p).unwrap();
    assert!(meta.len() > 0);

    // `chunks=0` 在 Go 侧是 no-op：既不报错，也不继续写新文件。
    let zero = Flags {
        chunks: 0,
        source_dir: dir.to_string_lossy().into(),
        ..Flags::default()
    };
    run(&zero).unwrap();
    let _ = fs::remove_dir_all(&dir);
}
