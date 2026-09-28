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

// 对应 Go `session_token_test.go`：令牌证书、签名算法、过期与轮换场景原文及 Rust JSON/错误断言。
//
// Go 测试全文保存在 `GO_REFERENCE` 中（已含中文说明）；文末 Rust 测试校验
// SessionToken JSON 字段名、签名 base64，以及缺证书时的 ErrCannotMigrateSession。

const GO_REFERENCE: &str = r################"

#![allow(dead_code, non_snake_case, non_upper_case_globals, unused_variables)]

// session token 证书配置、签名算法、过期验证、证书轮换和并发读写场景。
// 主要类型、函数、测试场景和辅助函数按 Go 文件顺序保留；关键分支、资源收尾、错误路径、并发和外部依赖在附近用中文说明。
// Go imports（保留依赖边界，待 Rust crate 接线）：
// - "crypto/x509"
// - "encoding/json"
// - "fmt"
// - "path/filepath"
// - "testing"
// - "time"
// - "github.com/pingcap/failpoint"
// - "github.com/pingcap/tidb/pkg/config/deploymode"
// - "github.com/pingcap/tidb/pkg/config/kerneltype"
// - "github.com/pingcap/tidb/pkg/util"
// - "github.com/stretchr/testify/require"

// 变量声明沿用 Go 测试状态；涉及 failpoint 名称、证书路径或临时结果时保持原表达式。
var (
	mockNowOffset = "github.com/pingcap/tidb/pkg/sessionctx/sessionstates/mockNowOffset"
)

// TestSetCertAndKey 对应 Go 测试：覆盖证书/私钥路径缺失、成对配置以及不匹配时沿用旧证书对。
// 该测试函数在 Go 中依赖 testing.T/require/testkit；这里保留断言点，不声明已经可运行。
func TestSetCertAndKey(t *testing.T) {
	// Go 的 t.TempDir 管理临时证书目录，测试结束自动清理。
	tempDir := t.TempDir()
	certPath := filepath.Join(tempDir, "test1_cert.pem")
	keyPath := filepath.Join(tempDir, "test1_key.pem")
	createRSACert(t, certPath, keyPath)

	// no cert and no key
	// CreateSessionToken 会读取当前签名证书；缺证书路径时应返回错误。
	_, err := CreateSessionToken("test_user")
	require.ErrorContains(t, err, "no certificate or key file")
	// no cert
	// 证书/私钥路径是包级状态；测试按 Go 顺序强制刷新签名证书。
	SetKeyPath(keyPath)
	// CreateSessionToken 会读取当前签名证书；缺证书路径时应返回错误。
	_, err = CreateSessionToken("test_user")
	require.ErrorContains(t, err, "no certificate or key file")
	// no key
	// 证书/私钥路径是包级状态；测试按 Go 顺序强制刷新签名证书。
	SetKeyPath("")
	SetCertPath(certPath)
	// CreateSessionToken 会读取当前签名证书；缺证书路径时应返回错误。
	_, err = CreateSessionToken("test_user")
	require.ErrorContains(t, err, "no certificate or key file")
	// both configured
	// 证书/私钥路径是包级状态；测试按 Go 顺序强制刷新签名证书。
	SetKeyPath(keyPath)
	// CreateSessionToken 会读取当前签名证书；缺证书路径时应返回错误。
	_, err = CreateSessionToken("test_user")
	require.NoError(t, err)
	// When the key and cert don't match, it will still use the old pair.
	certPath2 := filepath.Join(tempDir, "test2_cert.pem")
	keyPath2 := filepath.Join(tempDir, "test2_key.pem")
	// 证书文件写入临时目录，属于 IO 边界；不实际执行。
	err = util.CreateCertificates(certPath2, keyPath2, 4096, x509.RSA, x509.UnknownSignatureAlgorithm)
	require.NoError(t, err)
	// 证书/私钥路径是包级状态；测试按 Go 顺序强制刷新签名证书。
	SetKeyPath(keyPath2)
	// CreateSessionToken 会读取当前签名证书；缺证书路径时应返回错误。
	_, err = CreateSessionToken("test_user")
	require.NoError(t, err)
}

