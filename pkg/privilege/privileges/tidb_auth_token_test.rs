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
// Copyright 2026 AsterSQL.

// TiDB Auth Token / JWKS 单元测试。
//
// 覆盖 JWT claims 校验各分支，以及 JWKS 加载、验签、重试与密钥轮换。

// Ported from tidb_auth_token_test.go (TestAuthTokenClaims, TestJWKSImpl).
//
// Go signs/parses JWTs with lestrrat-go/jwx and inspects claims as typed
// `time.Time`/string values; Rust drives the same production `JWKSImpl` and
// `checkAuthTokenClaims` through the `jsonwebtoken` crate and plain
// `serde_json::Value` claims. Two Go-library-specific error strings are not
// reproduced verbatim (see inline comments at each site) because they come
// from jwx's internal claim-typing and verification-error formatting, which
// have no equivalent surface in `jsonwebtoken`; the underlying behavior
// (reject malformed/expired/mismatched claims and bad signatures) is fully
// exercised.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::Serialize;
use serde_json::json;

use crate::*;

/// 测试用 RSA 私钥 PEM（三把钥，对应不同 kid）。
const PRIVATE_KEY_PEMS: [&str; 3] = [
    "-----BEGIN RSA PRIVATE KEY-----
MIIEpAIBAAKCAQEAq8G5n9XBidxmBMVJKLOBsmdOHrCqGf17y9+VUXingwDUZxRp
2XbuLZLbJtLgcln1lC0L9BsogrWf7+pDhAzWovO6Ai4Aybu00tJ2u0g4j1aLiDds
y0gyvSb5FBoL08jFIH7t/JzMt4JpF487AjzvITwZZcnsrB9a9sdn2E5B/aZmpDGi
2+Isf5osnlw0zvveTwiMo9ba416VIzjntAVEvqMFHK7vyHqXbfqUPAyhjLO+iee9
9Tg5AlGfjo1s6FjeML4xX7sAMGEy8FVBWNfpRU7ryTWoSn2adzyA/FVmtBvJNQBC
MrrAhXDTMJ5FNi8zHhvzyBKHU0kBTS1UNUbP9wIDAQABAoIBAFF0sbz82imwje2L
RvP3lfXvClyBulpTHigFJEKcLw1xEkrEoqKQxcp1UFvsPKfexBn+9yFQ0/iRfIWC
m3x/vjdP0ZKBELybudkWGVsemDxadhgm+QC7f9y3I/+FjsBlAiA0MlfQYUJSpdaX
hgu8rEgdwYnFpunGgRRyY2xxSNirEAzA6aTa1PkNU6W7nF5trOUOfdUSNZuPsS4y
rQjZJZDxB4SW+biuTqNAOKPPnnFY3PdntQx9uhcSm+qiDP2yQXoXuDK/TAN4euOK
vR5POnnDNKhFizGnR8xjW8GSmfg9ILxw/BpNFoIkvZo5xLtt7lNM2VPJaLzXEse2
axOpKckCgYEA2g8GWQOmqH8M4LaOxZcy+4dvoOou4vv+V5Bn4TDtmRaQd40BqfOZ
jyi9sci7iGYVsHdSpLlLFcXedx97QKstJZZ8RKQZv/wBZ7JH6Hn80ipGnJ3a7S9+
JY99iVDF6hOroR2fbnrqa/Dx8pPdMy9ZOXZvh3Q527j8u4m9zXUXfVUCgYEAyaRG
dSEt/AJxoecZqa450H8rlOQVDC0DcQcxGlEP7L2wQRinnJkfZ6+r7jhfu4SikOZO
MdXDF/ILGxSXw6+0xHwq9XfSlNhgTTcBNZOYfchMi6mvUxe/r4TsMXEcbRPSsuWo
EZJ1oZLHxdw9B96R9blnxk54VvILG60rrwbaOBsCgYEAz8EQ4y4/Urn5ov9L96We
xVa8XCvCkDBWm0bSMhNTzE9bRQvrUejtnR/L297MDaB1ebO14YtIpm3nDsfHvk1Y
rj86FovinK+VBx8ss6nF3ta4f+9F7kUZgt+7U2DJr8Md+lsm0zP4tO7TFbMbRPEP
qVfV2tA5b8ZHxMXvOBkfUCECgYAZbFvx0rAgkRJQrnme2jex4QbWq/c3ZMmFS7nW
LphKahQ58OjZJrk98nlD/NmdI/j3OgJr6B7D+yGJVYxZAONSzrD/6A6l864YrjG5
1pUobsOv7EINwPXLJIA/L5q86f3rzmblaEjqiT4k5ULQpjBTAgBikWw80iGyaKAU
XlHPNwKBgQDC45gv8aRxJXwSjpCXHnnzoWAJHBOXIpTbQOVdGbuMRr5RAh4CVFsp
6rnNlannpnE8EMkLtAmPLNqmsP0XCRo2TpHU86PRO3OGH/3KEtU/X3ij9sts2OlM
03m9HNt6/h9glwk7NYwbGgOlKhRxr/DUTkumu0tdfYN+tLU83mBeNw==
-----END RSA PRIVATE KEY-----",
    "-----BEGIN RSA PRIVATE KEY-----
MIIEpAIBAAKCAQEAywV8/DH1vLyuTOu9MBiAF2DLlZi0SOMEUznXVSRbt0+YVfsr
o67+66B7ATnB2a5BCyOGaFJ9aIwfTWILMTJo91hVk4gHdvsSYeiS3gnSQtKYEdAX
ZgL2apGP1s08XQfluTF57fxVn8RpKieox6Ea68JSGMuh0AEr2MuJzaTcxzQ5UpIi
K2vUuBXNMzZwbZKvssfsyoZ6zIEeco4BCGXXmJUyxFb6MLV8DWKwmUQjhV/EjDem
vE0vrUziY1afo2J9Ngk03mPHqprDZEa8u2wwtm2ghuCaislKh9X7vl31Yj5lcPCU
iacBupV6/bhMjPTAgIAOEcsLVZMK2P+snREDjwIDAQABAoIBACiu/93V8SWSNeeK
Mg5KSpjkt8dRo4cbnwlChQk10P9J/v/z5knVzpXPQfb76QHDLpuZ0dxj82eY9Mjg
Bdgk/u3aEMQQtVY9d/CQ16WRGEZ1xy2Cor25iEHQy59C337RD1LuPD3ZnBr5FA3z
hpoCic+G0EbRv6pcIbo/B21jRS7Rx+w13CNZQD1fL5vEc1CTR+WL/DeCTugGcj8i
wiaUb6eu2Z4YFoJqCWGhTfz1HL4i+y12HfAlezfYae9Lhm0r/mLMos6O7gHWqW24
EbmeQZy+TGjd7SBw1wsEv7ZO+MFsfvbBvZidmK/FcxUqiyfsvhsTuRbgv6+GiMep
rF+acgkCgYEA6C7dg6GtBydIGq1iE7ty2pUcW4YPL2BVjTK7Fntt1ToVzUKZAulG
Av0+kukeReDLGxrNMhHDzGuLboA2v/PNcMnoJWnzg2+tMByyLWEvIvp9fngbSwRr
JEdDbUDQZbpEkyEC8fDAO3l3EmoHaGBEshZ0tDl0fui36vM1w8lhZDUCgYEA39jW
bsHHny4QUwwsXu/dvg8meYP2rCjBxjM7PIz1FKut+oftUmYCVhRvhZl5ydpO9/2f
VQYqHnDMlmAzjCovKvjFFMXJl2QucUHR+S94sobmTj6tfY9VzAq8uZaMi9jq5uRL
WZvmTPtj3U7KequCqCN7w14o7JkFxOGquFy5eTMCgYANr42BD8uaK1eVsvif/yGS
/s0QHAPTIBOK4h2jAp2Dvwu/8JgCUuu8i17f2/vb1JdEPr0voVpwNzqdxdL0V5OZ
fV1Ar1EaQz/rIRXjlOHpZuh0xvGc52LFXan8y6A9DtCx93Ur+6vpFYzOOg+7uEj0
UlyIrwZN4LvOjo1xv/IMrQKBgQCfsFAhSUqAa1sn87o/q/zTlnlLHPI/lP/PxkKP
CrvYGDWQUaHjM3SdNgztETUJ5ByL27nr7O7lMnExIcYESx/FFx15mTQcNVLQZzVF
ADGpooTv8tTPiw6Y9lv2RclUBtZlCx4Z+hbMelaezZOy+WHHUzD6idTGHNA5yQeC
aFvEcwKBgQC+QzEkoG7IDqrFL62x+H607juYF4IY4kXo7zsrfY4uWffC7Mf5XaYs
qkX9+ouK/CROAKO+UdMEs8PWHF1CHmgV3t/EF2+xfkGvVr/RlgtMHgQe8lX9a+sK
1xpqDpqmXTST37cy+lQGPXmWrJsTulWQj0F1LV4i4qt7Ph4JK4kzvA==
-----END RSA PRIVATE KEY-----",
    "-----BEGIN RSA PRIVATE KEY-----
MIIEowIBAAKCAQEAoaJsIxrUBKPW/dogPpUxxhiL8cpUt8uWlclOrmUSHFZzY50r
wsCt2ndnZRHE/HD+X7oCo28pdYTySZWnsiY/K2HeyYdsRzUH71Mx0Z+a1uBa0k6B
VHY8vrPObLCPFEmxnqml+Wj74zocsR23/puCz8Vgm+0VF49vu+ab90lc2iLJtElv
eRnLrSkaudCUndmn+aVftwnpDxJ4Z0rRlJkhyeZMN4+EMse5+0hAhg5UiPHE6pG8
RI3zYnp0EYKvN+M9/cdNntyuKCCCvOCi4b4d4wpGOrDuiA/moh2J9zwPBMiyvIFo
zUMmqAQV3zuUxx+jAAjrc9ReQLnoExhuhfrU5wIDAQABAoIBAC62PgI3MqbUosFi
VIdBnszdMzSBgNJNKAvJzc9grkc6RMa5GXiDLrtAXsU6yW8bSKhpnXGWIqkv7sWN
VpWJsB/dfQFI/eXmUZC8vl0SfzEyTY0R2xaJxSxn0nRe4jq+wXJVHP5jdMhKdxhI
um/+iWN6a10kuz+/2E65asGglhEEHxzm9ux9PGbhOR7NVAiReRfEKN0UgmD9jWHL
nR2uBsS3BsPBURKBERzYOqGmxMgOq9Y07Jf6d4Ln33SfkKsD/ibTPoUyTsvwG0g1
J7wVmqZRxG7GLGxLjjs+s16LWjRKUCbHOf7VIMKkYj2HBgzMZOG6/f579mejB//D
K5rSwuECgYEA0doT9iTVq9ZbpYKfLH5HzqoTuZeP5Q5acO2ZM2Bat6Xdk/ZgOgh4
Gvzgfi33kl03Pp2ZhQX9m1k1eicTcDPvNQZ3JeTI7bgO3ZQXt8PkCL/pPCZyfusB
C9sP4zhhmieLuX7SZmkWpJvy1XtjvJsyhnnZz2s51nvCKKAe6JVFatUCgYEAxS3f
yFOBzRAyuPWUF4pGTAVfysM47Zl0alDcZgM30ARhqhsfHOo26xeU6TEWucCh30fS
tehXlQDlygHN1+CxkqH6mv0Nlp1j/1YV9mZIEZ++jIggAgsit29YtoQMIe6/lv0+
+aivyNJrCtbgm9ZA4+OOie3Cvjf/6qnqnBSFpssCgYBWkfyCIpfzF68fDE/V7xJ4
czlH6vp1qAIvbBUzWKCT+lz6WT1BM5U4rPF/nD7xpnrP3fwjIGGK4LZq+gvO0d3w
pgYpH8S0LKYVSq6uJKXB5km1grbhHNmFpo1bUzsQeRfvIh5yGRA6QAthflGa0Pt6
9nGgW7+0d8GVONkHYe0NMQKBgAJU64uL6UIKif8D8G9i1Df77EkSi+7LXMQRFroi
GZvdIWaIkZKe9m1LRxiG2xTxQTjJuaUrDTYW36DG6q892fu47KS+j1WToOYZF4Nl
bD7BG9i/l1lO1mdC6tKltxsDnsJjVkZPh1yhmGB1cAyHuRa4zyu0YxQqx1z4C20z
FO2HAoGBALp9nGqbK6N96LYgef8GpP6o5pz3D1Jtj18iYyn3oz6z9t3dqNbpf2vh
cYnDqCQWSX5rfDRMbuhEJB+GvHYKVY/yVJ2ZWu1cKsB+2gzsITWewfxTS/ns+4Qk
RfViImdNIa19f7cmeC8RjhaSWBmb9JJk+p75e4XpgD1bG9U7DjiH
-----END RSA PRIVATE KEY-----",
];

