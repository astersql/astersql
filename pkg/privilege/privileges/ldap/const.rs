// Copyright 2023-2023 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// LDAP SASL 认证方法名常量（与 Go `ldap` 包保持一致）。

#![allow(non_upper_case_globals)]

/// SCRAM-SHA-1 SASL 机制名。
pub const SASLAuthMethodSCRAMSHA1: &str = "SCRAM-SHA-1";
/// SCRAM-SHA-256 SASL 机制名。
pub const SASLAuthMethodSCRAMSHA256: &str = "SCRAM-SHA-256";
/// GSSAPI（Kerberos）SASL 机制名。
pub const SASLAuthMethodGSSAPI: &str = "GSSAPI";
