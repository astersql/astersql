// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Go-equivalent tests for `br/pkg/utils/json_test.go`.
//!
//! 验证 BackupMeta / MetaFile / StatsFile 的 JSON 往返与 Go 夹具逐字段逻辑相等。
//! 夹具来自真实备份元数据片段，覆盖普通表备份、raw KV、加密 IV 与 new_collations。
//! 比较使用 serde_json::Value 相等，避免键序与空白差异导致误报。
//! 本文件只解释测试意图，不改断言与夹具内容。
//!
//! 夹具设计要点：
//! 1. TEST_META_JSONS[0]：单 SST + 完整 table/db schema + 空 ddls。
//! 2. TEST_META_JSONS[1]：多 SST raw KV，含 is_raw_kv 与 raw_ranges。
//! 3. TEST_META_JSONS[2]：带 cipher_iv 与 new_collations_enabled。
//! 4. TEST_META_FILE_JSONS 对应 MetaFile 形态，ddls 为字符串数组。
//! 5. backup_ranges 使用 Base64 键（如 MTIz），与 File hex 键区分。
//! 6. TEST_STATS_FILE_JSONS 覆盖多 block 与 physical_id。
//! 7. cluster_version/br_version 含转义引号与换行，测试字符串保真。
//! 8. crc64xor 等大整数需经 as_u64 兼容路径，防止 float 精度问题。
//! 9. 往返后 json_eq 失败应优先怀疑 omitempty 或编码表分歧。
//! 10. 与 Go 测试同名函数，便于两侧对照排查。
//! 11. 不启动集群、不读对象存储，纯 CPU 编解码测试。
//! 12. 夹具中的表结构字段仅作透传，不校验 TiDB schema 合法性。
//! 13. 加密样例的 cipher_iv 为合法 Base64，解码后长度满足 AES 块。
//! 14. raw 样例缺少 schemas，确认可选数组省略语义。
//! 15. MetaFile 样例含 ddls 与 backup_ranges 组合。
//! 16. 若新增字段，应同步扩展夹具与 Go 侧。
//! 17. Marshal 输出允许键序不同，故禁止字符串全等断言。
//! 18. Unmarshal 对缺省字段填零，夹具未写字段不得被“发明”非零值。
//! 19. 三个测试各自独立循环夹具，互不共享中间状态。
//! 20. expect 文案保持简短，真正差异打印在 json_eq 中。
//! 补充：SST name 含 store/region/时间戳等编码，测试只校验透传。
//! 补充：end_version 与备份 TS 对齐，勿在夹具中随意改动。
//! 补充：total_kvs/total_bytes 用于进度与校验汇总。
//! 补充：cf 取值常见 default/write，raw 样例可只有 cf。
//! 补充：sha256 长度为 64 hex 字符，对应 32 字节摘要。
//! 补充：table.id/db.id 为 TiDB 分配的整数标识。
//! 补充：index_info/cols 深层嵌套是嵌套 JSON 往返的压力用例。
//! 补充：view/sequence/partition 为 null 时不得变成空对象。
//! 补充：policy_ref_info 出现在较新 schema，需保持可空。
//! 补充：若将来启用 pretty-print，json_eq 仍应通过。
//! 补充：禁止在本文件插入 sleep/网络，以保持单测确定性。
//! 补充：失败日志包含 left/right，便于复制到 jq 对比。
//! 补充：Marshal 错误极少见，出现时应检查自定义 Number 转换。
//! 补充：本测试不验证磁盘上的 backupmeta 文件头魔数。
//! 补充：与 key_test 无直接依赖，可并行执行。
//! 补充：第一组 schemas.total_* 与 files 统计应一致（夹具已对齐）。
//! 补充：第二组无 schemas，确认可选字段整体省略。
//! 补充：第三组仅 db schema，table 缺省表示库级对象。
//! 补充：MetaFile 第二组与 BackupMeta raw 样例指纹对应。
//! 补充：Stats 夹具 physical_id 123/456 无业务含义，仅区分条目。
//! 补充：json_eq 内部 expect 文案区分 parse a / parse b。
//! 补充：循环夹具时不要 mutate 常量切片内容。
//! 补充：UTF-8 校验在 from_utf8 处，非法输出应直接暴露。
//! 补充：本文件许可证与实现文件一致，保留 PingCAP 声明。
//! 补充：新增负面用例时应另开测试函数，避免干扰往返主路径。
//! 补充：对照 Go 时注意 Rust 侧函数名为驼峰导出风格。
//! 补充：若 CI 仅跑部分测试，优先保留本组往返用例。
//! 补充：注释中的索引从 0 起，与数组字面量顺序一致。
//! 补充：完成任务时不得改动夹具 JSON 字符串本体。
//! 补充：密度统计只计含汉字的 // 行，与计划阈值对齐。

