// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

//! Check result template matching Go `check_template.go`.
//!
//! 中文概览：这个文件定义 importer 预检查结果如何被收集、计数和最终渲染出来。
//! `Template` trait 是抽象接口，约束调用方可以收集结果、判断成功与否、读取失败计数并输出摘要。
//! `SimpleTemplate` 则提供一份与 Go 版简单模板对齐的默认实现。
//! 它把每条检查记录保存成 `TemplateRow`，同时分别累计 warning 与 critical 失败次数。
//! 这样的拆分让调用方既能拿到总表格，也能单独读取关键失败信息。
//! `Collect` 的职责不是只追加一行文本，而是同步维护计数、关键失败消息和展示顺序。
//! `Success` 围绕 critical 失败数判断，体现“warning 不阻断导入、critical 才阻断”的规则。
//! `Output` 则用最小 go-pretty 风格复刻表格渲染，并保留 warning/critical 的颜色区分。
//! 因为很多预检查输出最终会直接展示给用户，所以这里保护的是用户可见格式而非内部存储。
//! 中文注释重点说明：这不是通用表格库，而是 importer 预检查摘要的专用模板。
//! 换句话说，这里的核心价值是把“结果收集”和“结果展示”绑定在同一份稳定协议上。
//! 只要该协议不漂移，上层控制器就能继续像 Go 一样先累积结果、最后统一输出。
//! 若未来要替换模板实现，也应优先回归失败计数、关键消息拼接和表格可读性这三件事。
//! 这也是为什么文件虽然不大，却承担了较高的用户可见面风险。

use astersql_lightning_pkg_precheck::{self as precheck, CheckType, Critical, Warn};

/// Template is the interface for lightning check.
pub trait Template: Send {
    // trait 只暴露 importer 真正依赖的五个动作，避免模板实现承担多余职责。
    fn Collect(&mut self, t: CheckType, passed: bool, msg: String);
    fn Success(&self) -> bool;
    fn FailedCount(&self, t: CheckType) -> i32;
    fn Output(&mut self) -> String;
    fn FailedMsg(&self) -> String;
}

#[derive(Clone, Debug)]
struct TemplateRow {
    // 每一行都保留序号、消息、严重级别和是否通过，
    // 便于后续渲染表格时复用同一份结构化数据。
    idx: i32,
    msg: String,
    typ: CheckType,
    passed: bool,
}

/// SimpleTemplate is a simple template for lightning check.
pub struct SimpleTemplate {
    // 公开字段主要服务测试观察；真正的渲染顺序仍由内部 `rows` 保存。
    pub count: i32,
    pub warnFailedCount: i32,
    pub criticalFailedCount: i32,
    pub normalMsgs: Vec<String>,
    pub criticalMsgs: Vec<String>,
    rows: Vec<TemplateRow>,
}

/// NewSimpleTemplate returns a simple template.
pub fn NewSimpleTemplate() -> Box<dyn Template> {
    // 返回 trait object 是为了让上层控制器按接口持有模板，而不绑定具体实现。
    Box::new(SimpleTemplate {
        count: 0,
        warnFailedCount: 0,
        criticalFailedCount: 0,
        normalMsgs: Vec::new(),
        criticalMsgs: Vec::new(),
        rows: Vec::new(),
    })
}

impl Template for SimpleTemplate {
    fn FailedMsg(&self) -> String {
        // 关键失败消息使用 `;\n` 拼接，方便直接挂到最终错误文本里。
        self.criticalMsgs.join(";\n")
    }

    fn Collect(&mut self, t: CheckType, passed: bool, msg: String) {
        // 这里一次性维护总数、失败计数、消息列表和表格行，确保各视图共享同一来源。
        self.count += 1;
        if !passed {
            if t == Critical {
                self.criticalFailedCount += 1;
            } else if t == Warn {
                self.warnFailedCount += 1;
            }
        }
        if !passed && t == Critical {
            self.criticalMsgs.push(msg.clone());
        } else {
            self.normalMsgs.push(msg.clone());
        }
        self.rows.push(TemplateRow {
            idx: self.count,
            msg,
            typ: t,
            passed,
        });
    }

    fn Success(&self) -> bool {
        // 只有 critical 失败会阻断导入，warning 只影响展示和统计。
        // 该判定规则与 Lightning 的 fail-fast 语义直接相连。
        self.criticalFailedCount == 0
    }

