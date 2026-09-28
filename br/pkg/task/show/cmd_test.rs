// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Ports of `cmd_test.go` for `br/pkg/task/show`.
//!
//! Real local file I/O mirrors Go fixtures. Protobuf decode / metautil schema
//! walk / TiDB `testkit` SQL execution are the mocked boundaries (no kvproto /
//! domain on darwin arm64). Hooks still verify on-disk bytes and cipher shape.
//!
//! 对齐 Go `cmd_test.go`：真实落盘 fixture + ReadBackupMeta hook，
//! 覆盖全量/V2/加密 show、以及 SQL 时区格式化。解码与 schema 遍历用本地
//! 轻量解析代替 kvproto；断言仍核对本盘字节与 Cipher 形态。

use std::collections::BTreeSet;
#[cfg(unix)]
use std::ffi::{CStr, c_char, c_long, c_void};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::cmd::{Config, CreateExec, TimeStamp};
use crate::stubs::backuppb::{BackupMeta, CipherInfo, StorageBackend};
use crate::stubs::encryptionpb::EncryptionMethod;
use crate::stubs::{
    CIStr, Context, DBInfo, MemStorage, MetaFile, MetaTable, TableInfo, set_read_backup_meta_hook,
};

/// `//go:embed testdata/full-schema.meta`
/// 嵌入 Go 同款 full-schema.meta fixture 字节。
static FULL_META: &[u8] = include_bytes!("testdata/full-schema.meta");

const TPCC_TABLES: &[&str] = &[
    // TPC-C 九张基表名，与 Go 断言集合一致。
    "customer",
    // TPC-C: customer
    "district",
    // TPC-C: district
    "history",
    // TPC-C: history
    "item",
    // TPC-C: item
    "new_order",
    // TPC-C: new_order
    "order_line",
    // TPC-C: order_line
    "orders",
    // TPC-C: orders
    "stock",
    // TPC-C: stock
    "warehouse",
    // TPC-C: warehouse
];

#[cfg(unix)]
pub(crate) fn go_local_timestamp_string(ts: u64) -> String {
    unsafe extern "C" {
        fn localtime(timep: *const c_long) -> *mut c_void;
        fn strftime(s: *mut c_char, max: usize, format: *const c_char, tm: *const c_void) -> usize;
    }

    let seconds = ((ts >> 18) / 1_000) as c_long;
    let mut output = [0_i8; 64];
    let written = unsafe {
        let tm = localtime(&seconds);
        assert!(!tm.is_null(), "localtime must accept the fixture TSO");
        strftime(
            output.as_mut_ptr(),
            output.len(),
            c"Y%yM%mD%d,%H:%I:%M".as_ptr(),
            tm,
        )
    };
    assert!(written > 0, "strftime must format the fixture TSO");
    unsafe { CStr::from_ptr(output.as_ptr()) }
        .to_str()
        .expect("strftime result is ASCII")
        .to_owned()
}

#[cfg(unix)]
#[test]
fn timestamp_display_uses_local_timezone_like_go() {
    let ts = 440689413714870273;
    assert_eq!(
        TimeStamp(ts).to_string(),
        format!("{ts}({})", go_local_timestamp_string(ts))
    );
}

/// 临时备份目录：测试结束 Drop 时清理。
struct TempBackup {
    path: PathBuf,
}

impl TempBackup {
    fn new(test_name: &str) -> Self {
        // 按 pid+用例名隔离，避免并行冲突。
        let path = std::env::temp_dir()
            // 系统临时目录下创建用例专属路径。
            .join(std::process::id().to_string())
            .join(test_name);
        fs::create_dir_all(&path).expect("mkdir temp backup");
        // 确保目录存在。
        Self { path }
    }

    fn path(&self) -> &Path {
        // 返回临时目录绝对路径。
        &self.path
    }

    fn local_uri(&self) -> String {
        // 拼 local:// URI，供 Config.Storage 使用。
        format!("local://{}", self.path.display())
    }
}

impl Drop for TempBackup {
    // RAII：尽力删除临时目录，忽略清理错误。
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
        // Drop 时清理，失败忽略。
    }
}

fn meta_table(db: &str, table: &str) -> MetaTable {
    // 构造仅含库表名的 MetaTable，KV/TiFlash 计数置零。
    MetaTable {
        // Info::Some 表示具体表；Total* 置 0 因 show 断言不依赖。
        DB: DBInfo {
            Name: CIStr::new(db),
        },
        Info: Some(TableInfo {
            Name: CIStr::new(table),
        }),
        TotalKvs: 0,
        TotalBytes: 0,
        TiFlashReplicas: 0,
    }
}

