// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 基于 JSON 套件的测试数据加载与录制。
//
// 从 `{suite}_in.json` / `{suite}_out.json`（及可选的 Cascades 输出
// `{suite}_xut.json`）加载用例；开启录制模式（[`Record`]）时可写回期望输出。
// Drop 时若有脏数据会自动 [`TestData::flush`]。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{TestError, TestResult};

/// 全局录制开关：为 true 时允许写回期望输出文件。
static RECORD: AtomicBool = AtomicBool::new(false);

/// 单个命名用例：名称与 JSON `cases` 载荷。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TestCase {
    // Go's encoding/json emits the exported field names by default.
    #[serde(rename = "Name", alias = "name")]
    pub name: String,
    #[serde(rename = "Cases", alias = "cases", default)]
    pub cases: Value,
}

/// 一套测试数据：输入、标准输出与 Cascades（xut）输出。
#[derive(Debug)]
pub struct TestData {
    directory: PathBuf,
    suite_name: String,
    input: Vec<TestCase>,
    output: Vec<TestCase>,
    cascades_output: Vec<TestCase>,
    /// 用例名 → 下标，加速按名查找。
    positions: HashMap<String, usize>,
    /// Whether a cascades `_xut.json` file was loaded for this suite.
    cascades_loaded: bool,
    /// 普通输出录制后是否尚未 flush。
    output_dirty: bool,
    /// Cascades 输出录制后是否尚未 flush。
    cascades_dirty: bool,
}

impl TestData {
    /// 从目录加载套件：校验 in/out 用例数量与名称一一对应。
    pub fn load(directory: impl AsRef<Path>, suite_name: &str) -> TestResult<Self> {
        Self::load_with_cascades(directory, suite_name, false)
    }

    /// Load a suite and, when requested, require its cascades output file.
    pub fn load_with_cascades(
        directory: impl AsRef<Path>,
        suite_name: &str,
        cascades: bool,
    ) -> TestResult<Self> {
        let directory = directory.as_ref().to_path_buf();
        let input = read_cases(directory.join(format!("{suite_name}_in.json")))?;
        let output = read_cases(directory.join(format!("{suite_name}_out.json")))?;
        let cascades_path = directory.join(format!("{suite_name}_xut.json"));
        // 无 xut 文件时用标准 out 作为 Cascades 期望的回退。
        let (cascades_output, cascades_loaded) = if cascades {
            if !cascades_path.exists() {
                return Err(TestError::new(format!(
                    "read {}: file does not exist",
                    cascades_path.display()
                )));
            }
            (read_cases(cascades_path)?, true)
        } else {
            (Vec::new(), false)
        };
        let positions = input
            .iter()
            .enumerate()
            .map(|(index, case)| (case.name.clone(), index))
            .collect();
        if input.len() != output.len() {
            return Err(TestError::new("testdata input/output case count mismatch"));
        }
        for (input, output) in input.iter().zip(output.iter()) {
            if input.name != output.name {
                return Err(TestError::new(format!(
                    "testdata case mismatch: {} != {}",
                    input.name, output.name
                )));
            }
        }
        if cascades_loaded {
            if input.len() != cascades_output.len() {
                return Err(TestError::new(
                    "testdata input/cascades case count mismatch",
                ));
            }
            for (input, output) in input.iter().zip(cascades_output.iter()) {
                if input.name != output.name {
                    return Err(TestError::new(format!(
                        "testdata case mismatch: {} != {}",
                        input.name, output.name
                    )));
                }
            }
        }
        Ok(Self {
            directory,
            suite_name: suite_name.to_owned(),
            input,
            output,
            cascades_output,
            positions,
            cascades_loaded,
            output_dirty: false,
            cascades_dirty: false,
        })
    }

