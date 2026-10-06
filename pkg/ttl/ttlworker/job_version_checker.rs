// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

/// Complete server build identity used by the rolling-upgrade safety gate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VersionInfo {
    pub version: String,
    pub git_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerInfo {
    pub version: VersionInfo,
    pub assumed: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum JobVersionCheckResult {
    #[default]
    FallbackToPrimaryKey,
    AllowIndexScan,
    BlockJob,
}

/// Compare all real TiDB servers with the local complete build identity.
pub fn version_infos_consistent(
    current: &VersionInfo,
    servers: &[(String, Option<ServerInfo>)],
) -> Result<bool, String> {
    if servers.is_empty() {
        return Err("TiDB server info list is empty".into());
    }
    let mut real_servers = 0;
    for (id, server) in servers {
        let server = server
            .as_ref()
            .ok_or_else(|| format!("TiDB server info is nil, server ID: {id}"))?;
        if server.assumed {
            continue;
        }
        real_servers += 1;
        if &server.version != current {
            return Ok(false);
        }
    }
    if real_servers == 0 {
        return Err("TiDB server info list contains no real servers".into());
    }
    Ok(true)
}

/// Non-thread-safe checker owned by the job-loop goroutine.
#[derive(Clone, Debug, Default)]
pub struct JobVersionChecker {
    last_check_seconds: Option<u64>,
    last_result: JobVersionCheckResult,
}

impl JobVersionChecker {
    pub fn check(
        &mut self,
        now_seconds: u64,
        local: Result<Option<ServerInfo>, String>,
        all: Result<Vec<(String, Option<ServerInfo>)>, String>,
    ) -> JobVersionCheckResult {
        let interval = if self.last_result == JobVersionCheckResult::BlockJob {
            60
        } else {
            10
        };
        if self
            .last_check_seconds
            .is_some_and(|last| now_seconds.saturating_sub(last) < interval)
        {
            return self.last_result;
        }
        let result = match (local, all) {
            (Ok(Some(local)), Ok(all)) => match version_infos_consistent(&local.version, &all) {
                Ok(true) => JobVersionCheckResult::AllowIndexScan,
                Ok(false) => JobVersionCheckResult::BlockJob,
                Err(_) => JobVersionCheckResult::FallbackToPrimaryKey,
            },
            _ => JobVersionCheckResult::FallbackToPrimaryKey,
        };
        self.last_check_seconds = Some(now_seconds);
        self.last_result = result;
        result
    }
}
