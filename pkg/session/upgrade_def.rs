// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Bootstrap 升级版本号与升级函数桩定义。
//
// `versionN` 为历史 bootstrap 版本常量；`upgradeToVerN` 为对应迁移入口（当前经
// `installUpgradeAction` 安装的执行器按名分发）。另含 binding 摘要更新相关结构。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use std::sync::{LazyLock, OnceLock};

// Bootstrap 版本号常量：集群元数据中的 tidb_server_version 步进值。
// 缺号表示该版本无独立迁移或已合并；当前最新为 version262。
pub const version2: i64 = 2;
pub const version3: i64 = 3;
pub const version4: i64 = 4;
pub const version5: i64 = 5;
pub const version6: i64 = 6;
pub const version7: i64 = 7;
pub const version8: i64 = 8;
pub const version9: i64 = 9;
pub const version10: i64 = 10;
pub const version11: i64 = 11;
pub const version12: i64 = 12;
pub const version13: i64 = 13;
pub const version14: i64 = 14;
pub const version15: i64 = 15;
pub const version16: i64 = 16;
pub const version17: i64 = 17;
pub const version18: i64 = 18;
pub const version19: i64 = 19;
pub const version20: i64 = 20;
pub const version21: i64 = 21;
pub const version22: i64 = 22;
pub const version23: i64 = 23;
pub const version24: i64 = 24;
pub const version25: i64 = 25;
pub const version26: i64 = 26;
pub const version27: i64 = 27;
pub const version28: i64 = 28;
pub const version29: i64 = 29;
pub const version30: i64 = 30;
pub const version31: i64 = 31;
pub const version32: i64 = 32;
pub const version33: i64 = 33;
pub const version34: i64 = 34;
pub const version35: i64 = 35;
pub const version36: i64 = 36;
pub const version37: i64 = 37;
pub const version38: i64 = 38;
pub const version40: i64 = 40;
pub const version41: i64 = 41;
pub const version42: i64 = 42;
pub const version43: i64 = 43;
pub const version44: i64 = 44;
pub const version45: i64 = 45;
pub const version46: i64 = 46;
pub const version47: i64 = 47;
pub const version50: i64 = 50;
pub const version52: i64 = 52;
pub const version53: i64 = 53;
pub const version54: i64 = 54;
pub const version55: i64 = 55;
pub const version56: i64 = 56;
pub const version57: i64 = 57;
pub const version59: i64 = 59;
pub const version60: i64 = 60;
pub const version62: i64 = 62;
pub const version63: i64 = 63;
pub const version64: i64 = 64;
pub const version65: i64 = 65;
pub const version66: i64 = 66;
pub const version67: i64 = 67;
pub const version68: i64 = 68;
pub const version69: i64 = 69;
pub const version70: i64 = 70;
pub const version71: i64 = 71;
pub const version72: i64 = 72;
pub const version73: i64 = 73;
pub const version74: i64 = 74;
pub const version75: i64 = 75;
pub const version76: i64 = 76;
pub const version77: i64 = 77;
pub const version78: i64 = 78;
pub const version79: i64 = 79;
pub const version80: i64 = 80;
pub const version81: i64 = 81;
pub const version82: i64 = 82;
pub const version83: i64 = 83;
pub const version84: i64 = 84;
pub const version85: i64 = 85;
pub const version86: i64 = 86;
pub const version87: i64 = 87;
pub const version88: i64 = 88;
pub const version89: i64 = 89;
pub const version90: i64 = 90;
pub const version91: i64 = 91;
pub const version92: i64 = 92;
pub const version93: i64 = 93;
pub const version94: i64 = 94;
pub const version95: i64 = 95;
pub const version97: i64 = 97;
pub const version98: i64 = 98;
pub const version99: i64 = 99;
pub const version100: i64 = 100;
pub const version101: i64 = 101;
pub const version102: i64 = 102;
pub const version103: i64 = 103;
pub const version104: i64 = 104;
pub const version105: i64 = 105;
pub const version106: i64 = 106;
pub const version107: i64 = 107;
pub const version108: i64 = 108;
pub const version109: i64 = 109;
pub const version110: i64 = 110;
pub const version130: i64 = 130;
pub const version131: i64 = 131;
pub const version132: i64 = 132;
pub const version133: i64 = 133;
pub const version134: i64 = 134;
pub const version135: i64 = 135;
pub const version136: i64 = 136;
pub const version137: i64 = 137;
pub const version138: i64 = 138;
pub const version139: i64 = 139;
pub const version140: i64 = 140;
pub const version141: i64 = 141;
pub const version142: i64 = 142;
pub const version143: i64 = 143;
pub const version144: i64 = 144;
pub const version145: i64 = 145;
pub const version146: i64 = 146;
pub const version167: i64 = 167;
pub const version168: i64 = 168;
pub const version169: i64 = 169;
pub const version170: i64 = 170;
pub const version171: i64 = 171;
pub const version172: i64 = 172;
pub const version173: i64 = 173;
pub const version174: i64 = 174;
pub const version175: i64 = 175;
pub const version176: i64 = 176;
pub const version177: i64 = 177;
pub const version178: i64 = 178;
pub const version179: i64 = 179;
pub const version190: i64 = 190;
pub const version191: i64 = 191;
pub const version192: i64 = 192;
pub const version193: i64 = 193;
pub const version194: i64 = 194;
pub const version195: i64 = 195;
pub const version196: i64 = 196;
pub const version197: i64 = 197;
pub const version198: i64 = 198;
pub const version209: i64 = 209;
pub const version210: i64 = 210;
pub const version211: i64 = 211;
pub const version212: i64 = 212;
pub const version213: i64 = 213;
pub const version214: i64 = 214;
pub const version215: i64 = 215;
pub const version216: i64 = 216;
pub const version217: i64 = 217;
pub const version218: i64 = 218;
pub const version239: i64 = 239;
pub const version240: i64 = 240;
pub const version241: i64 = 241;
pub const version242: i64 = 242;
pub const version243: i64 = 243;
pub const version244: i64 = 244;
pub const version245: i64 = 245;
pub const version246: i64 = 246;
pub const version247: i64 = 247;
pub const version248: i64 = 248;
pub const version249: i64 = 249;
pub const version250: i64 = 250;
pub const version251: i64 = 251;
pub const version252: i64 = 252;
pub const version253: i64 = 253;
pub const version254: i64 = 254;
pub const version255: i64 = 255;
pub const version256: i64 = 256;
pub const version257: i64 = 257;
pub const version258: i64 = 258;
pub const version259: i64 = 259;
pub const version260: i64 = 260;
pub const version261: i64 = 261;
pub const version262: i64 = 262;