/// 与私钥配对的 RSA 公钥 PEM。
const PUBLIC_KEY_PEMS: [&str; 3] = [
    "-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAq8G5n9XBidxmBMVJKLOB
smdOHrCqGf17y9+VUXingwDUZxRp2XbuLZLbJtLgcln1lC0L9BsogrWf7+pDhAzW
ovO6Ai4Aybu00tJ2u0g4j1aLiDdsy0gyvSb5FBoL08jFIH7t/JzMt4JpF487Ajzv
ITwZZcnsrB9a9sdn2E5B/aZmpDGi2+Isf5osnlw0zvveTwiMo9ba416VIzjntAVE
vqMFHK7vyHqXbfqUPAyhjLO+iee99Tg5AlGfjo1s6FjeML4xX7sAMGEy8FVBWNfp
RU7ryTWoSn2adzyA/FVmtBvJNQBCMrrAhXDTMJ5FNi8zHhvzyBKHU0kBTS1UNUbP
9wIDAQAB
-----END PUBLIC KEY-----",
    "-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAywV8/DH1vLyuTOu9MBiA
F2DLlZi0SOMEUznXVSRbt0+YVfsro67+66B7ATnB2a5BCyOGaFJ9aIwfTWILMTJo
91hVk4gHdvsSYeiS3gnSQtKYEdAXZgL2apGP1s08XQfluTF57fxVn8RpKieox6Ea
68JSGMuh0AEr2MuJzaTcxzQ5UpIiK2vUuBXNMzZwbZKvssfsyoZ6zIEeco4BCGXX
mJUyxFb6MLV8DWKwmUQjhV/EjDemvE0vrUziY1afo2J9Ngk03mPHqprDZEa8u2ww
tm2ghuCaislKh9X7vl31Yj5lcPCUiacBupV6/bhMjPTAgIAOEcsLVZMK2P+snRED
jwIDAQAB
-----END PUBLIC KEY-----",
    "-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAoaJsIxrUBKPW/dogPpUx
xhiL8cpUt8uWlclOrmUSHFZzY50rwsCt2ndnZRHE/HD+X7oCo28pdYTySZWnsiY/
K2HeyYdsRzUH71Mx0Z+a1uBa0k6BVHY8vrPObLCPFEmxnqml+Wj74zocsR23/puC
z8Vgm+0VF49vu+ab90lc2iLJtElveRnLrSkaudCUndmn+aVftwnpDxJ4Z0rRlJkh
yeZMN4+EMse5+0hAhg5UiPHE6pG8RI3zYnp0EYKvN+M9/cdNntyuKCCCvOCi4b4d
4wpGOrDuiA/moh2J9zwPBMiyvIFozUMmqAQV3zuUxx+jAAjrc9ReQLnoExhuhfrU
5wIDAQAB
-----END PUBLIC KEY-----",
];