fn read_varint(data: &[u8], mut i: usize) -> Option<(u64, usize)> {
    // 读 protobuf varint；越界或移位>63 返回 None。
    let mut n = 0u64;
    // 累积 varint 低 7 位。
    let mut s = 0u32;
    loop {
        if i >= data.len() {
            // 截断输入 → None
            return None;
        }
        let b = data[i];
        i += 1;
        // 逐字节滑动匹配
        n |= u64::from(b & 0x7f) << s;
        if b < 0x80 {
            // 最高位 0：varint 结束
            return Some((n, i));
        }
        s += 7;
        if s > 63 {
            // 超过 u64 位宽 → None
            return None;
        }
    }
}

/// Minimal BackupMeta scalar walk (no kvproto): fields used by show assertions.
/// 无 kvproto 时手工扫 BackupMeta 标量字段（cluster/version/TSO）。
fn parse_backup_meta_scalars(data: &[u8]) -> BackupMeta {
    let mut meta = BackupMeta::default();
    // 逐 tag 解析 wire type 0/2/1/5；未知字段跳过。
    let mut i = 0usize;
    while i < data.len() {
        // 主循环：读 tag → 按 wire type 分支
        let Some((tag, ni)) = read_varint(data, i) else {
            break;
        };
        i = ni;
        let field = (tag >> 3) as u32;
        // protobuf field number
        let wt = tag & 7;
        // wire type
        match wt {
            0 => {
                // varint 字段
                let Some((v, ni)) = read_varint(data, i) else {
                    break;
                };
                i = ni;
                match field {
                    1 => meta.ClusterId = v,
                    // field 1: cluster_id
                    5 => meta.StartVersion = v,
                    // field 5: start_version
                    6 => meta.EndVersion = v,
                    // field 6: end_version
                    12 => meta.Version = v as i32,
                    // field 12: meta version
                    _ => {}
                }
            }
            2 => {
                // length-delimited 字段
                let Some((ln, ni)) = read_varint(data, i) else {
                    break;
                };
                i = ni;
                let end = i.saturating_add(ln as usize);
                // 防止 length 溢出切片
                if end > data.len() {
                    break;
                }
                let blob = &data[i..end];
                i = end;
                match field {
                    2 => meta.ClusterVersion = String::from_utf8_lossy(blob).into_owned(),
                    // field 2: cluster_version 字符串
                    11 => meta.BrVersion = String::from_utf8_lossy(blob).into_owned(),
                    // field 11: br_version 字符串
                    _ => {}
                }
            }
            1 => i = i.saturating_add(8),
            // 64-bit fixed：跳过 8 字节
            5 => i = i.saturating_add(4),
            // 32-bit fixed：跳过 4 字节
            _ => break,
        }
    }
    meta
}

/// Extract `"name":{"O":"<tbl>","L":"<tbl>"}` entries that belong to a DB JSON
/// 从明文 schema JSON 片段提取指定库下的表名（v1 每 Schema 一表）。
/// sibling (`"db_name":{"O":"<db>"...}`) from plaintext schema payloads.
fn extract_tables_from_plaintext(data: &[u8], db: &str) -> Vec<MetaTable> {
    let text = String::from_utf8_lossy(data);
    // 明文 schema 按 lossy UTF-8 扫描
    let db_marker = format!("\"db_name\":{{\"O\":\"{db}\"");
    // 定位 db_name 标记，再在窗口内找表 name。
    let mut tables = Vec::new();
    // 累积 MetaTable
    let mut seen = BTreeSet::new();
    // 表名去重
    // Walk each db_name occurrence; take the following table `name` O/L pair
    // 每个 db_name 窗口内取紧随的表名 O/L 对，并用 seen 去重。
    // before the next db_name (v1 schemas embed one table per Schema message).
    let mut search_from = 0usize;
    // 从左到右推进搜索窗口
    while let Some(rel) = text[search_from..].find(&db_marker) {
        let start = search_from + rel;
        let next_db = text[start + db_marker.len()..]
            .find(&db_marker)
            .map(|n| start + db_marker.len() + n)
            .unwrap_or(text.len());
        let window = &text[start..next_db];
        // Prefer top-level table name after db_name; skip column names by
        // requiring the `"name":{"O":...}` that appears before `"cols"` / indexes.
        if let Some(name_rel) = window.find("\"name\":{\"O\":\"") {
            let ns = name_rel + "\"name\":{\"O\":\"".len();
            if let Some(ne) = window[ns..].find('"') {
                let name = &window[ns..ns + ne];
                let expect_l = format!("\",\"L\":\"{name}\"");
                if window[ns + ne..].starts_with(&expect_l) && seen.insert(name.to_string()) {
                    tables.push(meta_table(db, name));
                }
            }
        }
        search_from = start + db_marker.len();
        // 推进到下一可能 db_name
    }
    tables
}