// TestSignAlgo 对应 Go 表驱动测试：枚举 RSA/ECDSA/Ed25519 签名算法和 key size 后创建 token 并验证。
// 该测试函数在 Go 中依赖 testing.T/require/testkit；这里保留断言点，不声明已经可运行。
func TestSignAlgo(t *testing.T) {
	tests := []struct {
		pubKeyAlgo x509.PublicKeyAlgorithm
		signAlgos  []x509.SignatureAlgorithm
		keySizes   []int
	}{
		{
			pubKeyAlgo: x509.RSA,
			signAlgos: []x509.SignatureAlgorithm{
				x509.SHA256WithRSA,
				x509.SHA384WithRSA,
				x509.SHA512WithRSA,
				x509.SHA256WithRSAPSS,
				x509.SHA384WithRSAPSS,
				x509.SHA512WithRSAPSS,
			},
			keySizes: []int{
				2048,
				4096,
			},
		},
		{
			pubKeyAlgo: x509.ECDSA,
			signAlgos: []x509.SignatureAlgorithm{
				x509.ECDSAWithSHA256,
				x509.ECDSAWithSHA384,
				x509.ECDSAWithSHA512,
			},
			keySizes: []int{
				4096,
			},
		},
		{
			pubKeyAlgo: x509.Ed25519,
			signAlgos: []x509.SignatureAlgorithm{
				x509.PureEd25519,
			},
			keySizes: []int{
				4096,
			},
		},
	}

	// Go 的 t.TempDir 管理临时证书目录，测试结束自动清理。
	tempDir := t.TempDir()
	certPath := filepath.Join(tempDir, "test1_cert.pem")
	keyPath := filepath.Join(tempDir, "test1_key.pem")
	// 证书/私钥路径是包级状态；测试按 Go 顺序强制刷新签名证书。
	SetKeyPath(keyPath)
	SetCertPath(certPath)
	for _, test := range tests {
		for _, signAlgo := range test.signAlgos {
			for _, keySize := range test.keySizes {
				msg := fmt.Sprintf("pubKeyAlgo: %s, signAlgo: %s, keySize: %d", test.pubKeyAlgo.String(),
					signAlgo.String(), keySize)
				// 证书文件写入临时目录，属于 IO 边界；不实际执行。
				err := util.CreateCertificates(certPath, keyPath, keySize, test.pubKeyAlgo, signAlgo)
				require.NoError(t, err, msg)
				// ReloadSigningCert 对应证书重新加载边界，涉及旧证书宽限期。
				ReloadSigningCert()
				_, tokenBytes := createNewToken(t, "test_user")
				// ValidateSessionToken 是签名与过期校验入口，错误断言保留 Go 原消息。
				err = ValidateSessionToken(tokenBytes, "test_user")
				require.NoError(t, err, msg)
			}
		}
	}
}

// TestVerifyToken 对应 Go 测试：覆盖正常校验、token 过期、用户名不匹配以及伪造字段后的签名失败。
// 该测试函数在 Go 中依赖 testing.T/require/testkit；这里保留断言点，不声明已经可运行。
func TestVerifyToken(t *testing.T) {
	SetupSigningCertForTest(t)

	// check succeeds
	token, tokenBytes := createNewToken(t, "test_user")
	// ValidateSessionToken 是签名与过期校验入口，错误断言保留 Go 原消息。
	err := ValidateSessionToken(tokenBytes, "test_user")
	require.NoError(t, err)
	// the token expires
	timeOffset := uint64(tokenLifetime + time.Minute)
	// failpoint 控制测试时间偏移或错误注入，保留启用点和参数。
	require.NoError(t, failpoint.Enable(mockNowOffset, fmt.Sprintf(`return(%d)`, timeOffset)))
	// ValidateSessionToken 是签名与过期校验入口，错误断言保留 Go 原消息。
	err = ValidateSessionToken(tokenBytes, "test_user")
	// failpoint 在断言后关闭，避免污染后续用例。
	require.NoError(t, failpoint.Disable(mockNowOffset))
	require.ErrorContains(t, err, "token expired")
	// the current user is different with the token
	// ValidateSessionToken 是签名与过期校验入口，错误断言保留 Go 原消息。
	err = ValidateSessionToken(tokenBytes, "another_user")
	require.ErrorContains(t, err, "username does not match")
	// forge the user name
	token.Username = "another_user"
	// JSON marshal 保留 token 或 session state 的签名前/恢复前字节形状。
	tokenBytes2, err := json.Marshal(token)
	require.NoError(t, err)
	// ValidateSessionToken 是签名与过期校验入口，错误断言保留 Go 原消息。
	err = ValidateSessionToken(tokenBytes2, "another_user")
	require.ErrorContains(t, err, "verification error")
	// forge the expire time
	token.Username = "test_user"
	token.ExpireTime = time.Now().Add(-time.Minute)
	// JSON marshal 保留 token 或 session state 的签名前/恢复前字节形状。
	tokenBytes2, err = json.Marshal(token)
	require.NoError(t, err)
	// ValidateSessionToken 是签名与过期校验入口，错误断言保留 Go 原消息。
	err = ValidateSessionToken(tokenBytes2, "test_user")
	require.ErrorContains(t, err, "verification error")
}

