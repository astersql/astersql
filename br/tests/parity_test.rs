// Copyright 2026 AsterSQL.

//! Parity checks against `br/tests/utils.go` public contract.
//!
//! 对照 Go `utils.go` 的公开契约：扩展名常量、SST 魔数识别、zstd 判断与校验命令。
//! 使用临时目录构造 .sst/.log 样本，覆盖加密/压缩组合；不连真实备份存储。
//! 断言失败即契约漂移；资源在测试结束清理临时目录。
//! 魔数取自 RocksDB/TiKV footer；压缩检测走 zstd 解码成败。

use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use zstd::stream::encode_all;

use crate::stubs::Command;
use crate::{
    checkCompressionAndEncryption, extLOG, extSST, isLikelySSTFile, isZstdCompressed, parseCommand,
};

/// 临时目录序号，避免并行/重复运行冲突。
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// 创建唯一临时目录：`br-tests-parity-{pid}-{label}-{seq}`。
fn tmp_dir(label: &str) -> PathBuf {
    let n = TMP_SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "br-tests-parity-{}-{}-{}",
        std::process::id(),
        label,
        n
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create tmp dir");
    dir
}

/// 写入带指定 footer 魔数的伪 SST（至少 8 字节尾魔数）。
fn write_sst(path: &PathBuf, magic: u64) {
    let mut f = File::create(path).expect("create sst");
    // pad so seek(-8) works
    // 前填充保证可 seek 到末尾 8 字节魔数。
    f.write_all(&[0u8; 16]).unwrap();
    f.write_all(&magic.to_le_bytes()).unwrap();
}

/// 写入原始字节文件（用于 .log / zstd 样本）。
fn write_bytes(path: &PathBuf, data: &[u8]) {
    let mut f = File::create(path).expect("create file");
    f.write_all(data).unwrap();
}

