// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// Placement Policy（放置策略）元数据模型。
//
// Placement 描述 Region 副本应落在哪些可用区/标签上（主区域、投票者、学习者等约束）。
// Region 是键空间分片单位；本模块只序列化与格式化策略文本，不下发 PD 调度规则。

// 不会下发 PD placement rule、访问数据库或改变副本调度；AST、SchemaState 等类型沿用外部/同包名称。

use serde::{Deserialize, Serialize};
use std::time::Duration;

// PolicyRefInfo 对应元数据中对 placement policy 的稳定 ID/名称引用。
/// 对 placement policy 的稳定 ID/名称引用。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PolicyRefInfo {
    /// 策略 ID。
    #[serde(rename = "id")]
    /// 策略名称。
    pub ID: i64,
    pub Name: ast::CIStr,
}

// PlacementSettings 按 Go 字段顺序保存区域、副本数、约束与存活偏好。
/// 放置设置正文：区域、副本数、约束与存活偏好。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PlacementSettings {
    /// 主区域名称。
    pub PrimaryRegion: String,
    /// 参与调度的区域列表（逗号分隔）。
    pub Regions: String,
    /// learner 副本数。
    pub Learners: u64,
    /// follower 副本数。
    pub Followers: u64,
    /// voter 副本数。
    pub Voters: u64,
    /// 调度策略名（如 EVEN）。
    pub Schedule: String,
    /// 通用约束表达式。
    pub Constraints: String,
    /// Leader 约束。
    pub LeaderConstraints: String,
    /// Learner 约束。
    pub LearnerConstraints: String,
    /// Follower 约束。
    pub FollowerConstraints: String,
    /// Voter 约束。
    pub VoterConstraints: String,
    /// 存活偏好（survival preferences）。
    pub SurvivalPreferences: String,
}

impl PlacementSettings {
    // String 对应 Go fmt.Stringer，跳过零值并严格维持各设置项的历史输出顺序。
    /// 生成 CREATE/ALTER POLICY 风格的设置字符串，跳过零值并保持历史字段顺序。
    pub fn String(&self) -> String {
        let mut sb = String::new();
        if !self.PrimaryRegion.is_empty() {
            writeSettingStringToBuilder(&mut sb, "PRIMARY_REGION", &self.PrimaryRegion, &mut []);
        }
        if !self.Regions.is_empty() {
            writeSettingStringToBuilder(&mut sb, "REGIONS", &self.Regions, &mut []);
        }
        if !self.Schedule.is_empty() {
            writeSettingStringToBuilder(&mut sb, "SCHEDULE", &self.Schedule, &mut []);
        }
        if !self.Constraints.is_empty() {
            writeSettingStringToBuilder(&mut sb, "CONSTRAINTS", &self.Constraints, &mut []);
        }
        if !self.LeaderConstraints.is_empty() {
            writeSettingStringToBuilder(
                &mut sb,
                "LEADER_CONSTRAINTS",
                &self.LeaderConstraints,
                &mut [],
            );
        }
        if self.Voters > 0 {
            writeSettingIntegerToBuilder(&mut sb, "VOTERS", self.Voters, &mut []);
        }
        if !self.VoterConstraints.is_empty() {
            writeSettingStringToBuilder(
                &mut sb,
                "VOTER_CONSTRAINTS",
                &self.VoterConstraints,
                &mut [],
            );
        }
        if self.Followers > 0 {
            writeSettingIntegerToBuilder(&mut sb, "FOLLOWERS", self.Followers, &mut []);
        }
        if !self.FollowerConstraints.is_empty() {
            writeSettingStringToBuilder(
                &mut sb,
                "FOLLOWER_CONSTRAINTS",
                &self.FollowerConstraints,
                &mut [],
            );
        }
        if self.Learners > 0 {
            writeSettingIntegerToBuilder(&mut sb, "LEARNERS", self.Learners, &mut []);
        }
        if !self.LearnerConstraints.is_empty() {
            writeSettingStringToBuilder(
                &mut sb,
                "LEARNER_CONSTRAINTS",
                &self.LearnerConstraints,
                &mut [],
            );
        }
        if !self.SurvivalPreferences.is_empty() {
            writeSettingStringToBuilder(
                &mut sb,
                "SURVIVAL_PREFERENCES",
                &self.SurvivalPreferences,
                &mut [],
            );
        }
        sb
    }

    // Clone 对应 Go 的值拷贝；字符串均拥有自身内容。
    /// 值拷贝副本。
    pub fn Clone(&self) -> Self {
        self.clone()
    }
}

// SeparatorFn 对应 Go 的 variadic func()；Rust 把 builder 显式传入，避免闭包捕获造成隐式共享可变状态。
/// 分隔符回调；对应 Go variadic separator，显式传入 builder 避免闭包共享可变状态。
pub type SeparatorFn<'a> = Box<dyn FnMut(&mut String) + 'a>;