/// 版本号与升级函数指针的配对（对应 Go upgradeToVerFunctions 表项）。
pub struct VersionedUpgradeFunction {
    /// 目标版本。
    pub version: i64,
    /// 升级回调：会话与“升级前版本”。
    pub function: fn(&sessionapi::Session, i64),
}
/// Go 风格别名。
#[allow(non_camel_case_types)]
pub type versionedUpgradeFunction = VersionedUpgradeFunction;

/// mysql.bind_info 行的核心字段快照。
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub struct bindInfo {
    /// 绑定后的 SQL 文本。
    pub bindSQL: String,
    /// 状态（enabled/deleted 等）。
    pub status: String,
    /// 创建时间。
    pub createTime: String,
    /// 字符集。
    pub charset: String,
    /// 排序规则。
    pub collation: String,
    /// 来源标记。
    pub source: String,
}

/// 升级时待写回的 binding digest 更新项。
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub struct bindingDigestUpdate {
    /// 行 ID。
    pub rowID: i64,
    /// 规范化前/后的原始 SQL。
    pub originalSQL: String,
    /// 新的 sql_digest。
    pub sqlDigest: String,
    /// 是否与已有 digest 冲突（需标记删除）。
    pub duplicate: bool,
}

/// sql_digest 与 plan_digest 对。
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub struct bindingDigestPair {
    /// SQL 文本 digest。
    pub sqlDigest: String,
    /// 执行计划 digest。
    pub planDigest: String,
}
/// 当前代码支持的最新 bootstrap 版本。
pub static mut currentBootstrapVersion: i64 = version262;
/// 有序升级函数表，对应 Go `upgradeToVerFunctions`。
pub static upgradeToVerFunctions: LazyLock<Vec<VersionedUpgradeFunction>> = LazyLock::new(|| {
    macro_rules! upgrade_function {
        (2) => {
            upgradeToVer2
        };
        (3) => {
            upgradeToVer3
        };
        (4) => {
            upgradeToVer4
        };
        (5) => {
            upgradeToVer5
        };
        (6) => {
            upgradeToVer6
        };
        (7) => {
            upgradeToVer7
        };
        (8) => {
            upgradeToVer8
        };
        (9) => {
            upgradeToVer9
        };
        (10) => {
            upgradeToVer10
        };
        (11) => {
            upgradeToVer11
        };
        (12) => {
            upgradeToVer12
        };
        (13) => {
            upgradeToVer13
        };
        (14) => {
            upgradeToVer14
        };
        (15) => {
            upgradeToVer15
        };
        (16) => {
            upgradeToVer16
        };
        (17) => {
            upgradeToVer17
        };
        (18) => {
            upgradeToVer18
        };
        (19) => {
            upgradeToVer19
        };
        (20) => {
            upgradeToVer20
        };
        (21) => {
            upgradeToVer21
        };
        (22) => {
            upgradeToVer22
        };
        (23) => {
            upgradeToVer23
        };
        (24) => {
            upgradeToVer24
        };
        (25) => {
            upgradeToVer25
        };
        (26) => {
            upgradeToVer26
        };
        (27) => {
            upgradeToVer27
        };
        (28) => {
            upgradeToVer28
        };
        (29) => {
            upgradeToVer29
        };
        (30) => {
            upgradeToVer30
        };
        (31) => {
            upgradeToVer31
        };
        (32) => {
            upgradeToVer32
        };
        (33) => {
            upgradeToVer33
        };
        (34) => {
            upgradeToVer34
        };
        (35) => {
            upgradeToVer35
        };
        (36) => {
            upgradeToVer36
        };
        (37) => {
            upgradeToVer37
        };
        (38) => {
            upgradeToVer38
        };
        (40) => {
            upgradeToVer40
        };
        (41) => {
            upgradeToVer41
        };
        (42) => {
            upgradeToVer42
        };
        (43) => {
            upgradeToVer43
        };
        (44) => {
            upgradeToVer44
        };
        (45) => {
            upgradeToVer45
        };
        (46) => {
            upgradeToVer46
        };
        (47) => {
            upgradeToVer47
        };
        (50) => {
            upgradeToVer50
        };
        (52) => {
            upgradeToVer52
        };
        (53) => {
            upgradeToVer53
        };
        (54) => {
            upgradeToVer54
        };
        (55) => {
            upgradeToVer55
        };
        (56) => {
            upgradeToVer56
        };
        (57) => {
            upgradeToVer57
        };
        (59) => {
            upgradeToVer59
        };
        (60) => {
            upgradeToVer60
        };
        (62) => {
            upgradeToVer62
        };
        (63) => {
            upgradeToVer63
        };
        (64) => {
            upgradeToVer64
        };
        (65) => {
            upgradeToVer65
        };
        (66) => {
            upgradeToVer66
        };
        (67) => {
            upgradeToVer67
        };
        (68) => {
            upgradeToVer68
        };
        (69) => {
            upgradeToVer69
        };
        (70) => {
            upgradeToVer70
        };
        (71) => {
            upgradeToVer71
        };
        (72) => {
            upgradeToVer72
        };
        (73) => {
            upgradeToVer73
        };
        (74) => {
            upgradeToVer74
        };
        (75) => {
            upgradeToVer75
        };
        (76) => {
            upgradeToVer76
        };
        (77) => {
            upgradeToVer77
        };
        (78) => {
            upgradeToVer78
        };
        (79) => {
            upgradeToVer79
        };
        (80) => {
            upgradeToVer80
        };
        (81) => {
            upgradeToVer81
        };
        (82) => {
            upgradeToVer82
        };
        (83) => {
            upgradeToVer83
        };
        (84) => {
            upgradeToVer84
        };
        (85) => {
            upgradeToVer85
        };
        (86) => {
            upgradeToVer86
        };
        (87) => {
            upgradeToVer87
        };
        (88) => {
            upgradeToVer88
        };
        (89) => {
            upgradeToVer89
        };
        (90) => {
            upgradeToVer90
        };
        (91) => {
            upgradeToVer91
        };
        (93) => {
            upgradeToVer93
        };
        (94) => {
            upgradeToVer94
        };
        (95) => {
            upgradeToVer95
        };
        (97) => {
            upgradeToVer97
        };
        (98) => {
            upgradeToVer98
        };
        (100) => {
            upgradeToVer100
        };
        (101) => {
            upgradeToVer101
        };
        (102) => {
            upgradeToVer102
        };
        (103) => {
            upgradeToVer103
        };
        (104) => {
            upgradeToVer104
        };
        (105) => {
            upgradeToVer105
        };
        (106) => {
            upgradeToVer106
        };
        (107) => {
            upgradeToVer107
        };
        (108) => {
            upgradeToVer108
        };
        (109) => {
            upgradeToVer109
        };
        (110) => {
            upgradeToVer110
        };
        (130) => {
            upgradeToVer130
        };
        (131) => {
            upgradeToVer131
        };
        (132) => {
            upgradeToVer132
        };
        (133) => {
            upgradeToVer133
        };
        (134) => {
            upgradeToVer134
        };
        (135) => {
            upgradeToVer135
        };
        (136) => {
            upgradeToVer136
        };
        (137) => {
            upgradeToVer137
        };
        (138) => {
            upgradeToVer138
        };
        (139) => {
            upgradeToVer139
        };
        (140) => {
            upgradeToVer140
        };
        (141) => {
            upgradeToVer141
        };
        (142) => {
            upgradeToVer142
        };
        (143) => {
            upgradeToVer143
        };
        (144) => {
            upgradeToVer144
        };
        (146) => {
            upgradeToVer146
        };
        (167) => {
            upgradeToVer167
        };
        (168) => {
            upgradeToVer168
        };
        (169) => {
            upgradeToVer169
        };
        (170) => {
            upgradeToVer170
        };
        (171) => {
            upgradeToVer171
        };
        (172) => {
            upgradeToVer172
        };
        (173) => {
            upgradeToVer173
        };
        (174) => {
            upgradeToVer174
        };
        (175) => {
            upgradeToVer175
        };
        (176) => {
            upgradeToVer176
        };
        (177) => {
            upgradeToVer177
        };
        (178) => {
            upgradeToVer178
        };
        (179) => {
            upgradeToVer179
        };
        (190) => {
            upgradeToVer190
        };
        (191) => {
            upgradeToVer191
        };
        (192) => {
            upgradeToVer192
        };
        (193) => {
            upgradeToVer193
        };
        (194) => {
            upgradeToVer194
        };
        (195) => {
            upgradeToVer195
        };
        (196) => {
            upgradeToVer196
        };
        (197) => {
            upgradeToVer197
        };
        (198) => {
            upgradeToVer198
        };
        (209) => {
            upgradeToVer209
        };
        (210) => {
            upgradeToVer210
        };
        (211) => {
            upgradeToVer211
        };
        (212) => {
            upgradeToVer212
        };
        (213) => {
            upgradeToVer213
        };
        (214) => {
            upgradeToVer214
        };
        (215) => {
            upgradeToVer215
        };
        (216) => {
            upgradeToVer216
        };
        (217) => {
            upgradeToVer217
        };
        (218) => {
            upgradeToVer218
        };
        (239) => {
            upgradeToVer239
        };
        (240) => {
            upgradeToVer240
        };
        (241) => {
            upgradeToVer241
        };
        (242) => {
            upgradeToVer242
        };
        (243) => {
            upgradeToVer243
        };
        (244) => {
            upgradeToVer244
        };
        (245) => {
            upgradeToVer245
        };
        (246) => {
            upgradeToVer246
        };
        (247) => {
            upgradeToVer247
        };
        (248) => {
            upgradeToVer248
        };
        (249) => {
            upgradeToVer249
        };
        (250) => {
            upgradeToVer250
        };
        (251) => {
            upgradeToVer251
        };
        (252) => {
            upgradeToVer252
        };
        (253) => {
            upgradeToVer253
        };
        (254) => {
            upgradeToVer254
        };
        (255) => {
            upgradeToVer255
        };
        (256) => {
            upgradeToVer256
        };
        (257) => {
            upgradeToVer257
        };
        (258) => {
            upgradeToVer258
        };
        (259) => {
            upgradeToVer259
        };
        (260) => {
            upgradeToVer260
        };
        (261) => {
            upgradeToVer261
        };
        (262) => {
            upgradeToVer262
        };
    }
    macro_rules! upgrades {
        ($($version:tt),+ $(,)?) => {
            vec![$(
                VersionedUpgradeFunction {
                    version: $version,
                    function: upgrade_function!($version),
                }
            ),+]
        };
    }

    upgrades![
        2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
        27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 40, 41, 42, 43, 44, 45, 46, 47, 50, 52, 53,
        54, 55, 56, 57, 59, 60, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78,
        79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 93, 94, 95, 97, 98, 100, 101, 102, 103,
        104, 105, 106, 107, 108, 109, 110, 130, 131, 132, 133, 134, 135, 136, 137, 138, 139, 140,
        141, 142, 143, 144, 146, 167, 168, 169, 170, 171, 172, 173, 174, 175, 176, 177, 178, 179,
        190, 191, 192, 193, 194, 195, 196, 197, 198, 209, 210, 211, 212, 213, 214, 215, 216, 217,
        218, 239, 240, 241, 242, 243, 244, 245, 246, 247, 248, 249, 250, 251, 252, 253, 254, 255,
        256, 257, 258, 259, 260, 261, 262,
    ]
});