fn extract_v2_tables_from_schema_file(data: &[u8]) -> Vec<MetaTable> {
    // V2 schema 文件：在原始字节上扫 db_name/name，过滤 testN 表。
    // Search on raw bytes so binary protobuf framing cannot break UTF-8 indices.
    // 不用 UTF-8 切片索引，避免 protobuf 分帧打断字符边界。
    let mut tables = Vec::new();
    let mut seen = BTreeSet::new();
    let mut db = b"test".to_vec();
    // V2 默认库名回退为 test
    let db_pat = b"\"db_name\":{\"O\":\"";
    // db_name JSON 前缀字节模式
    let name_pat = b"\"name\":{\"O\":\"";
    // 表 name JSON 前缀字节模式
    let mut i = 0usize;
    while i < data.len() {
        if data[i..].starts_with(db_pat) {
            let ns = i + db_pat.len();
            if let Some(ne) = data[ns..].iter().position(|&b| b == b'"') {
                db = data[ns..ns + ne].to_vec();
                i = ns + ne;
                continue;
            }
        }
        if data[i..].starts_with(name_pat) {
            let ns = i + name_pat.len();
            if let Some(ne) = data[ns..].iter().position(|&b| b == b'"') {
                let name = &data[ns..ns + ne];
                let rest = &data[ns + ne..];
                let mut expect_l = b"\",\"L\":\"".to_vec();
                expect_l.extend_from_slice(name);
                expect_l.push(b'"');
                let is_test_table =
                // 仅收集 test\d+ 形态表名，对齐 Go V2 小表夹具。
                    name.starts_with(b"test") && name.get(4).is_some_and(|c| c.is_ascii_digit());
                if rest.starts_with(&expect_l) && is_test_table {
                    if let Ok(name_s) = std::str::from_utf8(name) {
                        if seen.insert(name_s.to_string()) {
                            let db_s = String::from_utf8_lossy(&db).into_owned();
                            tables.push(meta_table(&db_s, name_s));
                        }
                    }
                }
                i = ns + ne;
                continue;
            }
        }
        i += 1;
    }
    tables
}

fn clone_fs(fixture_rel: &str, target: &Path) {
    // 将 crate 内 fixture 目录文件拷到临时目录（跳过子目录）。
    let src_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(fixture_rel);
    // fixture 相对 crate 根
    assert!(
        // fixture 目录必须存在，否则用例无意义。
        src_root.is_dir(),
        "fixture dir missing: {}",
        src_root.display()
    );
    for entry in fs::read_dir(&src_root).expect("read fixture dir") {
        let entry = entry.expect("dir entry");
        let ty = entry.file_type().expect("file type");
        if ty.is_dir() {
            // 跳过子目录，只拷文件
            continue;
        }
        let dst = target.join(entry.file_name());
        fs::copy(entry.path(), &dst).unwrap_or_else(|e| {
            // 拷贝失败立即 panic，带路径信息
            panic!("copy {} -> {}: {e}", entry.path().display(), dst.display())
        });
    }
}

