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

// 资源组（Resource Group）元数据模型。
//
// 资源组用于按 RU（Request Unit，请求单元）与优先级隔离租户/工作负载的 CPU、IO 与突发能力。
// 本模块描述设置结构、失控查询（runaway）与后台任务限制，并格式化为 SQL 风格文本。

// AST 枚举、SchemaState 与 placement builder 等依赖沿用外部/同包名称，等待后续模块接线。
//

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

use placement::{
    SeparatorFn, formatGoDuration, writeSettingDurationToBuilder, writeSettingIntegerToBuilder,
    writeSettingItemToBuilder, writeSettingStringToBuilder,
};

// unlimitedRURate 沿用 Go math.MaxInt32；该特殊值同时影响 burst limit 的解释。
/// 表示无限 RU 速率的特殊值（Go math.MaxInt32），同时影响 burst limit 解释。
pub const unlimitedRURate: u64 = i32::MAX as u64;

// ResourceGroupRunawaySettings 保存触发阈值、处置动作和观察规则。
/// 失控查询触发阈值、处置动作与观察（watch）规则。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ResourceGroupRunawaySettings {
    /// 执行耗时阈值（毫秒）。
    #[serde(rename = "exec_elapsed_time_ms")]
    pub ExecElapsedTimeMs: u64,
    /// 已处理 key 数阈值。
    #[serde(rename = "processed_keys")]
    pub ProcessedKeys: i64,
    /// 消耗 RU 阈值。
    #[serde(rename = "request_unit")]
    pub RequestUnit: i64,
    /// 触发后的处置动作。
    #[serde(rename = "action")]
    pub Action: ast::RunawayActionType,
    /// 切换目标资源组名（SwitchGroup 动作时使用）。
    #[serde(rename = "switch_group_name")]
    pub SwitchGroupName: String,
    /// 观察类型。
    #[serde(rename = "watch_type")]
    pub WatchType: ast::RunawayWatchType,
    /// 观察持续时长（毫秒）；<=0 表示无限。
    #[serde(rename = "watch_duration_ms")]
    pub WatchDurationMs: i64,
}

// ResourceGroupBackgroundSettings 保存允许的后台任务类型和资源利用率上限。
/// 后台任务允许类型与资源利用率上限。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ResourceGroupBackgroundSettings {
    /// 允许的后台任务类型列表。
    #[serde(rename = "job_types")]
    pub JobTypes: Vec<String>,
    /// 资源利用率上限。
    #[serde(rename = "utilization_limit")]
    pub ResourceUtilLimit: u64,
}

// ResourceGroupSettings 对应 Go 设置结构；Arc 复现 Go 指针字段在浅拷贝 Clone 后共享子配置的语义。
/// 资源组设置正文；Arc 子配置在浅拷贝后共享，对齐 Go 指针语义。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ResourceGroupSettings {
    /// 每秒 RU 配额。
    #[serde(rename = "ru_per_sec")]
    /// 调度优先级。
    pub RURate: u64,
    #[serde(rename = "priority")]
    /// CPU 限制描述。
    pub Priority: u64,
    #[serde(rename = "cpu_limit")]
    /// 读 IO 带宽限制。
    pub CPULimiter: String,
    #[serde(rename = "io_read_bandwidth")]
    /// 写 IO 带宽限制。
    pub IOReadBandwidth: String,
    #[serde(rename = "io_write_bandwidth")]
    /// 突发容量；负值有特殊语义（-1 unlimited，-2 moderated）。
    pub IOWriteBandwidth: String,
    #[serde(rename = "burst_limit")]
    /// 失控查询设置（可选，共享指针）。
    pub BurstLimit: i64,
    #[serde(rename = "runaway")]
    /// 后台任务设置（可选，共享指针）。
    pub Runaway: Option<Arc<ResourceGroupRunawaySettings>>,
    #[serde(rename = "background")]
    pub Background: Option<Arc<ResourceGroupBackgroundSettings>>,
}

impl ResourceGroupSettings {
    // GetBurstLimitAdjusted 把 unlimited RU 的特殊值统一投影为 -1，其余情况返回已存 burst limit。
    /// 无限 RU 时将 burst 投影为 -1，否则返回已存 BurstLimit。
    pub fn GetBurstLimitAdjusted(&self) -> i64 {
        if self.RURate == unlimitedRURate {
            -1
        } else {
            self.BurstLimit
        }
    }