// 以下 upgradeToVerN / 辅助写库函数均为按名分发的升级桩：
// 真正 DDL/DML 由 installUpgradeAction 安装的 handler 执行。
/// 升级到 bootstrap 版本 2。
pub fn upgradeToVer2(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer2");
}

pub fn upgradeToVer3(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer3");
}

pub fn upgradeToVer4(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer4");
}

pub fn upgradeToVer5(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer5");
}

pub fn upgradeToVer6(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer6");
}

pub fn upgradeToVer7(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer7");
}

pub fn upgradeToVer8(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer8");
}

pub fn upgradeToVer9(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer9");
}

/// 可重入 DDL 辅助（升级中重复执行应安全）。
pub fn doReentrantDDL(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("doReentrantDDL");
}

pub fn upgradeToVer10(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer10");
}

pub fn upgradeToVer11(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer11");
}

pub fn upgradeToVer12(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer12");
}

pub fn upgradeToVer13(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer13");
}

pub fn upgradeToVer14(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer14");
}

pub fn upgradeToVer15(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer15");
}

pub fn upgradeToVer16(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer16");
}

pub fn upgradeToVer17(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer17");
}

pub fn upgradeToVer18(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer18");
}

pub fn upgradeToVer19(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer19");
}

pub fn upgradeToVer20(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer20");
}