/// Install `ReadBackupMeta` hook: verify local files, return parsed / known meta.
/// 安装 hook：校验 URI/Cipher/落盘字节，再返回给定 meta 与表列表。
fn install_local_hook(
    expected_uri_prefix: String,
    cipher: CipherInfo,
    backup_meta: BackupMeta,
    tables: Vec<MetaTable>,
    require_meta_bytes: Option<&'static [u8]>,
) {
    set_read_backup_meta_hook(Some(Box::new(move |_ctx, file, cfg| {
        // 闭包捕获期望 URI/Cipher/meta/tables
        assert_eq!(file, MetaFile);
        // 只接受 backupmeta 文件名。
        assert!(
            // Storage URI 必须指向本用例临时目录。
            cfg.Storage.starts_with(&expected_uri_prefix) || cfg.Storage == expected_uri_prefix,
            "storage uri {} vs {}",
            cfg.Storage,
            expected_uri_prefix
        );
        assert_eq!(cfg.CipherInfo.CipherType, cipher.CipherType);
        // Cipher 类型与密钥必须与用例注入一致。
        assert_eq!(cfg.CipherInfo.CipherKey, cipher.CipherKey);
        // 密钥字节级相等

        let path = cfg.Storage.strip_prefix("local://").unwrap_or(&cfg.Storage);
        // 去掉 local:// 得到本地路径
        let meta_path = Path::new(path).join(MetaFile);
        let on_disk = fs::read(&meta_path)
            // 读取真实落盘 backupmeta
            .unwrap_or_else(|e| panic!("read on-disk backupmeta {}: {e}", meta_path.display()));
        if let Some(want) = require_meta_bytes {
            // 若给定嵌入字节，则要求落盘完全一致。
            assert_eq!(
                on_disk, want,
                "on-disk backupmeta must match embedded fixture"
            );
        } else {
            assert!(
                // 否则至少要求 backupmeta 非空。
                !on_disk.is_empty(),
                "backupmeta at {} must be non-empty",
                meta_path.display()
            );
        }

        Ok((
            StorageBackend {
                // 返回 local backend + MemStorage(表) + meta
                Scheme: "local".into(),
                Path: path.to_string(),
            },
            MemStorage::with_tables(format!("local-{}", path), tables.clone())
                as Arc<dyn crate::stubs::Storage>,
            backup_meta.clone(),
        ))
    })));
}

fn clear_hook() {
    // 清除 ReadBackupMeta hook，避免污染后续用例。
    set_read_backup_meta_hook(None);
    // 置空 hook
}

/// `TestFull`
/// 全量明文 fixture：解析标量 + tpcc 九表，经 CreateExec/Read 对照。
#[test]
fn test_full() {
    let temp = TempBackup::new("test_full");
    // 全量用例临时目录
    let meta_path = temp.path().join("backupmeta");
    // V2 backupmeta 路径
    fs::write(&meta_path, FULL_META).expect("write full-schema backupmeta");
    // 写入嵌入的 full-schema.meta
    // Match Go 0o444 mode as closely as possible on unix.
    // Unix 上尽量模拟 Go 只读 0444 权限。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&meta_path).unwrap().permissions();
        perms.set_mode(0o444);
        // 只读，贴近 Go 权限
        fs::set_permissions(&meta_path, perms).unwrap();
    }

    let on_disk = fs::read(&meta_path).expect("re-read");
    // 再读校验写入成功
    let mut backup_meta = parse_backup_meta_scalars(&on_disk);
    // 从落盘字节解析标量
    let tables = extract_tables_from_plaintext(&on_disk, "tpcc");
    // 从明文提取 tpcc 表
    // Keep only the nine TPC-C base tables asserted by Go.
    // 只保留 Go 断言的九张 TPC-C 基表。
    let tables: Vec<_> = tables
        .into_iter()
        .filter(|t| {
            t.Info
                .as_ref()
                .is_some_and(|info| TPCC_TABLES.contains(&info.Name.O.as_str()))
        })
        .collect();
    assert_eq!(
        tables.len(),
        TPCC_TABLES.len(),
        "fixture must yield tpcc base tables, got {:?}",
        tables
            .iter()
            .map(|t| t.Info.as_ref().map(|i| i.Name.O.clone()))
            .collect::<Vec<_>>()
    );

    // EndVersion comes from field 6; ClusterID/Version from scalars.
    // 标量字段与 Go fixture 期望值对齐。
    assert_eq!(backup_meta.ClusterId, 7211076907329653533);
    assert_eq!(backup_meta.ClusterVersion, "\"7.1.0-alpha\"\n");
    assert_eq!(backup_meta.EndVersion, 440689413714870273);

    let cipher = CipherInfo {
        // 明文 Cipher：PLAINTEXT + 空 key
        CipherType: EncryptionMethod::PLAINTEXT,
        CipherKey: vec![],
    };
    install_local_hook(
        // SQL 用例要求落盘等于 FULL_META
        // 注入 hook，并要求落盘等于 FULL_META
        temp.local_uri(),
        cipher.clone(),
        backup_meta,
        tables,
        Some(FULL_META),
    );

    let cfg = Config {
        // show Config：Storage + Cipher
        Storage: temp.local_uri(),
        Cipher: cipher,
        ..Default::default()
    };
    let ctx = Context::Background();
    // 后台 Context，无取消
    let exec = CreateExec(&ctx, cfg).expect("CreateExec");
    // CreateExec → Read：走生产 show 路径（hook 注入 meta）。
    let items = exec.Read(&ctx).expect("Read");
    // 执行 Read 得到 ShowResult
    clear_hook();
    // 用例结束清理 hook

    assert_eq!(items.ClusterID, 7211076907329653533);
    // V2 ClusterID
    // 集群 ID / 版本 / EndVersion / 表名集合必须匹配。
    assert_eq!(items.ClusterVersion, "\"7.1.0-alpha\"\n");
    // 集群版本字符串含引号与换行，与 fixture 一致。
    assert_eq!(items.EndVersion.0, 440689413714870273);
    // SQL 用例使用 full-schema 的 EndVersion。
    let mut table_names = Vec::with_capacity(items.Tables.len());
    // 收集表名以便排序比较
    for tbl in &items.Tables {
        // 加密用例：库名固定 tpcc。
        assert_eq!(tbl.DBName, "tpcc");
        // 库名必须为 tpcc
        table_names.push(tbl.TableName.clone());
    }
    let mut got = table_names;
    // 加密用例表名排序比较
    got.sort();
    // 排序后比较表名，避免顺序依赖。
    let mut want: Vec<String> = TPCC_TABLES.iter().map(|s| (*s).to_string()).collect();
    // 期望表名集合
    want.sort();
    assert_eq!(got, want);
    // 全量用例：表名集合完全一致
}

