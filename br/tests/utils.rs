// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc. Licensed under Apache-2.0.

//! Utility commands for backup and restore — mirrors `br/tests/utils.go`.
//!
//! 备份/恢复测试辅助命令，对齐 Go `br/tests/utils.go`。
//! 入口注册 cobra 风格 `validateBackupFiles` 子命令。
//! 仅在完整 backup 或 `restore point` 完成后校验存储目录中的文件。
//! 日志备份本身不会立刻落齐 SST/LOG，故 parseCommand 在非目标子命令时返回 found=false。
//! SST 用文件末尾 8 字节小端魔数识别；同时接受 legacy 与 block-based 两种 RocksDB 表。
//! `.log` 用 zstd 解码器试读 1 字节：能解出则视作未加密压缩流。
//! 加密参数非空时期望“全部不像明文 SST/LOG”；为空时期望全部可识别为明文。
//! 混杂加密结果一律失败，避免部分密文漏检。
//! 命令行用空白拆词解析 `-s`/`--storage`，避免为集成命令声明完整 cobra flag 集。
//! 仅接受 `local://` 前缀的存储路径并剥掉前缀后做本地目录遍历。
//! 校验失败或目录不可遍历时 `process_exit(1)`，供 shell 用例捕获。
//! 测试构建下 `process_exit` 改为 panic，便于单元测试断言退出码。
//! `run_validate_for_test` 组装子命令参数，绕过真实进程 argv。
//! 遍历跳过非 `.sst`/`.log` 文件；打开/读错向上打印后失败。
//! 本模块是测试二进制辅助，不是生产 BR CLI。
//! walk 失败文案对齐 Go `no such file or directory`。
//! SST 魔数比较使用小端 u64，与 RocksDB footer 布局一致。
//! 加密启发式：能识别明文魔数/能 zstd 解压 ⇒ 视作未加密。
//! `local://` 以外的存储 scheme 直接跳过校验，避免误扫远程路径。
//! `restore` 必须紧跟 `point`，单独 `restore` 不会触发校验。
//! 空 `--command` 先 Usage 再 exit，避免静默失败。

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use zstd::stream::read::Decoder as ZstdDecoder;

use crate::stubs::{Command, Error};

/// 子命令名，与 Go 常量一致。
pub const cmdValidateBackupFiles: &str = "validateBackupFiles";
/// 快照备份数据文件后缀。
pub const extSST: &str = ".sst";
/// 日志备份数据文件后缀。
pub const extLOG: &str = ".log";

/// RocksDB / TiKV SST 表尾魔数（小端）。
/// SST magic numbers from RocksDB / TiKV (little-endian footer).
/// legacy 与 block-based 任一命中即视为明文 SST。
const SST_MAGIC_LEGACY: u64 = 0xdb4775248b80fb57;
const SST_MAGIC_BLOCK: u64 = 0x88e241b785f4cff7;

/// 二进制入口：挂载 `validateBackupFiles` 后执行。
/// Binary entry: cobra-style root with `validateBackupFiles`.
/// Execute 失败打印错误并以码 1 退出，对齐 Go main。
pub fn main() {
    let mut root_cmd = Command {
        Use: "utils".to_string(),
        Short: "Utility commands for backup and restore".to_string(),
        ..Default::default()
    };

    let mut validate_cmd = Command {
        Use: cmdValidateBackupFiles.to_string(),
        Short: "Validate backup files".to_string(),
        Run: Some(runValidateBackupFiles),
        ..Default::default()
    };

    // 两个 String flag：完整 CLI 文本与加密参数。
    validate_cmd
        .Flags()
        .String("command", "", "Backup or restore command");
    validate_cmd
        .Flags()
        .String("encryption", "", "Encryption argument");

    root_cmd.AddCommand(validate_cmd);

    // 解析失败或子命令错误 → 非零退出供 shell 捕获。
    if let Err(err) = root_cmd.Execute() {
        println!("{err}");
        process_exit(1);
    }
}

/// `validateBackupFiles` 回调：解析 --command/--encryption 并校验目录。
/// Run callback for `validateBackupFiles` — mirrors Go `runValidateBackupFiles`.
/// 解析 command/encryption，失败则 process_exit(1)。
pub fn runValidateBackupFiles(cmd: &mut Command, _args: &[String]) {
    let command = cmd.Flags().GetString("command").unwrap_or_default();
    let encryption_arg = cmd.Flags().GetString("encryption").unwrap_or_default();

    // 缺少完整命令时打印用法并以非零退出。
    if command.is_empty() {
        println!("Please provide the full backup or restore command using --command flag");
        if cmd.Usage().is_err() {
            println!("Usage error");
            return;
        }
        process_exit(1);
    }

    let (storage_path, found) = parseCommand(&command);
    // 非 backup / restore point：无需校验（例如纯 log-backup）。
    // doesn't need to validate if it's not doing backup/restore
    if !found {
        println!("No need to validate");
        return;
    }

    println!("Validating files in: {storage_path}");
    // 目录级加密一致性失败时同样非零退出。
    if !checkCompressionAndEncryption(&storage_path, &encryption_arg) {
        println!("validation failed");
        process_exit(1);
    }
}