pub fn upgradeToVer21(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer21");
}

pub fn upgradeToVer22(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer22");
}

pub fn upgradeToVer23(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer23");
}

/// 写入系统时区参数。
pub fn writeSystemTZ(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("writeSystemTZ");
}

pub fn upgradeToVer24(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer24");
}

pub fn upgradeToVer25(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer25");
}

pub fn upgradeToVer26(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer26");
}

pub fn upgradeToVer27(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer27");
}

pub fn upgradeToVer28(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer28");
}

pub fn upgradeToVer29(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer29");
}

pub fn upgradeToVer30(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer30");
}

pub fn upgradeToVer31(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer31");
}

pub fn upgradeToVer32(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer32");
}

pub fn upgradeToVer33(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer33");
}

pub fn upgradeToVer34(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer34");
}

pub fn upgradeToVer35(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer35");
}

pub fn upgradeToVer36(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer36");
}

pub fn upgradeToVer37(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer37");
}

pub fn upgradeToVer38(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer38");
}

/// 写入新排序规则（new collation）参数。
pub fn writeNewCollationParameter(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("writeNewCollationParameter");
}

pub fn upgradeToVer40(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer40");
}

pub fn upgradeToVer41(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer41");
}

/// 写入默认表达式下推黑名单。
pub fn writeDefaultExprPushDownBlacklist(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("writeDefaultExprPushDownBlacklist");
}