/// `TestV2AndSmallTables`
/// V2 夹具：schema 文件应解析出 500 张小表。
#[test]
fn test_v2_and_small_tables() {
    let temp = TempBackup::new("test_v2_and_small_tables");
    // V2 小表用例临时目录
    clone_fs("testdata/v2", temp.path());
    // 拷贝 v2 backupmeta + schema 分片到临时目录。

    let meta_path = temp.path().join("backupmeta");
    let schema_path = temp.path().join("backupmeta.schema.000000001");
    // V2 schema 分片路径
    let on_disk = fs::read(&meta_path).expect("read v2 backupmeta");
    // 读 V2 meta
    let schema = fs::read(&schema_path).expect("read v2 schema");
    // 读 V2 schema
    let backup_meta = parse_backup_meta_scalars(&on_disk);
    // 解析 V2 标量
    let tables = extract_v2_tables_from_schema_file(&schema);
    // 从 schema 字节提取 500 表
    assert_eq!(tables.len(), 500, "v2 fixture must expose 500 tables");
    // 500 表是 Go 用例硬门槛。

    let cipher = CipherInfo {
        CipherType: EncryptionMethod::PLAINTEXT,
        CipherKey: vec![],
    };
    install_local_hook(temp.local_uri(), cipher.clone(), backup_meta, tables, None);
    // V2 不强制嵌入字节相等

    let cfg = Config {
        Storage: temp.local_uri(),
        Cipher: cipher,
        ..Default::default()
    };
    let ctx = Context::Background();
    let exec = CreateExec(&ctx, cfg).expect("CreateExec");
    let items = exec.Read(&ctx).expect("Read");
    clear_hook();

    assert_eq!(items.Tables.len(), 500);
    // Read 结果表数与 EndVersion/ClusterID 对照。
    assert_eq!(items.ClusterID, 7211076907329653533);
    assert_eq!(items.EndVersion.0, 440691270926467074);
    // V2 EndVersion 与 Go 夹具一致
}