    // String 对应 Go fmt.Stringer，保持各部分顺序、逗号/空格和嵌套括号格式。
    /// 生成资源组设置的 SQL 风格字符串，保持字段顺序与括号格式。
    pub fn String(&self) -> String {
        let mut sb = String::new();
        let mut comma: [SeparatorFn<'_>; 1] = [Box::new(|out| out.push_str(", "))];

        if self.RURate != 0 {
            writeSettingIntegerToBuilder(&mut sb, "RU_PER_SEC", self.RURate, &mut comma);
        }
        writeSettingItemToBuilder(
            &mut sb,
            &format!("PRIORITY={}", ast::PriorityValueToName(self.Priority)),
            &mut comma,
        );
        if !self.CPULimiter.is_empty() {
            writeSettingStringToBuilder(&mut sb, "CPU", &self.CPULimiter, &mut comma);
        }
        if !self.IOReadBandwidth.is_empty() {
            writeSettingStringToBuilder(
                &mut sb,
                "IO_READ_BANDWIDTH",
                &self.IOReadBandwidth,
                &mut comma,
            );
        }
        if !self.IOWriteBandwidth.is_empty() {
            writeSettingStringToBuilder(
                &mut sb,
                "IO_WRITE_BANDWIDTH",
                &self.IOWriteBandwidth,
                &mut comma,
            );
        }

        // -2 表示适度 burst，-1 表示 unlimited；其他值不额外输出 BURSTABLE 项。
        match self.BurstLimit {
            -2 => writeSettingItemToBuilder(&mut sb, "BURSTABLE(MODERATED)", &mut comma),
            -1 => writeSettingItemToBuilder(&mut sb, "BURSTABLE(UNLIMITED)", &mut comma),
            _ => {}
        }

        if let Some(runaway) = &self.Runaway {
            sb.push_str(", QUERY_LIMIT=(");
            let mut first_param = true;
            // 按条件拼接 QUERY_LIMIT 括号内参数。
            if runaway.ExecElapsedTimeMs > 0 {
                let duration = format_go_duration(runaway.ExecElapsedTimeMs);
                sb.push_str(&format!("EXEC_ELAPSED=\"{duration}\""));
                first_param = false;
            }
            if runaway.ProcessedKeys > 0 {
                if !first_param {
                    sb.push(' ');
                }
                sb.push_str(&format!("PROCESSED_KEYS={}", runaway.ProcessedKeys));
                first_param = false;
            }
            if runaway.RequestUnit > 0 {
                if !first_param {
                    sb.push(' ');
                }
                sb.push_str(&format!("RU={}", runaway.RequestUnit));
            }

            // ACTION/WATCH 使用 placement 的默认空格分隔，保持括号内旧格式。
            let action = if runaway.Action == ast::RunawayActionSwitchGroup {
                format!(
                    "ACTION={}({})",
                    runaway.Action.to_string(),
                    runaway.SwitchGroupName
                )
            } else {
                format!("ACTION={}", runaway.Action)
            };
            writeSettingItemToBuilder(&mut sb, &action, &mut []);

            if runaway.WatchType != ast::WatchNone {
                writeSettingItemToBuilder(
                    &mut sb,
                    &format!("WATCH={}", runaway.WatchType),
                    &mut [],
                );
                if runaway.WatchDurationMs > 0 {
                    writeSettingDurationToBuilder(
                        &mut sb,
                        "DURATION",
                        Duration::from_millis(runaway.WatchDurationMs as u64),
                        &mut [],
                    );
                } else {
                    writeSettingItemToBuilder(&mut sb, "DURATION=UNLIMITED", &mut []);
                }
            }
            sb.push(')');
        }

        if let Some(background) = &self.Background {
            sb.push_str(", BACKGROUND=(");
            let mut first = true;
            if !background.JobTypes.is_empty() {
                sb.push_str(&format!("TASK_TYPES='{}'", background.JobTypes.join(",")));
                first = false;
            }
            if background.ResourceUtilLimit > 0 {
                if !first {
                    sb.push_str(", ");
                }
                sb.push_str(&format!(
                    "UTILIZATION_LIMIT={}",
                    background.ResourceUtilLimit
                ));
            }
            sb.push(')');
        }
        sb
    }

    // Adjust 将普通非负 burst capacity 同步为 RU rate；两个负值和 unlimited RU 均保留特殊语义。
    /// 将普通非负 burst 同步为 RU rate；负值与无限 RU 保留特殊语义。
    pub fn Adjust(&mut self) {
        if self.RURate != unlimitedRURate && self.BurstLimit >= 0 {
            self.BurstLimit = self.RURate as i64;
        }
    }

    // Clone 对应 Go 的结构体浅拷贝；Arc 子设置继续共享，字符串与标量按值复制。
    /// 浅拷贝；Arc 子设置继续共享。
    pub fn Clone(&self) -> Self {
        self.clone()
    }
}

// NewResourceGroupSettings 对应 Go 构造器；优先级采用 parser/ast 的 medium 默认值。
/// 构造默认资源组设置（优先级为 medium）。
pub fn NewResourceGroupSettings() -> ResourceGroupSettings {
    ResourceGroupSettings {
        Priority: ast::MediumPriorityValue,
        ..Default::default()
    }
}

// format_go_duration 覆盖本文件由毫秒生成的 Go duration 文本：整秒写 s，否则写 ms。
/// 将毫秒格式化为 Go duration 文本（整秒写 s，否则写 ms）。
fn format_go_duration(milliseconds: u64) -> String {
    formatGoDuration(Duration::from_millis(milliseconds))
}

// ResourceGroupInfo 对应嵌入 ResourceGroupSettings 的 Go 元数据结构。
/// 完整资源组元数据：嵌入设置并带 ID/名称/schema 状态。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ResourceGroupInfo {
    /// 嵌入的资源组设置。
    #[serde(flatten)]
    pub ResourceGroupSettings: ResourceGroupSettings,
    /// 资源组 ID。
    #[serde(rename = "id")]
    /// 资源组名称。
    pub ID: i64,
    #[serde(rename = "name")]
    /// schema 状态。
    pub Name: ast::CIStr,
    #[serde(rename = "state")]
    pub State: SchemaState,
}

impl ResourceGroupInfo {
    // Clone 明确克隆 settings 外层值，并保留其中 runaway/background 指针共享关系。
    /// 克隆 settings 外层值，保留 runaway/background 指针共享。
    pub fn Clone(&self) -> Self {
        Self {
            ResourceGroupSettings: self.ResourceGroupSettings.Clone(),
            ..self.clone()
        }
    }
}