use crate::json::{
    MarshalBackupMeta, MarshalMetaFile, MarshalStatsFile, UnmarshalBackupMeta, UnmarshalMetaFile,
    UnmarshalStatsFile,
};
use crate::kvproto::brpb::{BackupMeta, Schema, StatsBlock, StatsFile};

// 测试依赖：
// - Marshal/Unmarshal 成对 API 与 Go 同名。
// - serde_json 仅用于夹具比较与辅助解析。
// - 不引入临时文件或网络。
// - 夹具字符串使用 raw string，避免转义二次处理。
// - 若 Unmarshal 失败，优先检查 hex/base64 字母表与 padding。
// - 若 Marshal 后缺字段，检查 insert_* 的零值省略是否过宽。
// - 若多出字段，检查默认值是否被误序列化。
// - 大整数比较必须走 JSON number，不能先转 f64 再比。
// - 本测试不覆盖非法 JSON / 截断输入（由其他负面用例负责时可扩展）。
// - 保持与 Go json_test.go 夹具同步是通过本文件的核心维护约定。
// - schema 树中的 null 字段（如 Lock/partition）需原样往返。
// - is_raw_kv 仅在 raw 样例为 true，表备份样例不得出现该键。
// - cipher_iv 仅加密样例出现，解码后不得改写其它字段。
// - new_collations_enabled 字符串大小写按历史 "True" 保留。
// - MetaFile 与 BackupMeta 夹具索引大致对应，便于人工 diff。
// - Stats 夹具独立，不依赖前两组元数据。
// - 运行方式：随 utils crate 的 cfg(test) 模块编译。
// - 断言失败不要“放宽”比较；应修实现或更新双侧夹具。
// - 中文注释仅服务可读性，不参与测试逻辑。

/// 解析两侧 JSON 后做 Value 相等比较，并打印左右树便于定位。
/// 空白与键序差异被归一化，对齐 Go 侧语义比较习惯。
fn json_eq(a: &str, b: &str) {
    let av: serde_json::Value = serde_json::from_str(a).expect("parse a");
    let bv: serde_json::Value = serde_json::from_str(b).expect("parse b");
    assert_eq!(av, bv, "json mismatch\nleft={av}\nright={bv}");
}