    /// 按用例名返回 (输入 cases, 期望 cases)；`cascades` 选 xut 或 out。
    pub fn LoadTestCasesByName(&self, name: &str, cascades: bool) -> TestResult<(Value, Value)> {
        let index = self
            .positions
            .get(name)
            .copied()
            .ok_or_else(|| TestError::new(format!("test case {name:?} does not exist")))?;
        let output = if cascades {
            if !self.cascades_loaded {
                return Err(TestError::new("testdata cascades output is not loaded"));
            }
            &self.cascades_output
        } else {
            &self.output
        };
        Ok((self.input[index].cases.clone(), output[index].cases.clone()))
    }

    /// 在录制模式下覆盖指定用例的期望输出，并标记 dirty。
    pub fn RecordTestCasesByName(
        &mut self,
        name: &str,
        value: Value,
        cascades: bool,
    ) -> TestResult {
        if !Record() {
            return Err(TestError::new("testdata record mode is disabled"));
        }
        let index = self
            .positions
            .get(name)
            .copied()
            .ok_or_else(|| TestError::new(format!("test case {name:?} does not exist")))?;
        let output = if cascades {
            if !self.cascades_loaded {
                return Err(TestError::new("testdata cascades output is not loaded"));
            }
            self.cascades_dirty = true;
            &mut self.cascades_output
        } else {
            self.output_dirty = true;
            &mut self.output
        };
        output[index].cases = value;
        Ok(())
    }

    /// 若 dirty，将 out / xut JSON 写回磁盘。
    pub fn flush(&mut self) -> TestResult {
        if self.output_dirty {
            write_cases(
                self.directory.join(format!("{}_out.json", self.suite_name)),
                &self.output,
            )?;
            self.output_dirty = false;
        }
        if self.cascades_dirty {
            write_cases(
                self.directory.join(format!("{}_xut.json", self.suite_name)),
                &self.cascades_output,
            )?;
            self.cascades_dirty = false;
        }
        Ok(())
    }
}

impl Drop for TestData {
    fn drop(&mut self) {
        // 析构时尽量落盘，忽略 flush 错误以免 panic。
        let _ = self.flush();
    }
}

/// 从路径读取并反序列化用例列表。
fn read_cases(path: PathBuf) -> TestResult<Vec<TestCase>> {
    let bytes = fs::read(&path)
        .map_err(|error| TestError::new(format!("read {}: {error}", path.display())))?;
    // Keep parity with Go's loader: full-line `//` comments are accepted in
    // the fixture files, while inline comments remain ordinary JSON text.
    let text = String::from_utf8(bytes)
        .map_err(|error| TestError::new(format!("decode {}: {error}", path.display())))?;
    let uncommented = text
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    serde_json::from_str(&uncommented)
        .map_err(|error| TestError::new(format!("decode {}: {error}", path.display())))
}

/// 将用例列表以 pretty JSON 写入路径。
fn write_cases(path: PathBuf, cases: &[TestCase]) -> TestResult {
    let bytes =
        serde_json::to_vec_pretty(cases).map_err(|error| TestError::new(error.to_string()))?;
    fs::write(&path, bytes)
        .map_err(|error| TestError::new(format!("write {}: {error}", path.display())))
}

/// 查询是否处于录制模式。
pub fn Record() -> bool {
    RECORD.load(Ordering::Acquire)
}
/// 设置录制模式开关。
pub fn SetRecord(record: bool) {
    RECORD.store(record, Ordering::Release);
}

/// 便捷入口：按目录与套件名加载 [`TestData`]。
pub fn LoadTestSuiteData(directory: &str, suite_name: &str) -> TestResult<TestData> {
    TestData::load(directory, suite_name)
}

/// Cascades-aware variant of [`LoadTestSuiteData`].
pub fn LoadTestSuiteDataWithCascades(
    directory: &str,
    suite_name: &str,
    cascades: bool,
) -> TestResult<TestData> {
    TestData::load_with_cascades(directory, suite_name, cascades)
}

/// Convert row values to the space-separated strings used by Go fixtures.
pub fn ConvertRowsToStrings<T: std::fmt::Display>(rows: &[Vec<T>]) -> Vec<String> {
    rows.iter()
        .map(|row| {
            row.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}