// writeSettingStringToBuilder 生成 ITEM="value"，并按 Go 逻辑只转义值中的双引号。
/// 写入 `ITEM="value"`，仅转义值内双引号。
pub fn writeSettingStringToBuilder(
    sb: &mut String,
    item: &str,
    value: &str,
    separatorFns: &mut [SeparatorFn<'_>],
) {
    let escaped = value.replace('"', "\\\"");
    writeSettingItemToBuilder(sb, &format!("{item}=\"{escaped}\""), separatorFns);
}

// writeSettingIntegerToBuilder 生成无引号的无符号整数设置项。
/// 写入无引号的无符号整数设置项。
pub fn writeSettingIntegerToBuilder(
    sb: &mut String,
    item: &str,
    value: u64,
    separatorFns: &mut [SeparatorFn<'_>],
) {
    writeSettingItemToBuilder(sb, &format!("{item}={value}"), separatorFns);
}

// writeSettingDurationToBuilder 对应 time.Duration.String；当前调用来自毫秒配置，整秒时去掉小数。
/// 写入 duration 设置项；格式对齐 Go `time.Duration.String`。
pub fn writeSettingDurationToBuilder(
    sb: &mut String,
    item: &str,
    dur: Duration,
    separatorFns: &mut [SeparatorFn<'_>],
) {
    let value = formatGoDuration(dur);
    writeSettingStringToBuilder(sb, item, &value, separatorFns);
}

// formatGoDuration 对齐 Go time.Duration.String，覆盖非负 std::time::Duration。
/// 将 Duration 格式化为 Go 风格文本（如 `1h2m3s`、`500ms`）。
// formatGoDuration mirrors time.Duration.String for the non-negative durations
// represented by std::time::Duration.
pub fn formatGoDuration(dur: Duration) -> String {
    let nanos = dur.as_nanos();
    if nanos == 0 {
        return "0s".to_owned();
    }

    const NS_PER_US: u128 = 1_000;
    const NS_PER_MS: u128 = 1_000_000;
    const NS_PER_SEC: u128 = 1_000_000_000;
    const NS_PER_MIN: u128 = 60 * NS_PER_SEC;
    const NS_PER_HOUR: u128 = 60 * NS_PER_MIN;

        // 按单位拆分整数与小数部分，去掉尾部多余零。
    fn decimal(value: u128, unit: u128, suffix: &str) -> String {
        let whole = value / unit;
        let remainder = value % unit;
        if remainder == 0 {
            return format!("{whole}{suffix}");
        }
        let width = unit.ilog10() as usize;
        let fraction = format!("{remainder:0width$}")
            .trim_end_matches('0')
            .to_owned();
        format!("{whole}.{fraction}{suffix}")
    }

    // 不足一秒：按 ms / µs / ns 选择合适单位。
    if nanos < NS_PER_SEC {
        return if nanos >= NS_PER_MS {
            decimal(nanos, NS_PER_MS, "ms")
        } else if nanos >= NS_PER_US {
            decimal(nanos, NS_PER_US, "µs")
        } else {
            format!("{nanos}ns")
        };
    }

    let mut remaining = nanos;
    // 不少于一秒：依次输出 h、m、s（整分时补 0s）。
    let mut out = String::new();
    if remaining >= NS_PER_HOUR {
        let hours = remaining / NS_PER_HOUR;
        remaining %= NS_PER_HOUR;
        out.push_str(&format!("{hours}h"));
    }
    if !out.is_empty() || remaining >= NS_PER_MIN {
        let minutes = remaining / NS_PER_MIN;
        remaining %= NS_PER_MIN;
        out.push_str(&format!("{minutes}m"));
    }
    if remaining != 0 || out.is_empty() {
        out.push_str(&decimal(remaining, NS_PER_SEC, "s"));
    } else if out.ends_with('m') {
        out.push_str("0s");
    }
    out
}

// writeSettingItemToBuilder 是公共收尾：非首项先运行所有分隔函数；未提供函数时写一个空格。
/// 公共收尾：非首项先跑分隔函数，未提供时写入单个空格。
pub fn writeSettingItemToBuilder(
    sb: &mut String,
    item: &str,
    separatorFns: &mut [SeparatorFn<'_>],
) {
    if !sb.is_empty() {
        if separatorFns.is_empty() {
            sb.push(' ');
        } else {
            for separator in separatorFns {
                separator(sb);
            }
        }
    }
    sb.push_str(item);
}

// PolicyInfo 对应嵌入 PlacementSettings 的 Go 结构。
/// 完整 policy 元数据：嵌入 PlacementSettings，并带 ID/名称/schema 状态。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PolicyInfo {
    /// 嵌入的放置设置。
    #[serde(flatten)]
    pub PlacementSettings: PlacementSettings,
    /// 策略 ID。
    #[serde(rename = "id")]
    /// 策略名称。
    pub ID: i64,
    /// schema 状态。
    pub Name: ast::CIStr,
    pub State: SchemaState,
}

impl PolicyInfo {
    // Clone 明确深拷贝嵌入的 settings，保持 Go 手工 Clone 的所有权边界。
    /// 深拷贝嵌入 settings，保持 Go 手工 Clone 的所有权边界。
    pub fn Clone(&self) -> Self {
        Self {
            PlacementSettings: self.PlacementSettings.Clone(),
            ..self.clone()
        }
    }
}