/// 总契约测试：常量 → SST 识别 → zstd → parseCommand → 校验流程。
#[test]
fn go_rust_public_contract_matches() {
    // --- normal: parseCommand finds local:// storage for backup / restore point ---
    // 正常：backup full / restore point 的 local:// 路径应被解析出。
    let (path, found) = parseCommand("br backup full -s local:///tmp/backup-data");
    assert!(found);
    assert_eq!(path, "/tmp/backup-data");

    // restore point 同样允许校验（log 备份进行中除外）。
    let (path, found) = parseCommand("br restore point --storage=local:///data/pitr");
    assert!(found);
    assert_eq!(path, "/data/pitr");

    // 后者覆盖前者：`-s=` 覆盖 `--storage`。
    let (path, found) = parseCommand("br backup full --storage local://./out -s=local://override");
    assert!(found);
    assert_eq!(path, "override");

    // Valid SST magic → unencrypted SST; zstd log → unencrypted log.
    // 合法 SST 魔数 + zstd log 在无加密参数时应通过。
    let dir = tmp_dir("normal");
    write_sst(&dir.join(format!("a{extSST}")), 0xdb4775248b80fb57);
    let zstd_payload = encode_all(&b"hello-log"[..], 0).expect("zstd encode");
    write_bytes(&dir.join(format!("b{extLOG}")), &zstd_payload);
    assert!(
        checkCompressionAndEncryption(dir.to_str().unwrap(), ""),
        // 明文样本在无 encryption 参数下应通过。
        "plain backup files should pass without encryption"
    );
    assert!(isLikelySSTFile(dir.join(format!("a{extSST}"))).unwrap());
    assert!(isZstdCompressed(dir.join(format!("b{extLOG}"))).unwrap());

    // --- boundary: non-backup commands / non-local storage / empty storage ---
    // 边界：log start / 非 point restore / s3 / 空路径 / 缺 storage 均不应校验。
    let (_, found) = parseCommand("br log start -s local:///tmp/log");
    assert!(!found, "log backup must not validate");

    let (_, found) = parseCommand("br restore full -s local:///tmp/x");
    assert!(!found, "restore without point must not validate");

    let (_, found) = parseCommand("br backup full -s s3://bucket/path");
    assert!(!found, "non-local storage must not validate");

    let (_, found) = parseCommand("br backup full -s local://");
    assert!(!found, "empty local path must not validate");

    let (_, found) = parseCommand("br backup full");
    assert!(!found, "missing storage must not validate");

    // Empty dir: both flags stay true → without encryption returns true (Go).
    // 空目录：无文件时“全加密/全未加密”标志保持初值，与 Go 一致返回 true。
    let empty = tmp_dir("empty");
    assert!(checkCompressionAndEncryption(empty.to_str().unwrap(), ""));
    assert!(checkCompressionAndEncryption(
        empty.to_str().unwrap(),
        "aes128-ctr"
    ));

    // Alternate SST magic also accepted.
    // 另一合法 RocksDB/TiKV SST 魔数也应识别。
    let dir2 = tmp_dir("magic2");
    write_sst(&dir2.join(format!("c{extSST}")), 0x88e241b785f4cff7);
    assert!(isLikelySSTFile(dir2.join(format!("c{extSST}"))).unwrap());

    // --- error: open failures, invalid SST, encryption mismatches ---
    // 错误：缺失文件 open 失败；坏 footer 视为加密侧。
    let missing = dir.join("no-such.sst");
    assert!(isLikelySSTFile(&missing).is_err());
    assert!(isZstdCompressed(&missing).is_err());

    // Short / garbage file is not a valid SST → treated as encrypted.
    // 短/垃圾文件非合法 SST，在期望加密时应通过、无加密时期望失败。
    let enc_dir = tmp_dir("encrypted-sst");
    write_bytes(
        &enc_dir.join(format!("e{extSST}")),
        &[1, 2, 3, 4, 5, 6, 7, 8],
    );
    // seek(-8) works but magic won't match → allEncrypted stays true for SST-only dir
    assert!(
        !isLikelySSTFile(enc_dir.join(format!("e{extSST}"))).unwrap(),
        "non-magic footer is not a plain SST"
    );
    assert!(
        checkCompressionAndEncryption(enc_dir.to_str().unwrap(), "on"),
        "non-SST footer counts as encrypted when encryption expected"
    );
    assert!(
        !checkCompressionAndEncryption(enc_dir.to_str().unwrap(), ""),
        "all encrypted is unexpected without encryption flag"
    );

    // Plain (valid SST) files fail when encryption is required.
    // 明文合法 SST 在要求加密时必须失败。
    let plain = tmp_dir("plain-fail");
    write_sst(&plain.join(format!("p{extSST}")), 0xdb4775248b80fb57);
    assert!(!checkCompressionAndEncryption(
        plain.to_str().unwrap(),
        "aes128-ctr"
    ));

    // Non-zstd .log counts as encrypted; mixed with valid SST → mixed failure.
    // 非 zstd 的 .log 视为加密侧，与明文 SST 混放导致失败。
    let mixed = tmp_dir("mixed");
    write_sst(&mixed.join(format!("m{extSST}")), 0xdb4775248b80fb57);
    write_bytes(&mixed.join(format!("m{extLOG}")), b"not-zstd");
    assert!(!isZstdCompressed(mixed.join(format!("m{extLOG}"))).unwrap());
    assert!(!checkCompressionAndEncryption(mixed.to_str().unwrap(), ""));

    // --- resource cleanup: handles closed so files/dirs are removable ---
    // 资源：检查后句柄应关闭，文件/目录可删除。
    let cleanup = tmp_dir("cleanup");
    let sst = cleanup.join(format!("r{extSST}"));
    let log = cleanup.join(format!("r{extLOG}"));
    write_sst(&sst, 0xdb4775248b80fb57);
    write_bytes(&log, &encode_all(&b"bye"[..], 0).unwrap());
    assert!(isLikelySSTFile(&sst).unwrap());
    assert!(isZstdCompressed(&log).unwrap());
    assert!(checkCompressionAndEncryption(cleanup.to_str().unwrap(), ""));
    fs::remove_file(&sst).expect("sst handle must be closed");
    fs::remove_file(&log).expect("log handle must be closed");
    fs::remove_dir_all(&cleanup).expect("dir removable after checks");

    // Drop other tmp dirs.
    // 清理其余临时目录，避免残留。
    for d in [dir, empty, dir2, enc_dir, plain, mixed] {
        let _ = fs::remove_dir_all(d);
    }
}