pub fn upgradeToVer42(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer42");
}

/// 写入语句摘要相关系统变量。
pub fn writeStmtSummaryVars(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("writeStmtSummaryVars");
}

pub fn upgradeToVer43(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer43");
}

pub fn upgradeToVer44(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer44");
}

pub fn upgradeToVer45(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer45");
}

pub fn upgradeToVer46(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer46");
}

pub fn upgradeToVer47(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer47");
}

pub fn upgradeToVer50(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer50");
}

pub fn upgradeToVer52(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer52");
}

pub fn upgradeToVer53(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer53");
}

pub fn upgradeToVer54(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer54");
}

pub fn upgradeToVer55(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer55");
}

pub fn upgradeToVer56(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer56");
}

pub fn upgradeToVer57(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer57");
}

/// 插入内置 bind_info 行。
pub fn insertBuiltinBindInfoRow(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("insertBuiltinBindInfoRow");
}

pub fn upgradeToVer59(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer59");
}

pub fn upgradeToVer60(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer60");
}

pub fn upgradeToVer67(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer67");
}

/// 更新 bind_info 表内容。
pub fn updateBindInfo(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("updateBindInfo");
}

/// 写入默认查询内存配额。
pub fn writeMemoryQuotaQuery(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("writeMemoryQuotaQuery");
}