/// 测试用户邮箱 / sub 声明。
const EMAIL1: &str = "user1@pingcap.com";
/// 用于断言 mismatch 的另一邮箱。
const EMAIL2: &str = "user2@pingcap.com";
/// 合法 issuer。
const ISSUER1: &str = "issuer1";
/// 用于断言 Wrong iss 的另一 issuer。
const ISSUER2: &str = "issuer2";

#[derive(Serialize)]
/// 签名 JWT 时使用的 Auth Token claims。
struct AuthClaims {
    sub: String,
    email: String,
    iat: i64,
    exp: i64,
    iss: String,
}

/// 从公钥 PEM 提取 JWK 的 n/e（URL-safe Base64）。
fn rsa_jwk_n_e(pem: &str) -> (String, String) {
    let rsa = openssl::rsa::Rsa::public_key_from_pem(pem.as_bytes()).expect("parse RSA public key");
    (
        URL_SAFE_NO_PAD.encode(rsa.n().to_vec()),
        URL_SAFE_NO_PAD.encode(rsa.e().to_vec()),
    )
}

/// 按密钥下标集合构造 JWKS JSON 字符串。
fn jwks_json(indices: &[usize]) -> String {
    let keys: Vec<_> = indices
        .iter()
        .map(|&i| {
            let (n, e) = rsa_jwk_n_e(PUBLIC_KEY_PEMS[i]);
            json!({
                "kty": "RSA",
                "n": n,
                "e": e,
                "alg": "RS256",
                "use": "sig",
                "kid": format!("the-key-id-{i}"),
            })
        })
        .collect();
    json!({ "keys": keys }).to_string()
}

