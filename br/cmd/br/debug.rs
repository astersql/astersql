// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! 隐藏的 `br debug` / `validate` 命令集合，对齐 `br/cmd/br/debug.go`。
//! 这些命令主要服务于离线排障、备份元数据检查、兼容旧版工作流
//! 以及人工分析日志备份内容，因此默认不暴露在普通帮助信息里。
//! 模块本身不参与常规备份路径，而是把多个独立的小工具挂到同一棵命令树下，
//! 以便在需要时复用 BR 的公共初始化、存储访问与元数据解析能力。

use std::sync::Arc;

use sha2::{Digest, Sha256};

use astersql_br_pkg_task::stubs::backuppb::BackupMeta as TaskBackupMeta;
use astersql_br_pkg_task::{Config, GetKeepalive, GetStorage, NewMgr, ReadBackupMeta};

use crate::cmd::{GetDefaultContext, Init, log_arguments_for, tidbGlue};
use crate::stubs::backuppb::File;
use crate::stubs::metautil::{self, TableInfo};
use crate::stubs::rtree;
use crate::stubs::stream_search::{NewStartWithComparator, NewStreamBackupSearch};
use crate::stubs::*;

/// Resolve a protobuf-style backup-meta field without maintaining a partial
/// hand-written allow-list. The legacy kebab-case version names are retained
/// for compatibility with older BR releases.
pub(crate) fn backup_meta_field(meta: &TaskBackupMeta, requested: &str) -> Option<String> {
    let value = serde_json::to_value(meta).ok()?;
    backup_meta_json_field(&value, requested)
}

fn backup_meta_json_field(meta: &serde_json::Value, requested: &str) -> Option<String> {
    let field = match requested {
        "start-version" => "StartVersion",
        "end-version" => "EndVersion",
        other => other,
    };
    let value = meta.as_object()?.get(field)?;
    Some(match value {
        serde_json::Value::String(value) => value.clone(),
        serde_json::Value::Null => "<nil>".to_string(),
        value => value.to_string(),
    })
}

fn read_backup_meta_json(
    storage: &dyn astersql_br_pkg_task::stubs::Storage,
    cfg: &Config,
) -> Result<serde_json::Value> {
    let data = storage.ReadFile(metautil::MetaFile).map_err(Error::from)?;
    let body = if cfg.CipherInfo.CipherType
        == astersql_br_pkg_task::stubs::encryptionpb::EncryptionMethod::PLAINTEXT
    {
        data.as_slice()
    } else {
        data.get(16..)
            .ok_or_else(|| Error::new("backupmeta is shorter than cipher IV"))?
    };
    serde_json::from_slice(body).map_err(|error| Error::new(error.to_string()))
}

/// NewDebugCommand return a debug subcommand.
///
/// 中文补充：顶层 `debug` 命令是一个隐藏入口，
/// 同时保留 `validate` 别名以兼容旧版本 BR 的调用方式。
pub fn NewDebugCommand() -> Command {
    let mut meta = Command {
        Use: "debug <subcommand>".into(),
        Short: "commands to check/debug backup data".into(),
        SilenceUsage: false,
        Hidden: true,
        // 老脚本可能直接调用 `br validate ...`，这里保留别名避免升级后命令失效。
        Aliases: vec!["validate".into()],
        ..Default::default()
    };
    meta.PersistentPreRunE = Some(Arc::new(|c, _args| {
        // 调试子命令依然共享标准初始化流程，保证日志、环境变量和参数记录行为一致。
        Init(c)?;
        build::LogInfo(build::BR);
        logutil::LogEnvVariables();
        log_arguments_for(c);
        Ok(())
    }));
    meta.AddCommand(vec![
        newCheckSumCommand(),
        newBackupMetaCommand(),
        decodeBackupMetaCommand(),
        encodeBackupMetaCommand(),
        setPDConfigCommand(),
        searchStreamBackupCommand(),
    ]);
    meta
}