pub fn upgradeToVer62(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer62");
}

pub fn upgradeToVer63(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer63");
}

pub fn upgradeToVer64(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer64");
}

pub fn upgradeToVer65(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer65");
}

pub fn upgradeToVer66(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer66");
}

pub fn upgradeToVer68(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer68");
}

pub fn upgradeToVer69(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer69");
}

pub fn upgradeToVer70(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer70");
}

pub fn upgradeToVer71(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer71");
}

pub fn upgradeToVer72(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer72");
}

pub fn upgradeToVer73(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer73");
}

pub fn upgradeToVer74(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer74");
}

pub fn upgradeToVer75(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer75");
}

pub fn upgradeToVer76(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer76");
}

pub fn upgradeToVer77(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer77");
}

pub fn upgradeToVer78(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer78");
}

pub fn upgradeToVer79(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer79");
}

pub fn upgradeToVer80(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer80");
}

pub fn upgradeToVer81(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer81");
}

pub fn upgradeToVer82(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer82");
}

pub fn upgradeToVer83(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer83");
}

pub fn upgradeToVer84(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer84");
}

pub fn upgradeToVer85(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer85");
}

pub fn upgradeToVer86(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer86");
}

pub fn upgradeToVer87(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer87");
}

pub fn upgradeToVer88(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer88");
}

pub fn upgradeToVer89(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer89");
}

/// 导入配置项到系统变量。
pub fn importConfigOption(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("importConfigOption");
}

pub fn upgradeToVer90(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer90");
}

pub fn upgradeToVer91(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer91");
}

pub fn upgradeToVer93(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer93");
}

pub fn upgradeToVer94(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer94");
}

pub fn upgradeToVer95(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer95");
}

pub fn upgradeToVer97(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer97");
}

pub fn upgradeToVer98(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer98");
}

/// 元数据锁相关升级前钩子（version 99）。
pub fn upgradeToVer99Before(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer99Before");
}

/// 元数据锁相关升级后钩子（version 99）。
pub fn upgradeToVer99After(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer99After");
}

pub fn upgradeToVer100(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer100");
}

pub fn upgradeToVer101(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer101");
}

pub fn upgradeToVer102(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer102");
}

pub fn upgradeToVer103(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer103");
}

pub fn upgradeToVer104(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer104");
}

pub fn upgradeToVer105(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer105");
}

pub fn upgradeToVer106(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer106");
}

pub fn upgradeToVer107(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer107");
}

pub fn upgradeToVer108(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer108");
}

pub fn upgradeToVer109(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer109");
}

pub fn upgradeToVer110(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer110");
}

pub fn upgradeToVer130(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer130");
}

pub fn upgradeToVer131(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer131");
}

pub fn upgradeToVer132(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer132");
}

pub fn upgradeToVer133(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer133");
}

pub fn upgradeToVer134(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer134");
}

pub fn upgradeToVer135(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer135");
}

pub fn upgradeToVer136(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer136");
}

pub fn upgradeToVer137(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer137");
}

pub fn upgradeToVer138(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer138");
}

pub fn upgradeToVer139(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer139");
}

pub fn upgradeToVer140(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer140");
}

pub fn upgradeToVer141(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer141");
}

pub fn upgradeToVer142(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer142");
}

pub fn upgradeToVer143(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer143");
}

pub fn upgradeToVer144(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer144");
}