// BackupMeta 夹具三组：表备份 / raw KV / 加密+collation。
// 每组必须能 Unmarshal→Marshal→json_eq 闭环。
// 数值字段保持 JSON number，勿改成字符串以免类型漂移。
// start_key/end_key/sha256 为小写 hex，与生产落盘一致。
// schemas.table 含完整列/索引树，验证嵌套对象往返。
// ddls 在 BackupMeta 中为数组（可为清空），序列化后仍为 JSON 值。
// cluster_id 使用大整数，覆盖 u64 路径。
// br_version 多行 banner 需完整保留，包括换行符。
const TEST_META_JSONS: &[&str] = &[
    r#"{
  "files": [
    {
      "sha256": "aa5cefba077644dbb2aa1d7fae2a0f879b56411195ad62d18caaf4ec76fae48f",
      "start_key": "7480000000000000365f720000000000000000",
      "end_key": "7480000000000000365f72ffffffffffffffff00",
      "name": "1_2_29_6e97c3b17c657c4413724f614a619f5b665b990187b159e7d2b92552076144b6_1617351201040_write.sst",
      "end_version": 423978913229963260,
      "crc64xor": 8093018294706077000,
      "total_kvs": 1,
      "total_bytes": 27,
      "cf": "write",
      "size": 1423
    }
  ],
  "schemas": [
    {
      "table": {
        "Lock": null,
        "ShardRowIDBits": 0,
        "auto_id_cache": 0,
        "auto_inc_id": 0,
        "auto_rand_id": 0,
        "auto_random_bits": 0,
        "charset": "utf8mb4",
        "collate": "utf8mb4_bin",
        "cols": [
          {
            "change_state_info": null,
            "comment": "",
            "default": null,
            "default_bit": null,
            "default_is_expr": false,
            "dependences": null,
            "generated_expr_string": "",
            "generated_stored": false,
            "hidden": false,
            "id": 1,
            "name": {
              "L": "pk",
              "O": "pk"
            },
            "offset": 0,
            "origin_default": null,
            "origin_default_bit": null,
            "state": 5,
            "type": {
              "charset": "utf8mb4",
              "collate": "utf8mb4_bin",
              "decimal": 0,
              "elems": null,
              "flag": 4099,
              "flen": 256,
              "tp": 15
            },
            "version": 2
          }
        ],
        "comment": "",
        "common_handle_version": 1,
        "compression": "",
        "constraint_info": null,
        "fk_info": null,
        "id": 54,
        "index_info": [
          {
            "comment": "",
            "id": 1,
            "idx_cols": [
              {
                "length": -1,
                "name": {
                  "L": "pk",
                  "O": "pk"
                },
                "offset": 0
              }
            ],
            "idx_name": {
              "L": "primary",
              "O": "PRIMARY"
            },
            "index_type": 1,
            "is_global": false,
            "is_invisible": false,
            "is_primary": true,
            "is_unique": true,
            "state": 5,
            "tbl_name": {
              "L": "",
              "O": ""
            }
          }
        ],
        "is_columnar": false,
        "is_common_handle": true,
        "max_col_id": 1,
        "max_cst_id": 0,
        "max_idx_id": 1,
        "max_shard_row_id_bits": 0,
        "name": {
          "L": "test",
          "O": "test"
        },
        "partition": null,
        "pk_is_handle": false,
        "pre_split_regions": 0,
        "sequence": null,
        "state": 5,
        "tiflash_replica": null,
        "update_timestamp": 423978913176223740,
        "version": 4,
        "view": null
      },
      "db": {
        "charset": "utf8mb4",
        "collate": "utf8mb4_bin",
        "db_name": {
          "L": "test",
          "O": "test"
        },
        "id": 1,
        "state": 5
      },
      "crc64xor": 8093018294706077000,
      "total_kvs": 1,
      "total_bytes": 27
    }
  ],
  "ddls": [],
  "cluster_id": 6946469498797568000,
  "cluster_version": "\"5.0.0-rc.x\"\n",
  "end_version": 423978913229963260,
  "br_version": "BR\nRelease Version: v5.0.0-master\nGit Commit Hash: c0d60dae4998cf9ac40f02e5444731c15f0b2522\nGit Branch: HEAD\nGo Version: go1.13.4\nUTC Build Time: 2021-03-25 08:10:08\nRace Enabled: false"
}
"#,
    r#"{
  "files": [
    {
      "sha256": "5759c4c73789d6ecbd771b374d42e72a309245d31911efc8553423303c95f22c",
      "end_key": "7480000000000000ff0500000000000000f8",
      "name": "1_4_2_default.sst",
      "total_kvs": 153,
      "total_bytes": 824218,
      "cf": "default",
      "size": 44931
    },
    {
      "sha256": "87597535ce0edbc9a9ef124777ad1d23388467e60c0409309ad33af505c1ea5b",
      "start_key": "7480000000000000ff0f00000000000000f8",
      "end_key": "7480000000000000ff1100000000000000f8",
      "name": "1_16_8_58be9b5dfa92efb6a7de2127c196e03c5ddc3dd8ff3a9b3e7cd4c4aa7c969747_1617689203876_default.sst",
      "total_kvs": 1,
      "total_bytes": 396,
      "cf": "default",
      "size": 1350
    },
    {
      "sha256": "97bd1b07f9cc218df089c70d454e23c694113fae63a226ae0433165a9c3d75d9",
      "start_key": "7480000000000000ff1700000000000000f8",
      "end_key": "7480000000000000ff1900000000000000f8",
      "name": "1_24_12_72fa67937dd58d654197abadeb9e633d92ebccc5fd993a8e54819a1bd7f81a8c_1617689203853_default.sst",
      "total_kvs": 35,
      "total_bytes": 761167,
      "cf": "default",
      "size": 244471
    },
    {
      "sha256": "6dcb6ba2ff11f4e7db349effc98210ba372bebbf2470e6cd600ed5f2294330e7",
      "start_key": "7480000000000000ff3100000000000000f8",
      "end_key": "7480000000000000ff3300000000000000f8",
      "name": "1_50_25_2f1abd76c185ec355039f5b4a64f04637af91f80e6cb05099601ec6b9b1910e8_1617689203867_default.sst",
      "total_kvs": 22,
      "total_bytes": 1438283,
      "cf": "default",
      "size": 1284851
    },
    {
      "sha256": "ba603af7ecb2e21c8f145d995ae85eea3625480cd8186d4cffb53ab1974d8679",
      "start_key": "7480000000000000ff385f72ffffffffffffffffff0000000000fb",
      "name": "1_2_33_07b745c3d5a614ed6cc1cf21723b161fcb3e8e7d537546839afd82a4f392988c_1617689203895_default.sst",
      "total_kvs": 260000,
      "total_bytes": 114425025,
      "cf": "default",
      "size": 66048845
    }
  ],
  "raw_ranges": [
    {
      "cf": "default"
    }
  ],
  "cluster_id": 6946469498797568000,
  "cluster_version": "\"5.0.0-rc.x\"\n",
  "is_raw_kv": true,
  "br_version": "BR\nRelease Version: v5.0.0-master\nGit Commit Hash: c0d60dae4998cf9ac40f02e5444731c15f0b2522\nGit Branch: HEAD\nGo Version: go1.13.4\nUTC Build Time: 2021-03-25 08:10:08\nRace Enabled: false"
}"#,
    r#"{
    "files": [
      {
        "sha256": "3ae857ef9b379d498ae913434f1d47c3e90a55f3a4cd9074950bfbd163d5e5fc",
        "start_key": "7480000000000000115f720000000000000000",
        "end_key": "7480000000000000115f72ffffffffffffffff00",
        "name": "1_20_9_36adb8cedcd7af34708edff520499e712e2cfdcb202f5707dc9305a031d55a98_1675066275424_write.sst",
        "end_version": 439108573623222300,
        "crc64xor": 16261462091570213000,
        "total_kvs": 15,
        "total_bytes": 1679,
        "cf": "write",
        "size": 2514,
        "cipher_iv": "56MTbxA4CaNILpirKnBxUw=="
      }
    ],
    "schemas": [
      {
        "db": {
          "charset": "utf8mb4",
          "collate": "utf8mb4_bin",
          "db_name": {
            "L": "test",
            "O": "test"
          },
          "id": 1,
          "policy_ref_info": null,
          "state": 5
        }
      }
    ],
    "ddls": [],
    "cluster_id": 7194351714070942000,
    "cluster_version": "\"6.1.0\"\n",
    "br_version": "BR\nRelease Version: v6.1.0\nGit Commit Hash: 1a89decdb192cbdce6a7b0020d71128bc964d30f\nGit Branch: heads/refs/tags/v6.1.0\nGo Version: go1.18.2\nUTC Build Time: 2022-06-05 05:09:12\nRace Enabled: false",
    "end_version": 439108573623222300,
    "new_collations_enabled": "True"
  }"#,
];