/// 构造 `debug checksum` 子命令。
///
/// 它会遍历备份元数据中记录的所有文件，重新计算 sha256 并汇总表级统计量，
/// 用于确认外部存储中的备份文件没有被额外改写。
fn newCheckSumCommand() -> Command {
    let mut command = Command {
        Use: "checksum".into(),
        Short: "check the backup data".into(),
        no_args: true,
        Hidden: true,
        ..Default::default()
    };
    command.RunE = Some(Arc::new(|cmd, _| {
        let _ctx = GetDefaultContext();
        let flags = effective_task_flags(cmd);
        let mut cfg = Config::default();
        cfg.ParseFromFlags(&flags).map_err(Error::from)?;

        // 先验证主 backupmeta 可读，再从调试侧车文件中拿表与文件清单。
        let (_backend, s) = GetStorage(&cfg.Storage, &cfg).map_err(Error::from)?;
        let (_u, _backupMeta) =
            ReadBackupMeta(metautil::MetaFile, &cfg, s.as_ref()).map_err(Error::from)?;
        // Load table/file inventory from tables.json sidecar (test/debug harness).
        let dbs = metautil::LoadBackupTablesFromStorage(s.as_ref())?;

        for db in dbs.values() {
            for tbl in &db.Tables {
                // 这里沿用 Go 版的 XOR/求和统计方式，方便和 schema 中记录的摘要做人工对比。
                let mut calCRC64 = 0u64;
                let mut totalKVs = 0u64;
                let mut totalBytes = 0u64;
                for files in tbl.FilesOfPhysicals.values() {
                    // 一个逻辑表可能映射到多个 physical table/partition，因此这里先按 physical 聚合再展开。
                    for file in files {
                        calCRC64 ^= file.Crc64Xor;
                        totalKVs += file.GetTotalKvs();
                        totalBytes += file.GetTotalBytes();
                        log::Info(
                            "file info",
                            &[
                                (
                                    "table",
                                    tbl.Info
                                        .as_ref()
                                        .map_or_else(String::new, |i| i.Name.O.clone()),
                                ),
                                ("file", file.GetName().to_string()),
                                ("crc64xor", file.GetCrc64Xor().to_string()),
                                ("totalKvs", file.GetTotalKvs().to_string()),
                                ("totalBytes", file.GetTotalBytes().to_string()),
                                ("startVersion", file.GetStartVersion().to_string()),
                                ("endVersion", file.GetEndVersion().to_string()),
                                ("startKey", hex::encode(file.GetStartKey())),
                                ("endKey", hex::encode(file.GetEndKey())),
                            ],
                        );

                        // 直接读取对象内容计算 sha256，避免只依赖 metadata 中的声明值。
                        let data = s.ReadFile(&file.Name).map_err(Error::from)?;
                        let digest = Sha256::digest(&data);
                        if digest.as_slice() != file.Sha256.as_slice() {
                            return Err(berrors::checksum_mismatch(format!(
                                "\nbackup data checksum failed: {} may be changed\ncalculated sha256 is {},\norigin sha256 is {}",
                                file.Name,
                                hex::encode(digest),
                                hex::encode(&file.Sha256)
                            )));
                        }
                    }
                }
                if tbl.Info.is_none() {
                    // 空 schema 只打印数据库信息，避免在调试输出中伪造不存在的表摘要。
                    log::Info("table info(empty)", &[("db", db.Info.Name.O.clone())]);
                } else {
                    log::Info(
                        "table info",
                        &[
                            ("table", tbl.Info.as_ref().unwrap().Name.O.clone()),
                            ("CRC64", calCRC64.to_string()),
                            ("totalKvs", totalKVs.to_string()),
                            ("totalBytes", totalBytes.to_string()),
                            ("schemaTotalKvs", tbl.TotalKvs.to_string()),
                            ("schemaTotalBytes", tbl.TotalBytes.to_string()),
                            ("schemaCRC64", tbl.Crc64Xor.to_string()),
                        ],
                    );
                }
            }
        }
        cmd.Println("backup data checksum succeed!");
        Ok(())
    }));
    command
}