/// 从完整 CLI 文本提取 local 存储路径；仅 backup / restore point 返回 found。
/// Parses the command and only returns the storage path if it's a full backup or restore point
/// as full backup will have backup files ready in the storage path after returning from the command
/// and log backup will not, so we can only use restore point to validate.
/// 仅 `backup full` / `restore point` + `local://` 返回 (path,true)。
pub fn parseCommand(cmd: &str) -> (String, bool) {
    // 不用完整 cobra：集成命令 flag 过多，拆词足够且与 Go 一致。
    // not using cobra since it has to define all the possible flags otherwise will report parsing error
    let args: Vec<&str> = cmd.split_whitespace().collect();

    let mut has_backup_or_restore_point = false;
    let mut storage_path = String::new();

    let mut i = 0;
    while i < args.len() {
        let arg = args[i];
        if arg == "backup" {
            has_backup_or_restore_point = true;
            i += 1;
            continue;
        }
        // 必须是 `restore point` 两词，单独 restore 不算。
        if i < args.len() - 1 && arg == "restore" && args[i + 1] == "point" {
            has_backup_or_restore_point = true;
            i += 1;
            continue;
        }

        // 支持 -s/--storage 与 = 内联写法。
        // check for storage path in various formats
        if arg == "-s" || arg == "--storage" {
            if i + 1 < args.len() {
                storage_path = args[i + 1].to_string();
                i += 1; // skip the next arg since we consumed it
            }
        } else if let Some(v) = arg.strip_prefix("--storage=") {
            storage_path = v.to_string();
        } else if let Some(v) = arg.strip_prefix("-s=") {
            storage_path = v.to_string();
        }
        i += 1;
    }

    // 仅 local:// 可在测试机直接 walk；其它协议跳过。
    if let Some(stripped) = storage_path.strip_prefix("local://") {
        storage_path = stripped.to_string();
        if has_backup_or_restore_point && !storage_path.is_empty() {
            return (storage_path, true);
        }
    }
    (String::new(), false)
}

/// 遍历目录，按 encryptionArg 期望检查 SST/LOG 是否“像明文”。
/// Walks `dir` and validates SST/LOG encryption expectations against `encryptionArg`.
/// 加密参数非空期望“全不似明文”；为空期望“全可识别明文”；混杂失败。
pub fn checkCompressionAndEncryption(dir: &str, encryption_arg: &str) -> bool {
    let mut all_encrypted = true;
    let mut all_unencrypted = true;
    let mut _total_files = 0usize;

    let walk_result = walk_dir(Path::new(dir), &mut |path| {
        let path_str = path.to_string_lossy();
        if path_str.ends_with(extSST) {
            _total_files += 1;
            match isLikelySSTFile(path) {
                Ok(is_valid_sst) => {
                    if is_valid_sst {
                        all_encrypted = false;
                    } else {
                        all_unencrypted = false;
                    }
                    Ok(())
                }
                Err(err) => {
                    println!("Error checking SST file {path_str}: {err}");
                    Err(err)
                }
            }
        } else if path_str.ends_with(extLOG) {
            _total_files += 1;
            match isZstdCompressed(path) {
                Ok(is_compressed) => {
                    if is_compressed {
                        all_encrypted = false;
                    } else {
                        all_unencrypted = false;
                    }
                    Ok(())
                }
                Err(err) => {
                    println!("Error checking if file is encrypted {path_str}: {err}");
                    Err(err)
                }
            }
        } else {
            Ok(())
        }
    });

    if let Err(err) = walk_result {
        println!("Error walking through directory: {err}");
        process_exit(1);
    }

    // 开启加密：期望没有任何文件仍像明文 SST/LOG。
    // handle with encryption case
    if !encryption_arg.is_empty() {
        if all_encrypted {
            println!("All files in {dir} are encrypted, as expected with encryption");
            return true;
        }
        println!(
            "Error: Some files in {dir} are not encrypted, which is unexpected with encryption"
        );
        return false;
    }

    // 未加密：期望全部可识别；全密文或混杂都失败。
    // handle without encryption case
    if all_unencrypted {
        println!("All files in {dir} are not encrypted, as expected without encryption");
        true
    } else if all_encrypted {
        println!("Error: All files in {dir} are encrypted, which is unexpected without encryption");
        false
    } else {
        println!("Error: Mixed encryption in {dir}. Some files are encrypted, some are not.");
        false
    }
}