/// 将 JWKS 写到临时目录并返回文件路径。
fn write_jwks(dir: &tempfile::TempDir, name: &str, indices: &[usize]) -> String {
    let path = dir.path().join(name);
    std::fs::write(&path, jwks_json(indices)).expect("write jwks fixture");
    path.to_string_lossy().into_owned()
}

/// 用指定私钥与 kid 签发 RS256 JWT。
fn signed_token(key_index: usize, kid: &str, claims: &AuthClaims) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(kid.into());
    let key = EncodingKey::from_rsa_pem(PRIVATE_KEY_PEMS[key_index].as_bytes())
        .expect("load RSA private key");
    jsonwebtoken::encode(&header, claims, &key).expect("sign JWT")
}

/// 当前 Unix 秒时间戳。
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

// TestAuthTokenClaims ported: signs a real JWT with an RSA key, verifies it
// through the production JWKS pipeline, then exercises every branch of
// checkAuthTokenClaims by mutating the verified claim map.
#[test]
/// 验签后逐字段篡改 claims，覆盖 checkAuthTokenClaims 全部分支。
fn test_auth_token_claims() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path0 = write_jwks(&dir, "jwks0.json", &[0]);

    let mut jwks_impl = JWKSImpl::new();
    jwks_impl
        .LoadJWKS4AuthToken(None, path0.clone(), Duration::from_secs(3600))
        .unwrap_or_else(|err| panic!("load jwks {path0}: {err}"));

    let now = now_unix();
    let claims = AuthClaims {
        sub: EMAIL1.into(),
        email: EMAIL1.into(),
        iat: now,
        exp: now + 100 * 3600,
        iss: ISSUER1.into(),
    };
    let token = signed_token(0, "the-key-id-0", &claims);
    let mut verified = jwks_impl
        .checkSigWithRetry(&token, 0)
        .expect("verify token");
    assert_eq!(verified.get("sub").unwrap().as_str().unwrap(), EMAIL1);
    assert_eq!(verified.get("email").unwrap().as_str().unwrap(), EMAIL1);
    assert_eq!(verified.get("iss").unwrap().as_str().unwrap(), ISSUER1);
    assert_eq!(verified.get("iat").unwrap().as_i64().unwrap(), now);
    assert_eq!(
        verified.get("exp").unwrap().as_i64().unwrap(),
        now + 100 * 3600
    );

    let mut record = NewUserRecord("", EMAIL1);
    record.AuthTokenIssuer = ISSUER1.into();
    record.UserAttributesInfo.MetadataInfo.Email = EMAIL1.into();

    // Success.
    checkAuthTokenClaims(&verified, &record, defaultTokenLife).expect("claims should be valid");

    // 'sub'
    verified.insert("sub".into(), json!(EMAIL2));
    let err = checkAuthTokenClaims(&verified, &record, defaultTokenLife).unwrap_err();
    assert!(err.to_string().contains("Wrong 'sub'"), "{err}");
    verified.remove("sub");
    let err = checkAuthTokenClaims(&verified, &record, defaultTokenLife).unwrap_err();
    assert!(err.to_string().contains("lack 'sub'"), "{err}");
    verified.insert("sub".into(), json!(EMAIL1));

    // 'email'
    verified.insert("email".into(), json!(EMAIL2));
    let err = checkAuthTokenClaims(&verified, &record, defaultTokenLife).unwrap_err();
    assert!(err.to_string().contains("Wrong 'email'"), "{err}");
    verified.remove("email");
    let err = checkAuthTokenClaims(&verified, &record, defaultTokenLife).unwrap_err();
    assert!(err.to_string().contains("lack 'email'"), "{err}");
    verified.insert("email".into(), json!(EMAIL1));

    // 'iat': missing / out-of-life / issued-in-the-future.
    // Go additionally asserts a type-mismatch message ("iat: abc is not a
    // value of time.Time") produced by jwx's typed claim decoder; our claims
    // are plain serde_json::Value so a non-numeric 'iat' collapses into the
    // same "lack 'iat'" branch as a missing claim, which is exercised above.
    verified.remove("iat");
    let err = checkAuthTokenClaims(&verified, &record, defaultTokenLife).unwrap_err();
    assert!(err.to_string().contains("lack 'iat'"), "{err}");
    // Go sleeps 2s here so `now` (the token's `iat`) is provably older than
    // the 1s token life being tested against; mirror that real-time wait.
    std::thread::sleep(Duration::from_secs(2));
    verified.insert("iat".into(), json!(now));
    let err = checkAuthTokenClaims(&verified, &record, Duration::from_secs(1)).unwrap_err();
    assert!(err.to_string().contains("out of its life time"), "{err}");
    verified.insert("iat".into(), json!(now + 3600));
    let err = checkAuthTokenClaims(&verified, &record, defaultTokenLife).unwrap_err();
    assert!(err.to_string().contains("issued at a future time"), "{err}");
    verified.insert("iat".into(), json!(now));

    // 'exp': missing / already expired.
    verified.remove("exp");
    let err = checkAuthTokenClaims(&verified, &record, defaultTokenLife).unwrap_err();
    assert!(err.to_string().contains("lack 'exp'"), "{err}");
    verified.insert("exp".into(), json!(now));
    let err = checkAuthTokenClaims(&verified, &record, defaultTokenLife).unwrap_err();
    assert!(err.to_string().contains("has been expired"), "{err}");
    verified.insert("exp".into(), json!(now + 100 * 3600));

    // 'iss': missing / mismatched issuer.
    verified.remove("iss");
    let err = checkAuthTokenClaims(&verified, &record, defaultTokenLife).unwrap_err();
    assert!(err.to_string().contains("lack 'iss'"), "{err}");
    verified.insert("iss".into(), json!(ISSUER1));
    record.AuthTokenIssuer = ISSUER2.into();
    let err = checkAuthTokenClaims(&verified, &record, defaultTokenLife).unwrap_err();
    assert!(err.to_string().contains("Wrong 'iss'"), "{err}");
}