/// 构造 `debug backupmeta` 命令分组。
///
/// 当前仅挂载 `validate` 子命令，后续其他 backupmeta 工具也可继续归档在此分组下。
fn newBackupMetaCommand() -> Command {
    let mut command = Command {
        Use: "backupmeta".into(),
        Short: "utilities of backupmeta".into(),
        // 这里保留 usage 输出，便于用户发现下层 validate 子命令参数。
        SilenceUsage: false,
        ..Default::default()
    };
    command.AddCommand(vec![newBackupMetaValidateCommand()]);
    command
}

/// 构造 `debug backupmeta validate` 子命令。
///
/// 核心目标是复算文件范围与 rewrite rule，确认备份元数据
/// 在恢复阶段不会出现 key range 重叠或重写规则缺失。
fn newBackupMetaValidateCommand() -> Command {
    let mut command = Command {
        Use: "validate".into(),
        Short: "validate key range and rewrite rules of backupmeta".into(),
        ..Default::default()
    };
    command.Flags().DefineUint64("offset", 0);
    command.RunE = Some(Arc::new(|cmd, _| {
        let _ctx = GetDefaultContext();
        let flags = effective_task_flags(cmd);
        // 某些恢复场景会预留一段 table ID，因此调试命令允许人工注入 offset。
        let tableIDOffset = flags.GetUint64("offset").unwrap_or(0);

        let mut cfg = Config::default();
        cfg.ParseFromFlags(&flags).map_err(Error::from)?;
        let (_backend, s) = GetStorage(&cfg.Storage, &cfg).map_err(Error::from)?;
        if let Err(e) = ReadBackupMeta(metautil::MetaFile, &cfg, s.as_ref()) {
            log::Error("read backupmeta failed", &[zap::Error(&e.clone().into())]);
            return Err(Error::from(e));
        }
        let dbs = metautil::LoadBackupTablesFromStorage(s.as_ref()).map_err(|e| {
            log::Error("load tables failed", &[zap::Error(&e)]);
            e
        })?;

        let mut files: Vec<File> = Vec::new();
        let mut tables = Vec::new();
        for db in dbs.values() {
            // 这里按库展开而不是边读边校验，是为了先拿到全局文件集合构建 range tree。
            for table in &db.Tables {
                for fs in table.FilesOfPhysicals.values() {
                    // 先拍平成单一文件列表，后面才能统一做区间重叠校验。
                    files.extend(fs.iter().cloned());
                }
            }
            tables.extend(db.Tables.iter().cloned());
        }

        let mut rangeTree = rtree::RangeTree::new();
        for file in &files {
            // 若插入时返回已有区间，说明 backupmeta 内部存在重叠文件范围。
            if let Some(out) = rangeTree.InsertRange(rtree::file_range(file)) {
                log::Error("file ranges overlapped", &[("out", out.to_string())]);
            }
        }

        let tableIDAllocator = mockid::NewIDAllocator();
        for _ in 0..tableIDOffset {
            let _ = tableIDAllocator.Alloc();
        }
        // 这里模拟“恢复时重新分配表/索引/分区 ID”的过程，再让任务层生成 rewrite rules。
        let mut rewriteRules = restoreutils::RewriteRules { Data: Vec::new() };

        for table in &tables {
            let Some(info) = &table.Info else {
                // 没有 schema 的条目代表空数据库，占位即可，无需生成 rewrite rules。
                continue;
            };
            let indexIDAllocator = mockid::NewIDAllocator();
            let (tableID, _) = tableIDAllocator.Alloc();
            // 新表结构只保留 rewrite 规则生成所需的核心字段，避免把无关 schema 噪音带入校验。
            let mut newTable = TableInfo {
                ID: tableID as i64,
                Name: info.Name.clone(),
                Indices: Vec::with_capacity(info.Indices.len()),
                Partition: None,
            };
            for indexInfo in &info.Indices {
                let (indexID, _) = indexIDAllocator.Alloc();
                newTable.Indices.push(metautil::IndexInfo {
                    ID: indexID as i64,
                    Name: indexInfo.Name.clone(),
                });
            }
            if let Some(part) = &info.Partition {
                // 分区表必须为每个新分区单独分配 ID，否则 rewrite key 前缀会失真。
                newTable.Partition = Some(metautil::PartitionInfo {
                    Definitions: Vec::with_capacity(part.Definitions.len()),
                });
                for old in &part.Definitions {
                    let (partitionID, _) = tableIDAllocator.Alloc();
                    if let Some(p) = newTable.Partition.as_mut() {
                        p.Definitions.push(metautil::PartitionDefinition {
                            ID: partitionID as i64,
                            Name: old.Name.clone(),
                        });
                    }
                }
            }

            // 直接复用恢复工具层的规则生成器，确保调试结果与真实恢复路径一致。
            let rules = restoreutils::GetRewriteRules(&newTable, info, 0, true);
            rewriteRules.Data.extend(rules.Data);
        }

        for file in &files {
            // 校验每个文件都能被某条 rewrite rule 正确覆盖。
            restoreutils::ValidateFileRewriteRule(file, &rewriteRules)?;
        }
        cmd.Println("Check backupmeta done");
        Ok(())
    }));
    command
}