/// 判断路径是否像 zstd 流；打开失败上抛，解码失败视为非压缩。
/// Returns whether `file_path` looks like a zstd-compressed stream.
/// Open errors propagate; decode failures mean "not compressed".
/// 试读 1 字节；能解出视为未加密压缩日志。
pub fn isZstdCompressed(file_path: impl AsRef<Path>) -> io::Result<bool> {
    let file = File::open(file_path)?;
    let mut decoder = match ZstdDecoder::new(file) {
        Ok(d) => d,
        Err(_) => return Ok(false), // Not compressed or error in compression
    };

    // 试读 1 字节：Go 侧任意 nil 错误即视为已压缩。
    // Try to read a small amount of data (Go: any nil error ⇒ compressed)
    let mut buf = [0u8; 1];
    match decoder.read(&mut buf) {
        Ok(bytes_read) => Ok(bytes_read > 0),
        Err(_) => Ok(false), // Not compressed or error in decompression
    }
}

/// 读取表尾 8 字节，匹配 RocksDB/TiKV SST 魔数。
/// Returns whether footer magic matches a RocksDB/TiKV SST table.
/// seek 末尾 8 字节比较两魔数之一。
pub fn isLikelySSTFile(file_path: impl AsRef<Path>) -> io::Result<bool> {
    let mut file = File::open(file_path)?;

    // Seek to 8 bytes from the end of the file
    file.seek(SeekFrom::End(-8))?;

    // Read the last 8 bytes
    let mut footer = [0u8; 8];
    file.read_exact(&mut footer)?;

    // Check for SST magic number (kLegacyBlockBasedTableMagicNumber)
    // or (kBlockBasedTableMagicNumber)
    let magic_number = u64::from_le_bytes(footer);
    Ok(magic_number == SST_MAGIC_LEGACY || magic_number == SST_MAGIC_BLOCK)
}

/// 目录不存在时返回 NotFound，对齐 Go 文案。
/// 递归遍历目录，对每个文件调用 visit。
fn walk_dir(dir: &Path, visit: &mut dyn FnMut(&Path) -> io::Result<()>) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(dir) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("no such file or directory: {}", dir.display()),
            ));
        }
        Err(err) => return Err(err),
    };
    if metadata.is_dir() {
        walk_dir_inner(dir, visit)
    } else {
        visit(dir)
    }
}

/// 深度优先遍历；目录递归、文件交给 visit。
/// 实际递归实现；跳过非文件/目录项错误上抛。
fn walk_dir_inner(dir: &Path, visit: &mut dyn FnMut(&Path) -> io::Result<()>) -> io::Result<()> {
    // filepath.Walk reads each directory in lexical order.
    let mut entries = fs::read_dir(dir)?.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        // filepath.Walk uses Lstat and therefore does not descend into symlinks.
        let meta = fs::symlink_metadata(&path)?;
        if meta.is_dir() {
            walk_dir_inner(&path, visit)?;
        } else {
            visit(&path)?;
        }
    }
    Ok(())
}

/// 生产路径真正 exit；测试路径 panic 以便断言。
/// 退出进程；测试构建可改为 panic 以便断言。
fn process_exit(code: i32) -> ! {
    #[cfg(test)]
    {
        panic!("process_exit:{code}");
    }
    #[cfg(not(test))]
    {
        std::process::exit(code);
    }
}

/// 单测辅助：不依赖进程 argv 直接跑校验子命令。
/// Test helper: build a validate command and run it without process args.
#[cfg(test)]
pub fn run_validate_for_test(command: &str, encryption: &str) -> Result<(), Error> {
    let mut root = Command {
        Use: "utils".to_string(),
        Short: "Utility commands for backup and restore".to_string(),
        ..Default::default()
    };
    let mut validate = Command {
        Use: cmdValidateBackupFiles.to_string(),
        Short: "Validate backup files".to_string(),
        Run: Some(runValidateBackupFiles),
        ..Default::default()
    };
    validate
        .Flags()
        .String("command", "", "Backup or restore command");
    validate
        .Flags()
        .String("encryption", "", "Encryption argument");
    root.AddCommand(validate);
    root.SetArgs(vec![
        cmdValidateBackupFiles.to_string(),
        format!("--command={command}"),
        format!("--encryption={encryption}"),
    ]);
    root.Execute()
}