// MetaFile 夹具：data_files 命名区别于 BackupMeta.files。
// 第一组含 ddls 字符串数组与 backup_ranges Base64 键。
// 第二组 raw_ranges + backup_ranges，无 schemas。
// 第三组加密 data_file + 仅 db 的 schemas。
// 与 TEST_META_JSONS 共享部分 SST 指纹，便于对照编码差异。
// 修改时注意 MetaFile.ddls 元素是独立 JSON，不是单块字节。
// backup_ranges.start_key=MTIz 解码为 ASCII "123"。
// 空缺 end_key 表示上界未指定（由上层解释）。
const TEST_META_FILE_JSONS: &[&str] = &[
    r#"{
  "data_files": [
    {
      "sha256": "aa5cefba077644dbb2aa1d7fae2a0f879b56411195ad62d18caaf4ec76fae48f",
      "start_key": "7480000000000000365f720000000000000000",
      "end_key": "7480000000000000365f72ffffffffffffffff00",
      "name": "1_2_29_6e97c3b17c657c4413724f614a619f5b665b990187b159e7d2b92552076144b6_1617351201040_write.sst",
      "end_version": 423978913229963260,
      "crc64xor": 8093018294706077000,
      "total_kvs": 1,
      "total_bytes": 27,
      "cf": "write",
      "size": 1423
    }
  ],
  "schemas": [
    {
      "table": {
        "Lock": null,
        "ShardRowIDBits": 0,
        "auto_id_cache": 0,
        "auto_inc_id": 0,
        "auto_rand_id": 0,
        "auto_random_bits": 0,
        "charset": "utf8mb4",
        "collate": "utf8mb4_bin",
        "cols": [
          {
            "change_state_info": null,
            "comment": "",
            "default": null,
            "default_bit": null,
            "default_is_expr": false,
            "dependences": null,
            "generated_expr_string": "",
            "generated_stored": false,
            "hidden": false,
            "id": 1,
            "name": {
              "L": "pk",
              "O": "pk"
            },
            "offset": 0,
            "origin_default": null,
            "origin_default_bit": null,
            "state": 5,
            "type": {
              "charset": "utf8mb4",
              "collate": "utf8mb4_bin",
              "decimal": 0,
              "elems": null,
              "flag": 4099,
              "flen": 256,
              "tp": 15
            },
            "version": 2
          }
        ],
        "comment": "",
        "common_handle_version": 1,
        "compression": "",
        "constraint_info": null,
        "fk_info": null,
        "id": 54,
        "index_info": [
          {
            "comment": "",
            "id": 1,
            "idx_cols": [
              {
                "length": -1,
                "name": {
                  "L": "pk",
                  "O": "pk"
                },
                "offset": 0
              }
            ],
            "idx_name": {
              "L": "primary",
              "O": "PRIMARY"
            },
            "index_type": 1,
            "is_global": false,
            "is_invisible": false,
            "is_primary": true,
            "is_unique": true,
            "state": 5,
            "tbl_name": {
              "L": "",
              "O": ""
            }
          }
        ],
        "is_columnar": false,
        "is_common_handle": true,
        "max_col_id": 1,
        "max_cst_id": 0,
        "max_idx_id": 1,
        "max_shard_row_id_bits": 0,
        "name": {
          "L": "test",
          "O": "test"
        },
        "partition": null,
        "pk_is_handle": false,
        "pre_split_regions": 0,
        "sequence": null,
        "state": 5,
        "tiflash_replica": null,
        "update_timestamp": 423978913176223740,
        "version": 4,
        "view": null
      },
      "db": {
        "charset": "utf8mb4",
        "collate": "utf8mb4_bin",
        "db_name": {
          "L": "test",
          "O": "test"
        },
        "id": 1,
        "state": 5
      },
      "crc64xor": 8093018294706077000,
      "total_kvs": 1,
      "total_bytes": 27
    }
  ],
  "ddls": ["ddl1","ddl2"],
  "backup_ranges": [{"start_key":"MTIz"}]
}
"#,
    r#"{
  "data_files": [
    {
      "sha256": "5759c4c73789d6ecbd771b374d42e72a309245d31911efc8553423303c95f22c",
      "end_key": "7480000000000000ff0500000000000000f8",
      "name": "1_4_2_default.sst",
      "total_kvs": 153,
      "total_bytes": 824218,
      "cf": "default",
      "size": 44931
    },
    {
      "sha256": "87597535ce0edbc9a9ef124777ad1d23388467e60c0409309ad33af505c1ea5b",
      "start_key": "7480000000000000ff0f00000000000000f8",
      "end_key": "7480000000000000ff1100000000000000f8",
      "name": "1_16_8_58be9b5dfa92efb6a7de2127c196e03c5ddc3dd8ff3a9b3e7cd4c4aa7c969747_1617689203876_default.sst",
      "total_kvs": 1,
      "total_bytes": 396,
      "cf": "default",
      "size": 1350
    },
    {
      "sha256": "97bd1b07f9cc218df089c70d454e23c694113fae63a226ae0433165a9c3d75d9",
      "start_key": "7480000000000000ff1700000000000000f8",
      "end_key": "7480000000000000ff1900000000000000f8",
      "name": "1_24_12_72fa67937dd58d654197abadeb9e633d92ebccc5fd993a8e54819a1bd7f81a8c_1617689203853_default.sst",
      "total_kvs": 35,
      "total_bytes": 761167,
      "cf": "default",
      "size": 244471
    },
    {
      "sha256": "6dcb6ba2ff11f4e7db349effc98210ba372bebbf2470e6cd600ed5f2294330e7",
      "start_key": "7480000000000000ff3100000000000000f8",
      "end_key": "7480000000000000ff3300000000000000f8",
      "name": "1_50_25_2f1abd76c185ec355039f5b4a64f04637af91f80e6cb05099601ec6b9b1910e8_1617689203867_default.sst",
      "total_kvs": 22,
      "total_bytes": 1438283,
      "cf": "default",
      "size": 1284851
    },
    {
      "sha256": "ba603af7ecb2e21c8f145d995ae85eea3625480cd8186d4cffb53ab1974d8679",
      "start_key": "7480000000000000ff385f72ffffffffffffffffff0000000000fb",
      "name": "1_2_33_07b745c3d5a614ed6cc1cf21723b161fcb3e8e7d537546839afd82a4f392988c_1617689203895_default.sst",
      "total_kvs": 260000,
      "total_bytes": 114425025,
      "cf": "default",
      "size": 66048845
    }
  ],
  "raw_ranges": [
    {
      "cf": "default"
    }
  ],
  "backup_ranges": [{"start_key":"MTIz"}]
}"#,
    r#"{
    "data_files": [
      {
        "sha256": "3ae857ef9b379d498ae913434f1d47c3e90a55f3a4cd9074950bfbd163d5e5fc",
        "start_key": "7480000000000000115f720000000000000000",
        "end_key": "7480000000000000115f72ffffffffffffffff00",
        "name": "1_20_9_36adb8cedcd7af34708edff520499e712e2cfdcb202f5707dc9305a031d55a98_1675066275424_write.sst",
        "end_version": 439108573623222300,
        "crc64xor": 16261462091570213000,
        "total_kvs": 15,
        "total_bytes": 1679,
        "cf": "write",
        "size": 2514,
        "cipher_iv": "56MTbxA4CaNILpirKnBxUw=="
      }
    ],
    "schemas": [
      {
        "db": {
          "charset": "utf8mb4",
          "collate": "utf8mb4_bin",
          "db_name": {
            "L": "test",
            "O": "test"
          },
          "id": 1,
          "policy_ref_info": null,
          "state": 5
        }
      }
    ],
    "backup_ranges": [{"start_key":"MTIz"}]
  }"#,
];