/// 构造 `debug decode` 子命令。
///
/// 该命令既支持把完整 backupmeta 导出为 JSON，
/// 也支持按字段读取少量关键信息，方便脚本化排查。
fn decodeBackupMetaCommand() -> Command {
    let mut decodeBackupMetaCmd = Command {
        Use: "decode".into(),
        Short: "decode backupmeta to json".into(),
        // decode 只接受 flag，不接受额外位置参数，避免误把字段名写成参数。
        no_args: true,
        ..Default::default()
    };
    decodeBackupMetaCmd.Flags().DefineString("field", "");
    decodeBackupMetaCmd.RunE = Some(Arc::new(|cmd, _args| {
        let _ctx = GetDefaultContext();
        let flags = effective_task_flags(cmd);
        let mut cfg = Config::default();
        cfg.ParseFromFlags(&flags).map_err(Error::from)?;
        let (_backend, s) = GetStorage(&cfg.Storage, &cfg).map_err(Error::from)?;
        let (_u, backupMeta) =
            ReadBackupMeta(metautil::MetaFile, &cfg, s.as_ref()).map_err(Error::from)?;
        let backupMetaJSON = read_backup_meta_json(s.as_ref(), &cfg)?;

        let mut fieldName = flags.GetString("field").unwrap_or_default();
        if fieldName.is_empty() {
            // Go 会先展开 backupmeta 引用的子元数据和统计文件，再写完整 JSON。
            // 共享 task 类型只承载执行路径所需字段，因此索引从无损 JSON 文档读取。
            let indexed: crate::stubs::backuppb::BackupMeta =
                serde_json::from_value(backupMetaJSON.clone())
                    .map_err(|error| Error::new(error.to_string()))?;
            metautil::DecodeMetaFile(s.as_ref(), &cfg.CipherInfo, &indexed.FileIndex)?;
            metautil::DecodeMetaFile(s.as_ref(), &cfg.CipherInfo, &indexed.RawRangeIndex)?;
            metautil::DecodeMetaFile(s.as_ref(), &cfg.CipherInfo, &indexed.SchemaIndex)?;
            metautil::DecodeStatsFile(s.as_ref(), &cfg.CipherInfo, &indexed.Schemas)?;

            let encoded = serde_json::to_vec(&backupMetaJSON)
                .map_err(|error| Error::new(error.to_string()))?;
            s.WriteFile(metautil::MetaJSONFile, &encoded)
                .map_err(Error::from)?;
            cmd.Printf(format!(
                "backupmeta decoded at {}/{}\n",
                cfg.Storage,
                metautil::MetaJSONFile
            ));
            return Ok(());
        }

        if let Some(value) = backup_meta_json_field(&backupMetaJSON, &fieldName) {
            cmd.Printf(format!("{value}\n"));
        } else {
            // 与 Go 版类似，未知字段不会报错退出，方便脚本逐个探测兼容性。
            cmd.Printf(format!("field '{fieldName}' not found\n"));
        }
        // 显式保留任务层类型约束，确保这里读到的就是共享的 backupmeta 结构。
        let _: &TaskBackupMeta = &backupMeta;
        Ok(())
    }));
    decodeBackupMetaCmd
}