// TestStarterSessionTokenLifetime 对应 Go 测试：NextGen starter 模式下使用更长 token/cert 有效期。
// 该测试函数在 Go 中依赖 testing.T/require/testkit；这里保留断言点，不声明已经可运行。
func TestStarterSessionTokenLifetime(t *testing.T) {
	if !kerneltype.IsNextGen() {
		t.Skip("starter deploy mode is only available for nextgen TiDB")
	}

	original := deploymode.Get()
	require.NoError(t, deploymode.Set(deploymode.Starter))
	// Go 的 t.Cleanup 用于恢复全局配置或部署模式；Rust 接线时需要等价资源收尾。
	t.Cleanup(func() {
		require.NoError(t, deploymode.Set(original))
	})

	require.Equal(t, starterTokenLifetime, currentTokenLifetime())
	require.Equal(t, starterLoadCertInterval, GetLoadCertInterval())
	require.Equal(t, starterOldCertValidTime, currentOldCertValidTime())

	SetupSigningCertForTest(t)
	_, tokenBytes := createNewToken(t, "test_user")

	timeOffset := uint64(tokenLifetime + time.Minute)
	// failpoint 控制测试时间偏移或错误注入，保留启用点和参数。
	require.NoError(t, failpoint.Enable(mockNowOffset, fmt.Sprintf(`return(%d)`, timeOffset)))
	// ValidateSessionToken 是签名与过期校验入口，错误断言保留 Go 原消息。
	err := ValidateSessionToken(tokenBytes, "test_user")
	// failpoint 在断言后关闭，避免污染后续用例。
	require.NoError(t, failpoint.Disable(mockNowOffset))
	require.NoError(t, err)

	timeOffset = uint64(starterTokenLifetime + time.Minute)
	// failpoint 控制测试时间偏移或错误注入，保留启用点和参数。
	require.NoError(t, failpoint.Enable(mockNowOffset, fmt.Sprintf(`return(%d)`, timeOffset)))
	// ValidateSessionToken 是签名与过期校验入口，错误断言保留 Go 原消息。
	err = ValidateSessionToken(tokenBytes, "test_user")
	// failpoint 在断言后关闭，避免污染后续用例。
	require.NoError(t, failpoint.Disable(mockNowOffset))
	require.ErrorContains(t, err, "token expired")
}