    fn FailedCount(&self, t: CheckType) -> i32 {
        // 计数按严重级别分流，调用方可以分别读取 warning 与 critical 的失败数。
        if t == Warn {
            return self.warnFailedCount;
        }
        if t == Critical {
            return self.criticalFailedCount;
        }
        0
    }

    fn Output(&mut self) -> String {
        // 这里刻意只实现 importer 需要的最小 go-pretty 风格，
        // 重点保护列宽、边框和失败项颜色，而不是通用表格能力。
        // 这样测试仍能验证输出观感是否与 Go 接近，但不会额外引入庞大的表格依赖。
        const FG_RED: &str = "\x1b[31m";
        const FG_YELLOW: &str = "\x1b[33m";
        const RESET: &str = "\x1b[0m";

        let headers = ["#", "CHECK ITEM", "TYPE", "PASSED"];
        let mut body: Vec<Vec<String>> = Vec::new();
        for r in &self.rows {
            body.push(vec![
                r.idx.to_string(),
                r.msg.clone(),
                r.typ.to_string(),
                r.passed.to_string(),
            ]);
        }

        let ncols = headers.len();
        let mut widths = vec![0usize; ncols];
        for (i, h) in headers.iter().enumerate() {
            widths[i] = h.len();
        }
        for row in &body {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(display_width(cell));
            }
        }
        for (width, maximum) in widths.iter_mut().zip([6, 130, 20, 6]) {
            *width = (*width).min(maximum);
        }
        while widths.iter().sum::<usize>() + widths.len() * 3 + 1 > 170 {
            let widest = widths
                .iter()
                .enumerate()
                .max_by_key(|(_, width)| *width)
                .map(|(index, _)| index)
                .expect("the table always has columns");
            widths[widest] -= 1;
        }

        let sep = {
            // 边框宽度按当前表头和内容动态计算，确保长消息也能被完整包住。
            // 这让输出不依赖预设固定宽度，行为更接近 Go 模板。
            let mut s = String::from("+");
            for w in &widths {
                s.push_str(&"-".repeat(w + 2));
                s.push('+');
            }
            s
        };

        let mut out = String::new();
        out.push_str(&sep);
        out.push('\n');
        out.push('|');
        for (i, h) in headers.iter().enumerate() {
            out.push(' ');
            out.push_str(&format!("{:width$}", h, width = widths[i]));
            out.push(' ');
            out.push('|');
        }
        out.push('\n');
        out.push_str(&sep);
        out.push('\n');

        for (ri, row) in body.iter().enumerate() {
            // 颜色仅在失败行上生效，并根据 warning/critical 选黄或红。
            // 通过行级着色，用户在长表格里也能先看到真正需要处理的异常项。
            let color = {
                let r = &self.rows[ri];
                if !r.passed {
                    if r.typ == Warn {
                        Some(FG_YELLOW)
                    } else if r.typ == Critical {
                        Some(FG_RED)
                    } else {
                        None
                    }
                } else {
                    None
                }
            };
            let cells: Vec<Vec<String>> = row
                .iter()
                .enumerate()
                .map(|(i, cell)| wrap_cell(cell, widths[i]))
                .collect();
            let height = cells.iter().map(Vec::len).max().unwrap_or(1);
            for line in 0..height {
                out.push('|');
                for (i, cell) in cells.iter().enumerate() {
                    let content = cell.get(line).map(String::as_str).unwrap_or("");
                    let padded = pad_cell(content, widths[i], i == 0);
                    if let Some(c) = color {
                        out.push_str(c);
                        out.push(' ');
                        out.push_str(&padded);
                        out.push(' ');
                        out.push_str(RESET);
                    } else {
                        out.push(' ');
                        out.push_str(&padded);
                        out.push(' ');
                    }
                    out.push('|');
                }
                out.push('\n');
            }
            out.push_str(&sep);
            out.push('\n');
        }
        out.push('\n');
        out
    }
}

fn display_width(value: &str) -> usize {
    value.chars().count()
}

fn wrap_cell(value: &str, width: usize) -> Vec<String> {
    if value.is_empty() {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    for source_line in value.split('\n') {
        let chars: Vec<char> = source_line.chars().collect();
        if chars.is_empty() {
            lines.push(String::new());
        } else {
            lines.extend(chars.chunks(width).map(|chunk| chunk.iter().collect()));
        }
    }
    lines
}

fn pad_cell(value: &str, width: usize, right_align: bool) -> String {
    let padding = width.saturating_sub(display_width(value));
    if right_align {
        format!("{}{}", " ".repeat(padding), value)
    } else {
        format!("{}{}", value, " ".repeat(padding))
    }
}