/// 构造 `debug encode` 子命令。
///
/// 它把 JSON 形式的 backupmeta 重新编码并按当前加密配置写回外部存储，
/// 常用于离线编辑元数据后的回写验证。
fn encodeBackupMetaCommand() -> Command {
    let mut encodeBackupMetaCmd = Command {
        Use: "encode".into(),
        Short: "encode backupmeta json file to backupmeta".into(),
        // 与 decode 对称，所有输入都来自外部存储与 flag，而不是命令参数。
        no_args: true,
        ..Default::default()
    };
    encodeBackupMetaCmd.RunE = Some(Arc::new(|_cmd, _args| {
        let _ctx = GetDefaultContext();
        let flags = effective_task_flags(_cmd);
        let mut cfg = Config::default();
        cfg.ParseFromFlags(&flags).map_err(Error::from)?;
        let (_backend, s) = GetStorage(&cfg.Storage, &cfg).map_err(Error::from)?;

        let metaData = s.ReadFile(metautil::MetaJSONFile).map_err(Error::from)?;
        // Prefer task BackupMeta JSON; also accept cmd-local meta with Version.
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&metaData) {
            // v2 元数据需要额外索引文件协同编码，当前 slim 版本尚未实现该路径。
            if v.get("Version").and_then(|x| x.as_i64()) == Some(metautil::MetaV2 as i64) {
                return Err(Error::Errorf("encoding backupmeta v2 is unimplemented"));
            }
        }
        // Validate the command-relevant schema while retaining fields that are
        // represented only by the full backupmeta document (indexes, schemas,
        // and forward-compatible protobuf additions).
        let _: TaskBackupMeta =
            serde_json::from_slice(&metaData).map_err(|e| Error::new(e.to_string()))?;
        let backupMeta = metaData;

        let mut fileName = metautil::MetaFile.to_string();
        if s.FileExists(&fileName).unwrap_or(false) {
            // 为了保护原始元数据，默认改名输出而不是覆盖已有 backupmeta。
            fileName += "_from_json";
        }

        // 写回前仍需走和正式备份一致的加密封装，保持存储格式兼容。
        let (encryptedContent, iv) = metautil::Encrypt(&backupMeta, &cfg.CipherInfo)?;
        let mut out = iv;
        // 输出格式保持为 `iv + ciphertext`，这样现有读取逻辑无需额外适配。
        out.extend_from_slice(&encryptedContent);
        s.WriteFile(&fileName, &out).map_err(Error::from)?;
        Ok(())
    }));
    encodeBackupMetaCmd
}