// TestCertExpire 对应 Go 测试：证书替换后旧证书在宽限期内可验，过期后拒绝旧 token。
// 该测试函数在 Go 中依赖 testing.T/require/testkit；这里保留断言点，不声明已经可运行。
func TestCertExpire(t *testing.T) {
	// Go 的 t.TempDir 管理临时证书目录，测试结束自动清理。
	tempDir := t.TempDir()
	certPath := filepath.Join(tempDir, "test1_cert.pem")
	keyPath := filepath.Join(tempDir, "test1_key.pem")
	createRSACert(t, certPath, keyPath)
	// 证书/私钥路径是包级状态；测试按 Go 顺序强制刷新签名证书。
	SetKeyPath(keyPath)
	SetCertPath(certPath)

	_, tokenBytes := createNewToken(t, "test_user")
	// ValidateSessionToken 是签名与过期校验入口，错误断言保留 Go 原消息。
	err := ValidateSessionToken(tokenBytes, "test_user")
	require.NoError(t, err)
	// replace the cert, but the old cert is still valid for a while
	certPath2 := filepath.Join(tempDir, "test2_cert.pem")
	keyPath2 := filepath.Join(tempDir, "test2_key.pem")
	createRSACert(t, certPath2, keyPath2)
	// 证书/私钥路径是包级状态；测试按 Go 顺序强制刷新签名证书。
	SetKeyPath(keyPath2)
	SetCertPath(certPath2)
	// ValidateSessionToken 是签名与过期校验入口，错误断言保留 Go 原消息。
	err = ValidateSessionToken(tokenBytes, "test_user")
	require.NoError(t, err)
	// the old cert expires and the original token is invalid
	timeOffset := uint64(LoadCertInterval)
	// failpoint 控制测试时间偏移或错误注入，保留启用点和参数。
	require.NoError(t, failpoint.Enable(mockNowOffset, fmt.Sprintf(`return(%d)`, timeOffset)))
	// ReloadSigningCert 对应证书重新加载边界，涉及旧证书宽限期。
	ReloadSigningCert()
	timeOffset += uint64(oldCertValidTime + time.Minute)
	// failpoint 控制测试时间偏移或错误注入，保留启用点和参数。
	require.NoError(t, failpoint.Enable(mockNowOffset, fmt.Sprintf(`return(%d)`, timeOffset)))
	// ValidateSessionToken 是签名与过期校验入口，错误断言保留 Go 原消息。
	err = ValidateSessionToken(tokenBytes, "test_user")
	require.ErrorContains(t, err, "verification error")
	// the new cert is not rotated but is reloaded
	_, tokenBytes = createNewToken(t, "test_user")
	// ReloadSigningCert 对应证书重新加载边界，涉及旧证书宽限期。
	ReloadSigningCert()
	// ValidateSessionToken 是签名与过期校验入口，错误断言保留 Go 原消息。
	err = ValidateSessionToken(tokenBytes, "test_user")
	require.NoError(t, err)
	// the cert is rotated but is still valid
	createRSACert(t, certPath2, keyPath2)
	timeOffset += uint64(LoadCertInterval)
	// failpoint 控制测试时间偏移或错误注入，保留启用点和参数。
	require.NoError(t, failpoint.Enable(mockNowOffset, fmt.Sprintf(`return(%d)`, timeOffset)))
	// ReloadSigningCert 对应证书重新加载边界，涉及旧证书宽限期。
	ReloadSigningCert()
	// ValidateSessionToken 是签名与过期校验入口，错误断言保留 Go 原消息。
	err = ValidateSessionToken(tokenBytes, "test_user")
	require.ErrorContains(t, err, "token expired")
	// after some time, it's not valid
	timeOffset += uint64(oldCertValidTime + time.Minute)
	// failpoint 控制测试时间偏移或错误注入，保留启用点和参数。
	require.NoError(t, failpoint.Enable(mockNowOffset, fmt.Sprintf(`return(%d)`, timeOffset)))
	// ValidateSessionToken 是签名与过期校验入口，错误断言保留 Go 原消息。
	err = ValidateSessionToken(tokenBytes, "test_user")
	// failpoint 在断言后关闭，避免污染后续用例。
	require.NoError(t, failpoint.Disable(mockNowOffset))
	require.ErrorContains(t, err, "verification error")
}

// TestLoadAndReadConcurrently 对应 Go 并发测试：写证书、ReloadSigningCert 与生成/验证 token 并发运行。
// 该测试函数在 Go 中依赖 testing.T/require/testkit；这里保留断言点，不声明已经可运行。
func TestLoadAndReadConcurrently(t *testing.T) {
	// Go 的 t.TempDir 管理临时证书目录，测试结束自动清理。
	tempDir := t.TempDir()
	certPath := filepath.Join(tempDir, "test1_cert.pem")
	keyPath := filepath.Join(tempDir, "test1_key.pem")
	createRSACert(t, certPath, keyPath)
	// 证书/私钥路径是包级状态；测试按 Go 顺序强制刷新签名证书。
	SetKeyPath(keyPath)
	SetCertPath(certPath)

	deadline := time.Now().Add(5 * time.Second)

// 变量声明沿用 Go 测试状态；涉及 failpoint 名称、证书路径或临时结果时保持原表达式。
	var wg util.WaitGroupWrapper
	// the writer
	// 这里对应 Go 并发 goroutine；Rust 接线时需要线程/任务和 WaitGroup 等价语义。
	wg.Run(func() {
		for time.Now().Before(deadline) {
			createRSACert(t, certPath, keyPath)
			time.Sleep(time.Second)
		}
	})
	// the loader
	for range 2 {
		// 这里对应 Go 并发 goroutine；Rust 接线时需要线程/任务和 WaitGroup 等价语义。
		wg.Run(func() {
			for time.Now().Before(deadline) {
				// ReloadSigningCert 对应证书重新加载边界，涉及旧证书宽限期。
				ReloadSigningCert()
				time.Sleep(500 * time.Millisecond)
			}
		})
	}
	// the reader
	for i := range 3 {
		id := i
		// 这里对应 Go 并发 goroutine；Rust 接线时需要线程/任务和 WaitGroup 等价语义。
		wg.Run(func() {
			username := fmt.Sprintf("test_user_%d", id)
			for time.Now().Before(deadline) {
				_, tokenBytes := createNewToken(t, username)
				time.Sleep(10 * time.Millisecond)
				// ValidateSessionToken 是签名与过期校验入口，错误断言保留 Go 原消息。
				err := ValidateSessionToken(tokenBytes, username)
				require.NoError(t, err)
				time.Sleep(10 * time.Millisecond)
			}
		})
	}
	// 等待所有并发读写任务结束，对应 Go WaitGroupWrapper.Wait。
	wg.Wait()
}

