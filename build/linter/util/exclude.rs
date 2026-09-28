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

use crate::build;
use regex::Regex;

// shouldRun checks whether a `file` should be analyzed in the specific pass.
pub fn shouldRun(passName: &str, fileName: &str) -> bool {
    let Some(config) = build::NogoConfig.get(passName) else {
        return true;
    };
    shouldRunConfig(config, fileName)
}

pub(super) fn shouldRunConfig(config: &build::AnalysisConfig, fileName: &str) -> bool {
    if let Some(onlyFiles) = &config.OnlyFiles {
        for f in onlyFiles.keys() {
            let matched = regexMatch(f, fileName).unwrap_or_else(|_| {
                panic!("regex is wrong: {}", f);
            });
            if matched {
                return true;
            }
        }
        return false;
    }

    if let Some(excludeFiles) = &config.ExcludeFiles {
        for f in excludeFiles.keys() {
            let matched = regexMatch(f, fileName).unwrap_or_else(|_| {
                panic!("regex is wrong: {}", f);
            });
            if matched {
                return false;
            }
        }
        return true;
    }

    true
}

pub(super) fn regexMatch(pattern: &str, text: &str) -> Result<bool, regex::Error> {
    Regex::new(pattern).map(|compiled| compiled.is_match(text))
}