/// 构造“恢复 PD 调度配置为默认值”的调试命令。
///
/// BR 在某些恢复路径会临时调整 PD 参数，这个子命令用于人工兜底回滚配置。
fn setPDConfigCommand() -> Command {
    let mut pdConfigCmd = Command {
        Use: "reset-pd-config-as-default".into(),
        Short: "reset pd config adjusted by BR to default value".into(),
        // 这是危险性较高的调试命令，因此显式禁止携带多余参数。
        no_args: true,
        ..Default::default()
    };
    pdConfigCmd.RunE = Some(Arc::new(|cmd, _args| {
        let _ctx = GetDefaultContext();
        let flags = effective_task_flags(cmd);
        let mut cfg = Config::default();
        cfg.ParseFromFlags(&flags).map_err(Error::from)?;

        let g = tidbGlue().lock().unwrap();
        // 这里复用任务层 Mgr 初始化，确保 PD/TLS/版本校验逻辑与正式命令一致。
        let mgr = NewMgr(
            g.as_task(),
            &cfg.KeyspaceName,
            &cfg.PD,
            &cfg.TLS,
            GetKeepalive(&cfg),
            cfg.CheckRequirements,
            false,
            conn::NormalVersionChecker,
        )
        .map_err(Error::from)?;
        if let Err(e) = UpdatePDScheduleConfig(mgr.as_ref()) {
            // 先关闭连接再回报错误，避免调试失败时残留后台资源。
            mgr.Close();
            return Err(Error::Annotate(e.msg, "fail to update PD merge config"));
        }
        // slim trait 没有析构钩子，手动关闭连接与 Go 版 defer 对齐。
        mgr.Close();
        log::Info("add pd configs succeed", &[]);
        Ok(())
    }));
    pdConfigCmd
}

/// Forward PD schedule reset through the manager boundary.
pub fn UpdatePDScheduleConfig(mgr: &dyn astersql_br_pkg_task::stubs::Mgr) -> Result<()> {
    mgr.UpdatePDScheduleConfig().map_err(Error::from)
}

/// 构造 `debug search-log-backup` 子命令。
///
/// 它按 key 与时间窗口搜索日志备份中的 KV 记录，
/// 方便在 PITR/stream backup 排障时快速确认某个 key 的变更轨迹。
fn searchStreamBackupCommand() -> Command {
    let mut searchBackupCMD = Command {
        Use: "search-log-backup".into(),
        Short: "search log backup by key".into(),
        // 搜索条件全部来自 flag，避免把十六进制 key 当作位置参数后再做二次解析。
        no_args: true,
        ..Default::default()
    };
    searchBackupCMD.Flags().DefineString("search-key", "");
    searchBackupCMD.Flags().DefineUint64("start-ts", 0);
    // `end-ts` 为 0 时表示不限制上界，与任务层搜索器的默认语义保持一致。
    searchBackupCMD.Flags().DefineUint64("end-ts", 0);
    searchBackupCMD.RunE = Some(Arc::new(|cmd, _args| {
        let _ctx = GetDefaultContext();
        let flags = effective_task_flags(cmd);
        let searchKey = flags.GetString("search-key").map_err(Error::from)?;
        if searchKey.is_empty() {
            return Err(Error::new("key param can't be empty"));
        }
        // CLI 使用 hex 字符串承载原始 key，避免 shell 中直接传入二进制内容。
        let keyBytes = hex::decode(&searchKey).map_err(|e| Error::new(e.to_string()))?;
        let startTs = flags.GetUint64("start-ts").unwrap_or(0);
        let endTs = flags.GetUint64("end-ts").unwrap_or(0);

        let mut cfg = Config::default();
        cfg.ParseFromFlags(&flags).map_err(Error::from)?;
        let (_backend, s) = GetStorage(&cfg.Storage, &cfg).map_err(Error::from)?;
        let comparator = NewStartWithComparator();
        let mut bs = NewStreamBackupSearch(s, comparator, keyBytes);
        // 起止时间都为 0 时表示不设窗口，只按 key 前缀/比较器筛选。
        bs.SetStartTS(startTs);
        bs.SetEndTs(endTs);

        // 搜索结果可能为空；命令依旧输出标题，便于脚本区分“空结果”和“命令失败”。
        let kvs = bs.Search()?;
        // pretty JSON 便于人工阅读，比直接打印结构体更适合离线排障。
        let kvsBytes = serde_json::to_vec_pretty(&kvs).map_err(|e| Error::new(e.to_string()))?;
        cmd.Println("search result");
        cmd.Println(String::from_utf8_lossy(&kvsBytes));
        Ok(())
    }));
    searchBackupCMD
}