pub fn upgradeToVer146(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer146");
}

pub fn upgradeToVer167(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer167");
}

pub fn upgradeToVer168(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer168");
}

pub fn upgradeToVer169(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer169");
}

pub fn upgradeToVer170(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer170");
}

pub fn upgradeToVer171(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer171");
}

pub fn upgradeToVer172(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer172");
}

pub fn upgradeToVer173(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer173");
}

pub fn upgradeToVer174(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer174");
}

pub fn upgradeToVer175(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer175");
}

pub fn upgradeToVer176(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer176");
}

pub fn upgradeToVer177(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer177");
}

/// 写入 DDL 系统表版本。
pub fn writeDDLTableVersion(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("writeDDLTableVersion");
}

pub fn upgradeToVer178(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer178");
}

pub fn upgradeToVer179(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer179");
}

pub fn upgradeToVer190(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer190");
}

pub fn upgradeToVer191(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer191");
}

pub fn upgradeToVer192(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer192");
}

pub fn upgradeToVer193(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer193");
}

pub fn upgradeToVer194(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer194");
}

pub fn upgradeToVer195(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer195");
}

pub fn upgradeToVer196(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer196");
}

pub fn upgradeToVer197(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer197");
}

pub fn upgradeToVer198(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer198");
}

pub fn upgradeToVer209(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer209");
}

pub fn upgradeToVer210(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer210");
}

pub fn upgradeToVer211(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer211");
}

pub fn upgradeToVer212(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer212");
}

pub fn upgradeToVer213(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer213");
}

pub fn upgradeToVer214(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer214");
}

pub fn upgradeToVer215(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer215");
}

pub fn upgradeToVer216(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer216");
}

pub fn upgradeToVer217(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer217");
}

pub fn upgradeToVer218(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer218");
}

pub fn upgradeToVer239(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer239");
}

pub fn upgradeToVer240(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer240");
}

pub fn upgradeToVer241(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer241");
}

/// 写入集群 ID。
pub fn writeClusterID(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("writeClusterID");
}

pub fn upgradeToVer242(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer242");
}

pub fn upgradeToVer243(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer243");
}

pub fn upgradeToVer244(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer244");
}

pub fn upgradeToVer245(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer245");
}

pub fn upgradeToVer246(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer246");
}

pub fn upgradeToVer247(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer247");
}

pub fn upgradeToVer248(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer248");
}

pub fn upgradeToVer249(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer249");
}

pub fn upgradeToVer250(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer250");
}

pub fn upgradeToVer251(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer251");
}

pub fn upgradeToVer252(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer252");
}

pub fn upgradeToVer253(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer253");
}

pub fn upgradeToVer254(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer254");
}

pub fn upgradeToVer255(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer255");
}

pub fn upgradeToVer256(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer256");
}

pub fn upgradeToVer257(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer257");
}

pub fn upgradeToVer258(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer258");
}

/// 回填 ignore-inlist plan digest 相关变量。
pub fn upgradeToVer259(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer259");
}

/// 升级到版本 260。
pub fn upgradeToVer260(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer260");
}

/// 回填默认字符串匹配选择率。
pub fn upgradeToVer261(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer261");
}

/// 刷新 binding digest 算法。
pub fn upgradeToVer262(_s: &sessionapi::Session, _version: i64) {
    upgrade_action("upgradeToVer262");
}

/// 安装生产环境按名执行升级 DDL/DML 的 handler；未安装则升级必须失败。
/// Installs the production executor for version-specific DDL/DML.  Leaving the
/// boundary unwired is fatal, so an upgrade can never be reported as successful
/// after silently skipping a migration.
pub fn installUpgradeAction(handler: fn(&str)) -> Result<(), fn(&str)> {
    UPGRADE_ACTION.set(handler)
}

/// 已安装的升级动作回调。
static UPGRADE_ACTION: OnceLock<fn(&str)> = OnceLock::new();

/// 按升级函数名调用已安装的执行器；未安装则 panic。
fn upgrade_action(name: &str) {
    let handler = UPGRADE_ACTION
        .get()
        .unwrap_or_else(|| panic!("upgrade executor is not installed for {name}"));
    handler(name);
}
/// 占位 Session 类型，避免本定义文件依赖完整 session API。
mod sessionapi {
    pub struct Session;
}