/// Go's zstd reader returns EOF when a valid frame has no decompressed bytes.
/// The helper therefore treats an empty zstd frame as not compressed.
#[test]
fn empty_zstd_frame_matches_go_eof_semantics() {
    let dir = tmp_dir("empty-zstd");
    let log = dir.join(format!("empty{extLOG}"));
    write_bytes(&log, &encode_all(&b""[..], 0).expect("zstd encode"));

    assert!(!isZstdCompressed(&log).expect("open empty zstd frame"));

    fs::remove_dir_all(dir).expect("remove tmp dir");
}

/// `filepath.Walk` visits a regular root path instead of requiring a directory.
#[test]
fn validation_accepts_a_regular_file_as_the_walk_root() {
    let dir = tmp_dir("file-root");
    let sst = dir.join(format!("root{extSST}"));
    write_sst(&sst, 0xdb4775248b80fb57);

    assert!(checkCompressionAndEncryption(sst.to_str().unwrap(), ""));

    fs::remove_dir_all(dir).expect("remove tmp dir");
}

/// Cobra rejects unknown flags and string flags that have no argument.
#[test]
fn command_stub_rejects_invalid_flags_like_cobra() {
    fn no_op(_cmd: &mut Command, _args: &[String]) {}

    fn command_with_args(args: &[&str]) -> Command {
        let mut command = Command {
            Use: "validateBackupFiles".to_string(),
            Run: Some(no_op),
            ..Default::default()
        };
        command
            .Flags()
            .String("command", "", "Backup or restore command");
        command.SetArgs(args.iter().map(|arg| (*arg).to_string()).collect());
        command
    }

    assert!(command_with_args(&["--unknown=value"]).Execute().is_err());
    assert!(command_with_args(&["-x", "value"]).Execute().is_err());
    assert!(command_with_args(&["--command"]).Execute().is_err());
}

/// Cobra/pflag treats `--` as the end of flag parsing, so flag-shaped values
/// after it are positional arguments rather than unknown flags.
#[test]
fn command_stub_honors_double_dash_flag_terminator() {
    fn no_op(_cmd: &mut Command, _args: &[String]) {}

    let mut command = Command {
        Use: "validateBackupFiles".to_string(),
        Run: Some(no_op),
        ..Default::default()
    };
    command
        .Flags()
        .String("command", "", "Backup or restore command");
    command.SetArgs(vec!["--".to_string(), "--unknown=value".to_string()]);

    assert!(command.Execute().is_ok());
}

/// An explicitly empty SetArgs overrides the process arguments in Cobra.
#[test]
fn command_stub_preserves_explicitly_empty_args() {
    static SEEN_ARGS: AtomicU64 = AtomicU64::new(u64::MAX);

    fn record_args(_cmd: &mut Command, args: &[String]) {
        SEEN_ARGS.store(args.len() as u64, Ordering::SeqCst);
    }

    SEEN_ARGS.store(u64::MAX, Ordering::SeqCst);
    let mut command = Command {
        Use: "validateBackupFiles".to_string(),
        Run: Some(record_args),
        ..Default::default()
    };
    command.SetArgs(Vec::new());

    assert!(command.Execute().is_ok());
    assert_eq!(SEEN_ARGS.load(Ordering::SeqCst), 0);
}