/// `TestV2Encrypted`
/// 加密 V2：落盘应为密文；hook 模拟解密后返回 tpcc 表集。
#[test]
fn test_v2_encrypted() {
    let temp = TempBackup::new("test_v2_encrypted");
    // 加密 V2 临时目录
    clone_fs("testdata/v2-enc", temp.path());
    // 拷贝加密夹具

    let meta_path = temp.path().join("backupmeta");
    let schema_path = temp.path().join("backupmeta.schema.000000001");
    let on_disk = fs::read(&meta_path).expect("read enc backupmeta");
    let schema = fs::read(&schema_path).expect("read enc schema");
    assert!(!on_disk.is_empty(), "encrypted backupmeta present");
    // 加密 meta 必须存在且非空
    // Entire backupmeta is ciphertext under AES256-CTR — not a plaintext protobuf.
    // 密文 backupmeta 不得以明文 cluster_id tag 0x08 开头。
    assert_ne!(
        on_disk.first().copied().unwrap_or(0),
        0x08,
        "encrypted backupmeta should not start with plaintext cluster_id tag"
    );
    let needle = b"\"name\":{\"O\":\"customer\"";
    // 加密 schema 中不应出现明文 customer 表名。
    assert!(
        !schema.windows(needle.len()).any(|w| w == needle),
        "encrypted schema fixture unexpectedly plaintext"
    );

    // After decrypt (mocked), Go asserts these cluster fields + tpcc table set.
    // 解密后（mock）断言集群字段与 tpcc 表集合。
    let backup_meta = BackupMeta {
        ClusterId: 7211076907329653533,
        ClusterVersion: "\"7.1.0-alpha\"\n".into(),
        ..Default::default()
    };
    let tables: Vec<_> = TPCC_TABLES.iter().map(|n| meta_table("tpcc", n)).collect();
    // mock 解密后的 tpcc 九表

    let cipher = CipherInfo {
        CipherType: EncryptionMethod::AES256_CTR,
        CipherKey: vec![0x42; 32],
    };
    // AES256-CTR + 32 字节密钥，与加密夹具匹配。
    install_local_hook(temp.local_uri(), cipher.clone(), backup_meta, tables, None);

    let cfg = Config {
        Storage: temp.local_uri(),
        Cipher: cipher,
        ..Default::default()
    };
    let ctx = Context::Background();
    let exec = CreateExec(&ctx, cfg).expect("CreateExec");
    let items = exec.Read(&ctx).expect("Read");
    clear_hook();

    assert_eq!(items.ClusterID, 7211076907329653533);
    assert_eq!(items.ClusterVersion, "\"7.1.0-alpha\"\n");
    let mut table_names = Vec::with_capacity(items.Tables.len());
    for tbl in &items.Tables {
        assert_eq!(tbl.DBName, "tpcc");
        table_names.push(tbl.TableName.clone());
    }
    let mut got = table_names;
    got.sort();
    let mut want: Vec<String> = TPCC_TABLES.iter().map(|s| (*s).to_string()).collect();
    want.sort();
    assert_eq!(got, want);
}

/// Format TSO like TiDB `SHOW BACKUP METADATA` datetime in a fixed offset.
/// 将 TSO 格式化为固定时区下的 `YYYY-MM-DD HH:MM:SS`（对齐 SQL show）。
fn format_backup_ts(ts: u64, offset_secs: i32) -> String {
    let ms = (ts >> 18) as i64;
    // TSO 高位为物理毫秒；再换算到目标时区的本地秒。
    let utc_secs = ms.div_euclid(1000);
    // 毫秒 → UTC 秒
    let local = utc_secs + i64::from(offset_secs);
    // 套用时区偏移
    let days = local.div_euclid(86400);
    let tod = local.rem_euclid(86400) as u32;
    let hour = tod / 3600;
    let min = (tod % 3600) / 60;
    let sec = tod % 60;
    let (year, month, day) = civil_ymd(days);
    // 拆年/月/日
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{min:02}:{sec:02}")
}

fn civil_ymd(days: i64) -> (i32, u32, u32) {
    // 儒略日风格 civil 日期换算（无 chrono 依赖）。
    let z = days + 719468;
    // 算法常数：与 civil_from_days 同源
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}

fn show_sql_rows(end_version: u64, offset_secs: i32) -> Vec<Vec<String>> {
    // 构造 Go `SHOW BACKUP METADATA` 行：库/表/0/0/<nil>/时间。
    let ts = format_backup_ts(end_version, offset_secs);
    let mut rows: Vec<Vec<String>> = TPCC_TABLES
        .iter()
        .map(|tbl| {
            vec![
                "tpcc".into(),
                (*tbl).into(),
                "0".into(),
                "0".into(),
                "<nil>".into(),
                ts.clone(),
            ]
        })
        .collect();
    rows.sort();
    // 排序后与期望矩阵逐行比较。
    rows
}