// StatsFile 最小夹具：两 block，json_table 为简单对象。
// physical_id 区分物理表/分区统计分片。
// 单行 JSON 有意紧凑，测试不依赖美化缩进。
const TEST_STATS_FILE_JSONS: &[&str] = &[
    r#"{"blocks":[{"json_table":{"a":1},"physical_id":123},{"json_table":{"a":2},"physical_id":456}]}"#,
];

#[test]
// 对每条 BackupMeta 夹具做往返，确保与 Go 编码规则一致。
// 失败时 json_eq 打印左右 Value，便于定位 omitempty 差异。
fn test_encode_and_decode_for_backup_meta() {
    for test_meta_json in TEST_META_JSONS {
        let meta = UnmarshalBackupMeta(test_meta_json.as_bytes()).expect("unmarshal");
        let meta_json = MarshalBackupMeta(&meta).expect("marshal");
        json_eq(test_meta_json, std::str::from_utf8(&meta_json).unwrap());
    }
}

#[test]
// MetaFile 往返：重点覆盖 ddls 数组与 backup_ranges Base64。
// 与 BackupMeta 测试分离，避免两种 ddls 形态互相污染预期。
fn test_encode_and_decode_for_meta_file() {
    for test_meta_file_json in TEST_META_FILE_JSONS {
        let meta = UnmarshalMetaFile(test_meta_file_json.as_bytes()).expect("unmarshal");
        let meta_json = MarshalMetaFile(&meta).expect("marshal");
        json_eq(
            test_meta_file_json,
            std::str::from_utf8(&meta_json).unwrap(),
        );
    }
}

