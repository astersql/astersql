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

// LDAP 包迁移对照单元测试：常量、DN 规范化、配置池世代与 SASL 循环语义。

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs;
    use std::sync::Arc;

    use anyhow::{Result, anyhow};
    use tempfile::tempdir;

    use astersql_privilege_ldap::constants::{
        SASLAuthMethodGSSAPI, SASLAuthMethodSCRAMSHA1, SASLAuthMethodSCRAMSHA256,
    };
    use astersql_privilege_ldap::ldap_common::LdapAuthImpl;
    use astersql_privilege_ldap::sasl::{AuthConn, LdapSaslAuthImpl, SaslSession};
    use astersql_privilege_ldap::simple::LdapSimpleAuthImpl;

    /// 断言 SASL 方法常量字符串与 Go 一致。
    #[test]
    fn sasl_method_constants_match_go() {
        assert_eq!(SASLAuthMethodSCRAMSHA1, "SCRAM-SHA-1");
        assert_eq!(SASLAuthMethodSCRAMSHA256, "SCRAM-SHA-256");
        assert_eq!(SASLAuthMethodGSSAPI, "GSSAPI");
    }

    /// 覆盖完整 DN 与 `+suffix` 两种规范化路径。
    #[test]
    fn canonicalize_dn_matches_go_cases() {
        let ldap = LdapAuthImpl::default();
        ldap.SetSearchAttr("cn");
        assert_eq!(
            ldap.canonicalize_dn("yka", "cn=y,dc=ping,dc=cap"),
            "cn=y,dc=ping,dc=cap"
        );
        assert_eq!(
            ldap.canonicalize_dn("yka", "+dc=ping,dc=cap"),
            "cn=yka,dc=ping,dc=cap"
        );
    }

    /// 配置读写与连接池重建世代规则（仅有效容量/主机变更时重建）。
    #[test]
    fn setters_getters_and_pool_rebuild_rules_match_go() {
        let ldap = LdapAuthImpl::default();
        ldap.SetBindBaseDN("dc=example,dc=com");
        ldap.SetBindRootDN("cn=admin,dc=example,dc=com");
        ldap.SetBindRootPW("secret");
        ldap.SetSearchAttr("uid");
        ldap.SetLDAPServerHost("ldap.example.com");
        ldap.SetLDAPServerPort(389);
        ldap.SetEnableTLS(true);

        assert_eq!(ldap.GetBindBaseDN(), "dc=example,dc=com");
        assert_eq!(ldap.GetBindRootDN(), "cn=admin,dc=example,dc=com");
        assert_eq!(ldap.GetBindRootPW(), "secret");
        assert_eq!(ldap.GetSearchAttr(), "uid");
        assert_eq!(ldap.GetLDAPServerHost(), "ldap.example.com");
        assert_eq!(ldap.GetLDAPServerPort(), 389);
        assert!(ldap.GetEnableTLS());
        assert_eq!(ldap.pool_generation(), 0);

        ldap.SetInitCapacity(1);
        assert_eq!(ldap.pool_generation(), 0, "max capacity is not valid yet");
        ldap.SetMaxCapacity(2);
        assert_eq!(ldap.pool_generation(), 1);
        ldap.SetLDAPServerPort(636);
        assert_eq!(ldap.pool_generation(), 2);
        ldap.SetLDAPServerPort(636);
        assert_eq!(
            ldap.pool_generation(),
            2,
            "unchanged configuration must not rebuild"
        );
    }

    /// CA 路径：缺失文件、非法 PEM、清空路径三种结果。
    #[test]
    fn ca_path_reports_io_and_pem_errors_and_can_be_cleared() {
        let ldap = LdapAuthImpl::default();
        let missing = ldap.SetCAPath("/definitely/missing/ca.pem").unwrap_err();
        assert!(missing.to_string().contains("read ca certificate"));

        let dir = tempdir().unwrap();
        let invalid = dir.path().join("invalid.pem");
        fs::write(&invalid, b"not a certificate").unwrap();
        let parse = ldap.SetCAPath(invalid.to_str().unwrap()).unwrap_err();
        assert!(parse.to_string().contains("fail to parse ca certificate"));

        ldap.SetCAPath("").unwrap();
        assert_eq!(ldap.GetCAPath(), "");
    }

    /// Simple Bind 密码必须以 NUL 结尾，与 Go 行为对齐。
    #[test]
    fn simple_password_validation_matches_go_nul_termination() {
        assert_eq!(
            LdapSimpleAuthImpl::password_string(&[])
                .unwrap_err()
                .to_string(),
            "invalid password"
        );
        assert_eq!(
            LdapSimpleAuthImpl::password_string(b"secret")
                .unwrap_err()
                .to_string(),
            "invalid password"
        );
        assert_eq!(
            LdapSimpleAuthImpl::password_string(b"secret\0").unwrap(),
            "secret"
        );
        assert_eq!(LdapSimpleAuthImpl::password_string(b"\0").unwrap(), "");
    }

    /// 记录写出的 AuthMoreData 与 flush 次数的模拟连接。
    #[derive(Default)]
    struct MockAuthConn {
        writes: Vec<Vec<u8>>,
        flushes: usize,
        reads: VecDeque<Vec<u8>>,
    }

    impl AuthConn for MockAuthConn {
        fn write_auth_more_data(&mut self, data: &[u8]) -> Result<()> {
            self.writes.push(data.to_vec());
            Ok(())
        }
        fn flush(&mut self) -> Result<()> {
            self.flushes += 1;
            Ok(())
        }
        fn read_packet(&mut self) -> Result<Vec<u8>> {
            self.reads
                .pop_front()
                .ok_or_else(|| anyhow!("no client packet"))
        }
    }

    /// 按预设步骤返回 LDAP result_code / server_cred 的模拟会话。
    struct MockSaslSession {
        steps: VecDeque<(u32, Vec<u8>)>,
        seen: Vec<Vec<u8>>,
    }

    impl SaslSession for MockSaslSession {
        fn server_bind_step(
            &mut self,
            client_cred: &[u8],
            dn: &str,
            method: &str,
        ) -> Result<(u32, Vec<u8>)> {
            assert_eq!(dn, "uid=yka,dc=example,dc=com");
            assert_eq!(method, SASLAuthMethodSCRAMSHA256);
            self.seen.push(client_cred.to_vec());
            self.steps
                .pop_front()
                .ok_or_else(|| anyhow!("no LDAP step"))
        }
    }

    /// 成功前仍发送最后一轮 server credential（对齐 Go 故意行为）。
    #[test]
    fn sasl_loop_sends_final_credential_before_success() {
        let ldap = Arc::new(LdapAuthImpl::default());
        ldap.SetSearchAttr("uid");
        let sasl = LdapSaslAuthImpl::new(ldap, SASLAuthMethodSCRAMSHA256);
        let mut session = MockSaslSession {
            steps: VecDeque::from([(14, b"challenge".to_vec()), (0, b"final".to_vec())]),
            seen: Vec::new(),
        };
        let mut client = MockAuthConn {
            reads: VecDeque::from([b"response".to_vec()]),
            ..Default::default()
        };

        sasl.AuthLDAPSASL(
            "yka",
            "+dc=example,dc=com",
            b"initial".to_vec(),
            &mut session,
            &mut client,
        )
        .unwrap();

        assert_eq!(session.seen, [b"initial".to_vec(), b"response".to_vec()]);
        assert_eq!(client.writes, [b"challenge".to_vec(), b"final".to_vec()]);
        assert_eq!(client.flushes, 2);
    }
}