// createNewToken 对应 Go 辅助函数：创建 SessionToken 并 JSON marshal 成可验证字节。
func createNewToken(t *testing.T, username string) (*SessionToken, []byte) {
	// CreateSessionToken 会读取当前签名证书；缺证书路径时应返回错误。
	token, err := CreateSessionToken(username)
	require.NoError(t, err)
	// JSON marshal 保留 token 或 session state 的签名前/恢复前字节形状。
	tokenBytes, err := json.Marshal(token)
	require.NoError(t, err)
	return token, tokenBytes
}

// createRSACert 对应 Go 辅助函数：用 util.CreateCertificates 创建 RSA 测试证书。
func createRSACert(t *testing.T, certPath, keyPath string) {
	// 证书文件写入临时目录，属于 IO 边界；不实际执行。
	err := util.CreateCertificates(certPath, keyPath, 4096, x509.RSA, x509.UnknownSignatureAlgorithm)
	require.NoError(t, err)
}

// SetupSigningCertForTest sets signing cert.

// SetupSigningCertForTest 对应 Go 测试辅助：在临时目录生成证书并写入全局 cert/key 路径。
func SetupSigningCertForTest(t *testing.T) {
	// Go 的 t.TempDir 管理临时证书目录，测试结束自动清理。
	tempDir := t.TempDir()
	certPath := filepath.Join(tempDir, "test1_cert.pem")
	keyPath := filepath.Join(tempDir, "test1_key.pem")
	createRSACert(t, certPath, keyPath)
	// 证书/私钥路径是包级状态；测试按 Go 顺序强制刷新签名证书。
	SetKeyPath(keyPath)
	SetCertPath(certPath)
}
"################;

use super::*;
use chrono::{Duration, TimeZone, Utc};
use openssl::asn1::Asn1Time;
use openssl::bn::BigNum;
use openssl::ec::{EcGroup, EcKey};
use openssl::hash::MessageDigest;
use openssl::nid::Nid;
use openssl::pkey::{Id, PKey, Private};
use openssl::pkey_ctx::PkeyCtx;
use openssl::rsa::Rsa;
use openssl::x509::{X509, X509NameBuilder};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::{Duration as StdDuration, Instant};

/// 校验 SessionToken JSON 字段名与 signature 的 base64 往返与 Go 一致。
#[test]
fn session_token_json_matches_go_field_names_and_base64_signature() {
    let sign_time = Utc.with_ymd_and_hms(2026, 7, 23, 8, 9, 10).unwrap();
    let token = SessionToken {
        Username: "Test_User".to_owned(),
        SignTime: sign_time,
        ExpireTime: sign_time + Duration::minutes(1),
        Signature: vec![0, 1, 2, 0xff],
    };

    let value = serde_json::to_value(&token).unwrap();
    assert_eq!(value["username"], "Test_User");
    assert!(value.get("sign-time").is_some());
    assert!(value.get("expire-time").is_some());
    assert_eq!(value["signature"], "AAEC/w==");

    let decoded: SessionToken = serde_json::from_value(value).unwrap();
    assert_eq!(decoded.Username, token.Username);
    assert_eq!(decoded.SignTime, token.SignTime);
    assert_eq!(decoded.ExpireTime, token.ExpireTime);
    assert_eq!(decoded.Signature, token.Signature);
}

/// 无签名证书时 CreateSessionToken 应返回 ErrCannotMigrateSession。
#[test]
fn signing_without_certificate_returns_cannot_migrate() {
    let _guard = SESSION_TOKEN_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ResetSigningCertForTest();
    let error = CreateSessionToken("test_user").unwrap_err();
    assert_eq!(error.code(), errno::errcode::ErrCannotMigrateSession);
    assert!(error.to_string().contains("no certificate or key file"));
}