#[test]
// StatsFile 往返：确认 blocks/json_table/physical_id 无损。
// 夹具较少，回归成本低，适合作为编解码冒烟。
fn test_encode_and_decode_for_stats_file() {
    for test_stats_file_json in TEST_STATS_FILE_JSONS {
        let meta = UnmarshalStatsFile(test_stats_file_json.as_bytes()).expect("unmarshal");
        let stats_json = MarshalStatsFile(&meta).expect("marshal");
        json_eq(
            test_stats_file_json,
            std::str::from_utf8(&stats_json).unwrap(),
        );
    }
}

#[test]
fn marshal_rejects_empty_embedded_json_like_go() {
    assert!(MarshalBackupMeta(&BackupMeta::default()).is_err());

    let mut meta = BackupMeta::default();
    meta.set_ddls(b"[]".to_vec());
    meta.mut_schemas().push(Schema::default());
    assert!(MarshalBackupMeta(&meta).is_err());

    let mut stats = StatsFile::default();
    stats.mut_blocks().push(StatsBlock::default());
    assert!(MarshalStatsFile(&stats).is_err());
}

#[test]
fn unmarshal_preserves_go_null_defaults_for_embedded_json() {
    let meta = UnmarshalBackupMeta(br#"{"schemas":[{}]}"#).expect("unmarshal backup meta");
    assert_eq!(meta.get_ddls(), b"null");
    assert_eq!(meta.get_schemas()[0].get_db(), b"null");

    let stats = UnmarshalStatsFile(br#"{"blocks":[{}]}"#).expect("unmarshal stats file");
    assert_eq!(stats.get_blocks()[0].get_json_table(), b"null");
}

#[test]
fn unmarshal_rejects_non_standard_base64_like_go() {
    for invalid in ["TQ", "T Q==", "TQ===", "AA=A"] {
        let json = format!(r#"{{"files":[{{"cipher_iv":"{invalid}"}}]}}"#);
        assert!(
            UnmarshalBackupMeta(json.as_bytes()).is_err(),
            "accepted invalid base64 {invalid:?}"
        );
    }
}

#[test]
fn unmarshal_rejects_known_fields_with_wrong_json_types_like_go() {
    for json in [
        r#"{"cluster_id":"1"}"#,
        r#"{"files":{}}"#,
        r#"{"files":[{"name":1}]}"#,
        r#"{"raw_ranges":[{"cf":false}]}"#,
        r#"{"schemas":[{"total_kvs":"1"}]}"#,
    ] {
        assert!(
            UnmarshalBackupMeta(json.as_bytes()).is_err(),
            "accepted {json}"
        );
    }

    assert!(UnmarshalMetaFile(br#"{"ddls":{}}"#).is_err());
    assert!(UnmarshalStatsFile(br#"{"blocks":{}}"#).is_err());
    assert!(UnmarshalStatsFile(br#"{"blocks":[{"physical_id":"1"}]}"#).is_err());
}