// TestJWKSImpl ported: exercises LoadJWKS4AuthToken/load/verify/checkSigWithRetry
// against good and bad paths, tampered signatures, and key rotation across
// JWKS files containing different key subsets.
#[test]
/// 覆盖 JWKS 错误路径、签名篡改、kid 不匹配与密钥轮换。
fn test_jwksimpl() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path0 = write_jwks(&dir, "jwks0.json", &[0]);
    let path1 = write_jwks(&dir, "jwks1.json", &[0, 1]);
    let path2 = write_jwks(&dir, "jwks2.json", &[2]);

    let mut jwks_impl = JWKSImpl::new();

    // Wrong path.
    assert!(
        jwks_impl
            .LoadJWKS4AuthToken(None, "wrong-jwks-path", Duration::from_secs(3600))
            .is_err()
    );
    assert!(jwks_impl.load().is_err());
    assert!(
        jwks_impl
            .checkSigWithRetry("invalid tokenString", 4)
            .is_err()
    );
    assert!(jwks_impl.verify(b"invalid tokenString").is_err());

    jwks_impl
        .LoadJWKS4AuthToken(None, path0.clone(), Duration::from_secs(3600))
        .unwrap_or_else(|err| panic!("load jwks {path0}: {err}"));
    let now = now_unix();
    let mut claims = AuthClaims {
        sub: EMAIL1.into(),
        email: EMAIL1.into(),
        iat: now,
        exp: now + 100 * 3600,
        iss: ISSUER1.into(),
    };
    let signed = signed_token(0, "the-key-id-0", &claims);
    let parts: Vec<&str> = signed.split('.').collect();

    // Wrong encoded JWT format (missing/extra segments).
    let err = jwks_impl
        .checkSigWithRetry(&format!("{}.{}", parts[0], parts[1]), 0)
        .unwrap_err();
    assert!(err.to_string().contains("Invalid JWT"), "{err}");
    let err = jwks_impl
        .checkSigWithRetry(&format!("{signed}.{}", parts[1]), 0)
        .unwrap_err();
    assert!(err.to_string().contains("Invalid JWT"), "{err}");

    // Wrong signature. jsonwebtoken reports this as `InvalidSignature`
    // (jwx's wording "could not verify message using any of the signatures
    // or keys" has no equivalent string in the `jsonwebtoken` crate); we
    // assert on the crate's own error text instead of Go's literal message.
    let err = jwks_impl
        .checkSigWithRetry(&format!("{signed}A"), 0)
        .unwrap_err();
    assert!(err.to_string().contains("InvalidSignature"), "{err}");

    // Wrong signature, and fails to reload JWKS from a bad path.
    jwks_impl.filepath = "wrong-path".into();
    let err = jwks_impl
        .checkSigWithRetry(&format!("{signed}A"), 0)
        .unwrap_err();
    assert!(err.to_string().contains("I/O error"), "{err}");
    jwks_impl.filepath = path0.clone();

    jwks_impl
        .LoadJWKS4AuthToken(None, path0.clone(), Duration::from_secs(3600))
        .unwrap_or_else(|err| panic!("load jwks {path0}: {err}"));
    jwks_impl
        .checkSigWithRetry(&signed, 0)
        .expect("valid token should verify");

    // Wrong kid: key 1's id is not present in jwks0 (only key 0).
    let signed_wrong_kid = signed_token(0, "the-key-id-1", &claims);
    assert!(jwks_impl.checkSigWithRetry(&signed_wrong_kid, 0).is_err());

    claims.iat = now_unix();
    let signed_key0 = signed_token(0, "the-key-id-0", &claims);
    jwks_impl
        .LoadJWKS4AuthToken(None, path1.clone(), Duration::from_secs(3600))
        .unwrap_or_else(|err| panic!("load jwks {path1}: {err}"));
    jwks_impl
        .checkSigWithRetry(&signed_key0, 0)
        .expect("key 0 should still verify against jwks1");

    jwks_impl
        .LoadJWKS4AuthToken(None, path2.clone(), Duration::from_secs(3600))
        .unwrap_or_else(|err| panic!("load jwks {path2}: {err}"));
    assert!(
        jwks_impl.checkSigWithRetry(&signed_key0, 0).is_err(),
        "jwks2 only contains key 2, key 0's kid should no longer resolve"
    );
}

#[test]
/// 首次加载失败后，已启动的后台刷新仍应在 JWKS 文件出现时恢复。
fn test_jwks_refresh_recovers_after_initial_load_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("late-jwks.json");
    let path = path.to_string_lossy().into_owned();
    let cancellation = CancellationToken::default();
    let mut jwks_impl = JWKSImpl::new();

    assert!(
        jwks_impl
            .LoadJWKS4AuthToken(
                Some(cancellation.clone()),
                path.clone(),
                Duration::from_millis(10),
            )
            .is_err()
    );

    std::fs::write(&path, jwks_json(&[0])).expect("write delayed jwks fixture");
    let claims = AuthClaims {
        sub: EMAIL1.into(),
        email: EMAIL1.into(),
        iat: now_unix(),
        exp: now_unix() + 3600,
        iss: ISSUER1.into(),
    };
    let token = signed_token(0, "the-key-id-0", &claims);
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while std::time::Instant::now() < deadline {
        if jwks_impl.verify(token.as_bytes()).is_ok() {
            cancellation.cancel();
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    cancellation.cancel();
    panic!("background refresh did not recover after the initial load failure");
}