fn write_x509_pair(cert_path: &Path, key_path: &Path, key: &PKey<Private>, digest: MessageDigest) {
    let mut name = X509NameBuilder::new().unwrap();
    name.append_entry_by_text("CN", "session-token-test")
        .unwrap();
    let name = name.build();
    let serial = BigNum::from_u32(1).unwrap().to_asn1_integer().unwrap();
    let not_before = Asn1Time::days_from_now(0).unwrap();
    let not_after = Asn1Time::days_from_now(1).unwrap();
    let mut builder = X509::builder().unwrap();
    builder.set_version(2).unwrap();
    builder.set_serial_number(&serial).unwrap();
    builder.set_subject_name(&name).unwrap();
    builder.set_issuer_name(&name).unwrap();
    builder.set_pubkey(key).unwrap();
    builder.set_not_before(&not_before).unwrap();
    builder.set_not_after(&not_after).unwrap();
    builder.sign(key, digest).unwrap();
    fs::write(cert_path, builder.build().to_pem().unwrap()).unwrap();
    fs::write(key_path, key.private_key_to_pem_pkcs8().unwrap()).unwrap();
}

fn write_rsa_x509_pair(cert_path: &Path, key_path: &Path, bits: u32, digest: MessageDigest) {
    let key = PKey::from_rsa(Rsa::generate(bits).unwrap()).unwrap();
    write_x509_pair(cert_path, key_path, &key, digest);
}

fn write_rsa_pss_x509_pair(cert_path: &Path, key_path: &Path, bits: u32, digest: MessageDigest) {
    let mut context = PkeyCtx::new_id(Id::RSA_PSS).unwrap();
    context.keygen_init().unwrap();
    context.set_rsa_keygen_bits(bits).unwrap();
    let key = context.keygen().unwrap();
    write_x509_pair(cert_path, key_path, &key, digest);
}

fn write_ecdsa_x509_pair(cert_path: &Path, key_path: &Path, curve: Nid, digest: MessageDigest) {
    let group = EcGroup::from_curve_name(curve).unwrap();
    let key = PKey::from_ec_key(EcKey::generate(&group).unwrap()).unwrap();
    write_x509_pair(cert_path, key_path, &key, digest);
}

fn write_ed25519_x509_pair(cert_path: &Path, key_path: &Path) {
    let key = PKey::generate_ed25519().unwrap();
    write_x509_pair(cert_path, key_path, &key, MessageDigest::null());
}

fn assert_token_round_trip(username: &str) {
    ReloadSigningCert();
    let token = CreateSessionToken(username).unwrap();
    ValidateSessionToken(&serde_json::to_vec(&token).unwrap(), username).unwrap();
}

/// 对应 Go TestSetCertAndKey：缺路径失败、完整路径成功、不匹配密钥保留旧证书。
#[test]
fn set_cert_and_key_matches_go() {
    let _guard = SESSION_TOKEN_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ResetSigningCertForTest();
    let directory = tempfile::tempdir().unwrap();
    let cert_path = directory.path().join("cert.pem");
    let key_path = directory.path().join("key.pem");
    write_rsa_x509_pair(&cert_path, &key_path, 2048, MessageDigest::sha256());

    assert!(CreateSessionToken("test_user").is_err());
    SetKeyPath(key_path.to_string_lossy().into_owned());
    assert!(CreateSessionToken("test_user").is_err());
    SetKeyPath(String::new());
    SetCertPath(cert_path.to_string_lossy().into_owned());
    assert!(CreateSessionToken("test_user").is_err());
    SetKeyPath(key_path.to_string_lossy().into_owned());
    CreateSessionToken("test_user").unwrap();

    let other_key_path = directory.path().join("other-key.pem");
    let other_cert_path = directory.path().join("other-cert.pem");
    write_rsa_x509_pair(
        &other_cert_path,
        &other_key_path,
        4096,
        MessageDigest::sha256(),
    );
    SetKeyPath(other_key_path.to_string_lossy().into_owned());
    CreateSessionToken("test_user").unwrap();
    ResetSigningCertForTest();
}

