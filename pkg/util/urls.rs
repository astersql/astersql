// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

use anyhow::Result;

use crate::service_url::parse_service_url;

/// Parse a comma-separated service endpoint list, retaining explicit schemes.
pub fn ParseHostPortAddr(input: &str) -> Result<Vec<String>> {
    input
        .split(',')
        .map(|entry| {
            let entry = entry.trim();
            parse_service_url(entry, "http").map(|url| url.Endpoint(entry.contains("://")))
        })
        .collect()
}