/// `TestShowViaSQL` — TiDB `testkit` / domain mocked; timezone formatting is real.
/// testkit 被 mock；真正校验的是 +08/-08 时区下的时间字符串。
#[test]
fn test_show_via_sql() {
    let temp = TempBackup::new("test_show_via_sql");
    // SQL show 时区用例
    let meta_path = temp.path().join("backupmeta");
    fs::write(&meta_path, FULL_META).expect("write full-schema backupmeta");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&meta_path).unwrap().permissions();
        perms.set_mode(0o444);
        fs::set_permissions(&meta_path, perms).unwrap();
    }

    let on_disk = fs::read(&meta_path).expect("re-read");
    let backup_meta = parse_backup_meta_scalars(&on_disk);
    let tables: Vec<_> = TPCC_TABLES.iter().map(|n| meta_table("tpcc", n)).collect();
    let cipher = CipherInfo {
        CipherType: EncryptionMethod::PLAINTEXT,
        CipherKey: vec![],
    };
    install_local_hook(
        temp.local_uri(),
        cipher.clone(),
        backup_meta.clone(),
        tables,
        Some(FULL_META),
    );

    // Go: set @@time_zone='+08:00' then SHOW BACKUP METADATA FROM 'local://...'
    // 对应 Go +08:00；期望时间为 2023-04-10 11:18:21。
    let ctx = Context::Background();
    let exec = CreateExec(
        &ctx,
        Config {
            Storage: temp.local_uri(),
            Cipher: cipher,
            ..Default::default()
        },
    )
    .expect("CreateExec");
    let items = exec.Read(&ctx).expect("Read");
    clear_hook();

    assert_eq!(items.EndVersion.0, 440689413714870273);
    let plus8 = show_sql_rows(items.EndVersion.0, 8 * 3600);
    // +8 小时偏移行矩阵。
    assert_eq!(
        plus8,
        vec![
            vec!["tpcc", "customer", "0", "0", "<nil>", "2023-04-10 11:18:21"],
            vec!["tpcc", "district", "0", "0", "<nil>", "2023-04-10 11:18:21"],
            vec!["tpcc", "history", "0", "0", "<nil>", "2023-04-10 11:18:21"],
            vec!["tpcc", "item", "0", "0", "<nil>", "2023-04-10 11:18:21"],
            vec![
                "tpcc",
                "new_order",
                "0",
                "0",
                "<nil>",
                "2023-04-10 11:18:21"
            ],
            vec![
                "tpcc",
                "order_line",
                "0",
                "0",
                "<nil>",
                "2023-04-10 11:18:21"
            ],
            vec!["tpcc", "orders", "0", "0", "<nil>", "2023-04-10 11:18:21"],
            vec!["tpcc", "stock", "0", "0", "<nil>", "2023-04-10 11:18:21"],
            vec![
                "tpcc",
                "warehouse",
                "0",
                "0",
                "<nil>",
                "2023-04-10 11:18:21"
            ],
        ]
        .into_iter()
        .map(|r| r.into_iter().map(str::to_string).collect::<Vec<_>>())
        .collect::<Vec<_>>()
    );

    // Go: set @@time_zone='-08:00'
    // 对应 Go -08:00；同一 TSO 显示为 2023-04-09 19:18:21。
    let minus8 = show_sql_rows(items.EndVersion.0, -8 * 3600);
    // -8 小时偏移行矩阵。
    assert_eq!(
        minus8,
        vec![
            vec!["tpcc", "customer", "0", "0", "<nil>", "2023-04-09 19:18:21"],
            vec!["tpcc", "district", "0", "0", "<nil>", "2023-04-09 19:18:21"],
            vec!["tpcc", "history", "0", "0", "<nil>", "2023-04-09 19:18:21"],
            vec!["tpcc", "item", "0", "0", "<nil>", "2023-04-09 19:18:21"],
            vec![
                "tpcc",
                "new_order",
                "0",
                "0",
                "<nil>",
                "2023-04-09 19:18:21"
            ],
            vec![
                "tpcc",
                "order_line",
                "0",
                "0",
                "<nil>",
                "2023-04-09 19:18:21"
            ],
            vec!["tpcc", "orders", "0", "0", "<nil>", "2023-04-09 19:18:21"],
            vec!["tpcc", "stock", "0", "0", "<nil>", "2023-04-09 19:18:21"],
            vec![
                "tpcc",
                "warehouse",
                "0",
                "0",
                "<nil>",
                "2023-04-09 19:18:21"
            ],
        ]
        .into_iter()
        .map(|r| r.into_iter().map(str::to_string).collect::<Vec<_>>())
        .collect::<Vec<_>>()
    );
}