/// 对应 Go TestSignAlgo：覆盖 RSA PKCS#1/PSS、ECDSA、Ed25519 与密钥尺寸矩阵。
#[test]
fn signing_algorithm_matrix_matches_go() {
    let _guard = SESSION_TOKEN_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ResetSigningCertForTest();
    let directory = tempfile::tempdir().unwrap();
    let cert_path = directory.path().join("cert.pem");
    let key_path = directory.path().join("key.pem");
    SetKeyPath(key_path.to_string_lossy().into_owned());
    SetCertPath(cert_path.to_string_lossy().into_owned());

    for bits in [2048, 4096] {
        for (name, digest) in [
            ("sha256", MessageDigest::sha256()),
            ("sha384", MessageDigest::sha384()),
            ("sha512", MessageDigest::sha512()),
        ] {
            write_rsa_x509_pair(&cert_path, &key_path, bits, digest);
            assert_token_round_trip(&format!("rsa-{name}-{bits}"));
            write_rsa_pss_x509_pair(&cert_path, &key_path, bits, digest);
            assert_token_round_trip(&format!("rsa-pss-{name}-{bits}"));
        }
    }
    for (name, curve, digest) in [
        ("sha256", Nid::X9_62_PRIME256V1, MessageDigest::sha256()),
        ("sha384", Nid::SECP384R1, MessageDigest::sha384()),
        ("sha512", Nid::SECP521R1, MessageDigest::sha512()),
    ] {
        write_ecdsa_x509_pair(&cert_path, &key_path, curve, digest);
        assert_token_round_trip(&format!("ecdsa-{name}"));
    }
    write_ed25519_x509_pair(&cert_path, &key_path);
    assert_token_round_trip("ed25519");
    ResetSigningCertForTest();
}

/// 对应 Go TestVerifyToken：正常、过期、用户名不匹配及字段伪造。
#[test]
fn token_verification_failures_match_go() {
    let _guard = SESSION_TOKEN_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ResetSigningCertForTest();
    let directory = tempfile::tempdir().unwrap();
    let cert_path = directory.path().join("cert.pem");
    let key_path = directory.path().join("key.pem");
    write_rsa_x509_pair(&cert_path, &key_path, 4096, MessageDigest::sha256());
    SetKeyPath(key_path.to_string_lossy().into_owned());
    SetCertPath(cert_path.to_string_lossy().into_owned());

    let token = CreateSessionToken("test_user").unwrap();
    let token_bytes = serde_json::to_vec(&token).unwrap();
    ValidateSessionToken(&token_bytes, "test_user").unwrap();
    SetMockNowOffset(tokenLifetime + Duration::minutes(1));
    assert!(
        ValidateSessionToken(&token_bytes, "test_user")
            .unwrap_err()
            .to_string()
            .contains("token expired")
    );
    SetMockNowOffset(Duration::zero());
    assert!(
        ValidateSessionToken(&token_bytes, "another_user")
            .unwrap_err()
            .to_string()
            .contains("username does not match")
    );

    let mut forged = token.clone();
    forged.Username = "another_user".to_owned();
    assert!(
        ValidateSessionToken(&serde_json::to_vec(&forged).unwrap(), "another_user")
            .unwrap_err()
            .to_string()
            .contains("verification")
    );
    forged.Username = "test_user".to_owned();
    forged.ExpireTime = Utc::now() - Duration::minutes(1);
    assert!(
        ValidateSessionToken(&serde_json::to_vec(&forged).unwrap(), "test_user")
            .unwrap_err()
            .to_string()
            .contains("verification")
    );
    ResetSigningCertForTest();
}

/// Go `strings.EqualFold` uses Unicode simple case folding, including the
/// ordinary and final lowercase forms of Greek sigma.
#[test]
fn token_username_uses_go_unicode_case_folding() {
    let _guard = SESSION_TOKEN_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ResetSigningCertForTest();
    let directory = tempfile::tempdir().unwrap();
    let cert_path = directory.path().join("cert.pem");
    let key_path = directory.path().join("key.pem");
    write_rsa_x509_pair(&cert_path, &key_path, 2048, MessageDigest::sha256());
    SetKeyPath(key_path.to_string_lossy().into_owned());
    SetCertPath(cert_path.to_string_lossy().into_owned());

    let token = CreateSessionToken("Σ").unwrap();
    let token_bytes = serde_json::to_vec(&token).unwrap();
    ValidateSessionToken(&token_bytes, "ς").unwrap();
    ResetSigningCertForTest();
}

/// 对应 Go TestStarterSessionTokenLifetime。
#[test]
fn starter_token_lifetime_matches_go() {
    // Go TestStarterSessionTokenLifetime only runs in NextGen.
    if !config::kerneltype::IsNextGen() {
        return;
    }
    let _guard = SESSION_TOKEN_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ResetSigningCertForTest();
    let original_mode = config::deploymode::Get();
    config::deploymode::Set(config::deploymode::Starter).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let cert_path = directory.path().join("cert.pem");
    let key_path = directory.path().join("key.pem");
    write_rsa_x509_pair(&cert_path, &key_path, 2048, MessageDigest::sha256());
    SetKeyPath(key_path.to_string_lossy().into_owned());
    SetCertPath(cert_path.to_string_lossy().into_owned());

    let token = CreateSessionToken("test_user").unwrap();
    let token_bytes = serde_json::to_vec(&token).unwrap();
    assert_eq!(token.ExpireTime - token.SignTime, starterTokenLifetime);
    assert_eq!(GetLoadCertInterval(), starterLoadCertInterval);
    SetMockNowOffset(tokenLifetime + Duration::minutes(1));
    ValidateSessionToken(&token_bytes, "test_user").unwrap();
    SetMockNowOffset(starterTokenLifetime + Duration::minutes(1));
    assert!(
        ValidateSessionToken(&token_bytes, "test_user")
            .unwrap_err()
            .to_string()
            .contains("token expired")
    );

    config::deploymode::Set(original_mode).unwrap();
    ResetSigningCertForTest();
}

/// 对应 Go TestCertExpire：轮换后旧证书先可用，超过宽限期后失效。
#[test]
fn certificate_grace_period_matches_go() {
    let _guard = SESSION_TOKEN_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ResetSigningCertForTest();
    let directory = tempfile::tempdir().unwrap();
    let cert_path = directory.path().join("cert.pem");
    let key_path = directory.path().join("key.pem");
    write_rsa_x509_pair(&cert_path, &key_path, 2048, MessageDigest::sha256());
    SetKeyPath(key_path.to_string_lossy().into_owned());
    SetCertPath(cert_path.to_string_lossy().into_owned());
    let old_token = serde_json::to_vec(&CreateSessionToken("test_user").unwrap()).unwrap();

    let new_cert_path = directory.path().join("new-cert.pem");
    let new_key_path = directory.path().join("new-key.pem");
    write_rsa_x509_pair(&new_cert_path, &new_key_path, 2048, MessageDigest::sha256());
    SetKeyPath(new_key_path.to_string_lossy().into_owned());
    SetCertPath(new_cert_path.to_string_lossy().into_owned());
    ValidateSessionToken(&old_token, "test_user").unwrap();

    SetMockNowOffset(LoadCertInterval);
    ReloadSigningCert();
    SetMockNowOffset(LoadCertInterval + oldCertValidTime + Duration::minutes(1));
    assert!(
        ValidateSessionToken(&old_token, "test_user")
            .unwrap_err()
            .to_string()
            .contains("verification")
    );
    ResetSigningCertForTest();
}

/// 对应 Go TestLoadAndReadConcurrently：5 秒并发写证书、重载、签发和验签。
#[test]
fn certificate_reload_and_reads_are_concurrent_safe() {
    let _guard = SESSION_TOKEN_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ResetSigningCertForTest();
    let directory = tempfile::tempdir().unwrap();
    let cert_path = Arc::new(directory.path().join("cert.pem"));
    let key_path = Arc::new(directory.path().join("key.pem"));
    write_rsa_x509_pair(&cert_path, &key_path, 4096, MessageDigest::sha256());
    SetKeyPath(key_path.to_string_lossy().into_owned());
    SetCertPath(cert_path.to_string_lossy().into_owned());
    let deadline = Instant::now() + StdDuration::from_secs(5);

    let writer_cert = Arc::clone(&cert_path);
    let writer_key = Arc::clone(&key_path);
    let writer = thread::spawn(move || {
        while Instant::now() < deadline {
            write_rsa_x509_pair(&writer_cert, &writer_key, 4096, MessageDigest::sha256());
            thread::sleep(StdDuration::from_secs(1));
        }
    });
    let loaders: Vec<_> = (0..2)
        .map(|_| {
            thread::spawn(move || {
                while Instant::now() < deadline {
                    ReloadSigningCert();
                    thread::sleep(StdDuration::from_millis(500));
                }
            })
        })
        .collect();
    let readers: Vec<_> = (0..3)
        .map(|id| {
            thread::spawn(move || {
                let username = format!("test_user_{id}");
                while Instant::now() < deadline {
                    let bytes =
                        serde_json::to_vec(&CreateSessionToken(&username).unwrap()).unwrap();
                    thread::sleep(StdDuration::from_millis(10));
                    ValidateSessionToken(&bytes, &username).unwrap();
                    thread::sleep(StdDuration::from_millis(10));
                }
            })
        })
        .collect();

    writer.join().unwrap();
    for loader in loaders {
        loader.join().unwrap();
    }
    for reader in readers {
        reader.join().unwrap();
    }
    ResetSigningCertForTest();
}
