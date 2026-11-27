//! AWS Signature Version 4 for the Bedrock runtime: the `Authorization` header from IAM
//! credentials ([`AwsCredentials`]). Pure Rust on `sha2` (HMAC-SHA256 is written out here, RFC
//! 2104); no AWS SDK. Pure: the time is an argument.
//!
//! - **Canonical request**: the method; the path as sent, URI-encoded once more (AWS's rule for
//!   every service but S3, so a model id's `%3A` is signed as `%253A`); the query pairs sorted
//!   and encoded; the signed headers lowercased, trimmed and sorted; the hex SHA-256 of the body.
//! - **Signed headers**: every header the caller passes. [`bedrock_headers`] passes `host`,
//!   `content-type`, `x-amz-date` and, with a session token, `x-amz-security-token`.
//! - **Not supported** (not needed for one `POST` to Bedrock): path normalization
//!   (`.` and `..` segments), unsigned or chunked payloads, `SigV4a`.
//!
//! Credentials are [`Secret`]s: they never appear in `Debug` output, and the derived keys live
//! only on the stack of [`authorization`].

use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use super::endpoint::{AwsCredentials, Secret};

/// The algorithm name in the header and the string to sign.
pub const ALGORITHM: &str = "AWS4-HMAC-SHA256";
/// The signing name of the Bedrock runtime.
pub const BEDROCK_SERVICE: &str = "bedrock";

/// One request to sign. `path` and `query` are as sent on the wire (already percent-encoded).
#[derive(Clone, Copy, Debug)]
pub struct Signable<'a> {
    /// `GET`, `POST`, ...
    pub method: &'a str,
    /// The path as sent (`/model/qwen.q3%3A0/converse`).
    pub path: &'a str,
    /// The query string without `?` (empty when none).
    pub query: &'a str,
    /// Every header to sign, `x-amz-date` among them (names in any case).
    pub headers: &'a [(&'a str, &'a str)],
    /// The body.
    pub payload: &'a [u8],
}

/// The scope of a signature: date (`YYYYMMDD`), region and service.
#[derive(Clone, Copy, Debug)]
pub struct Scope<'a> {
    /// `YYYYMMDDTHHMMSSZ`, the same as the `x-amz-date` header.
    pub amz_date: &'a str,
    /// Region (`eu-west-2`).
    pub region: &'a str,
    /// Service signing name (`bedrock`).
    pub service: &'a str,
}

impl Scope<'_> {
    fn date(&self) -> &str {
        self.amz_date.get(..8).unwrap_or(self.amz_date)
    }

    /// `YYYYMMDD/region/service/aws4_request`.
    pub fn credential_scope(&self) -> String {
        format!(
            "{}/{}/{}/aws4_request",
            self.date(),
            self.region,
            self.service
        )
    }
}

/// The canonical request of `r` (step 1 of `SigV4`), and the signed-header list.
pub fn canonical_request(r: &Signable<'_>) -> (String, String) {
    let mut headers: Vec<(String, String)> = r
        .headers
        .iter()
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), collapse(v)))
        .collect();
    headers.sort_by(|a, b| a.0.cmp(&b.0));
    // Repeated names: one line, values comma-joined in the order given.
    let mut merged: Vec<(String, String)> = Vec::with_capacity(headers.len());
    for (k, v) in headers {
        match merged.last_mut() {
            Some((lk, lv)) if *lk == k => {
                lv.push(',');
                lv.push_str(&v);
            }
            _ => merged.push((k, v)),
        }
    }
    let signed = merged
        .iter()
        .map(|(k, _)| k.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let mut out = String::new();
    out.push_str(&r.method.to_ascii_uppercase());
    out.push('\n');
    out.push_str(&uri_encode(
        if r.path.is_empty() { "/" } else { r.path },
        false,
    ));
    out.push('\n');
    out.push_str(&canonical_query(r.query));
    out.push('\n');
    for (k, v) in &merged {
        let _ = writeln!(out, "{k}:{v}");
    }
    out.push('\n');
    out.push_str(&signed);
    out.push('\n');
    out.push_str(&hex(&Sha256::digest(r.payload)));
    (out, signed)
}

/// The string to sign (step 2) for a canonical request.
pub fn string_to_sign(scope: &Scope<'_>, canonical: &str) -> String {
    format!(
        "{ALGORITHM}\n{}\n{}\n{}",
        scope.amz_date,
        scope.credential_scope(),
        hex(&Sha256::digest(canonical.as_bytes()))
    )
}

/// The signing key (step 3): `HMAC(HMAC(HMAC(HMAC("AWS4" + secret, date), region), service),
/// "aws4_request")`.
pub fn signing_key(secret: &Secret, scope: &Scope<'_>) -> [u8; 32] {
    let k_date = hmac(
        format!("AWS4{}", secret.expose()).as_bytes(),
        scope.date().as_bytes(),
    );
    let k_region = hmac(&k_date, scope.region.as_bytes());
    let k_service = hmac(&k_region, scope.service.as_bytes());
    hmac(&k_service, b"aws4_request")
}

/// The `Authorization` header value for `r` signed with `creds` in `scope`.
pub fn authorization(creds: &AwsCredentials, scope: &Scope<'_>, r: &Signable<'_>) -> String {
    let (canonical, signed) = canonical_request(r);
    let sts = string_to_sign(scope, &canonical);
    let key = signing_key(&creds.secret_access_key, scope);
    let signature = hex(&hmac(&key, sts.as_bytes()));
    format!(
        "{ALGORITHM} Credential={}/{}, SignedHeaders={signed}, Signature={signature}",
        creds.access_key_id.expose(),
        scope.credential_scope()
    )
}

/// The headers of a signed Bedrock `POST` of `body` to `url` (JSON) at `now`: `host`,
/// `x-amz-date`, `x-amz-security-token` (with a session token) and `authorization`.
/// `content-type: application/json` is signed too; the client adds it.
pub fn bedrock_headers(
    creds: &AwsCredentials,
    region: &str,
    url: &str,
    body: &str,
    now: SystemTime,
) -> Vec<(&'static str, String)> {
    let amz_date = amz_date(now);
    let host = signing_host(url);
    let (path, query) = path_and_query(url);
    let mut signed: Vec<(&str, &str)> = vec![
        ("content-type", "application/json"),
        ("host", &host),
        ("x-amz-date", &amz_date),
    ];
    if let Some(t) = &creds.session_token {
        signed.push(("x-amz-security-token", t.expose()));
    }
    let scope = Scope {
        amz_date: &amz_date,
        region,
        service: BEDROCK_SERVICE,
    };
    let auth = authorization(
        creds,
        &scope,
        &Signable {
            method: "POST",
            path,
            query,
            headers: &signed,
            payload: body.as_bytes(),
        },
    );
    let mut out = vec![("host", host.clone()), ("x-amz-date", amz_date.clone())];
    if let Some(t) = &creds.session_token {
        out.push(("x-amz-security-token", t.expose().to_owned()));
    }
    out.push(("authorization", auth));
    out
}

/// `YYYYMMDDTHHMMSSZ` (UTC) of `t`; the epoch for a time before it.
pub fn amz_date(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Year, month, day of a day count since 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (
        y,
        u32::try_from(m).unwrap_or(1),
        u32::try_from(d).unwrap_or(1),
    )
}

/// The `host` header for `url`: lowercase, without user info and without the scheme's default
/// port. The request sends exactly this value, so the signature covers what the server sees.
pub fn signing_host(url: &str) -> String {
    let host = super::endpoint::host_of(url).to_ascii_lowercase();
    let https = url
        .get(..8)
        .is_some_and(|s| s.eq_ignore_ascii_case("https://"));
    let default = if https { ":443" } else { ":80" };
    match host.strip_suffix(default) {
        Some(h) if !h.is_empty() => h.to_owned(),
        _ => host,
    }
}

/// The path (`/` when empty) and the query string of `url`.
fn path_and_query(url: &str) -> (&str, &str) {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let rest = rest.split('#').next().unwrap_or("");
    let path_start = rest.find('/').unwrap_or(rest.len());
    let tail = &rest[path_start..];
    let (path, query) = tail.split_once('?').unwrap_or((tail, ""));
    (if path.is_empty() { "/" } else { path }, query)
}

/// Sorted, encoded `k=v` pairs joined by `&`.
fn canonical_query(query: &str) -> String {
    let mut pairs: Vec<(String, String)> = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (
                uri_encode(&percent_decode(k), true),
                uri_encode(&percent_decode(v), true),
            )
        })
        .collect();
    pairs.sort();
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// AWS's `UriEncode`: every byte but `A-Za-z0-9-._~` as `%XX` (uppercase); `/` kept unless
/// `slash`.
fn uri_encode(s: &str, slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'.' | b'_' | b'~')
            || (b == b'/' && !slash)
        {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// `%XX` sequences decoded (`+` stays `+`); invalid ones kept as they are.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(h) = s.get(i + 1..i + 3)
            && let Ok(v) = u8::from_str_radix(h, 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Header value trimmed, inner runs of spaces collapsed to one.
fn collapse(v: &str) -> String {
    v.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// HMAC-SHA256 (RFC 2104) of `msg` under `key`.
pub fn hmac(key: &[u8], msg: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let inner = Sha256::new()
        .chain_update(ipad)
        .chain_update(msg)
        .finalize();
    let outer = Sha256::new()
        .chain_update(opad)
        .chain_update(inner)
        .finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&outer);
    out
}

/// Lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    //! The vectors are AWS's published ones: the `aws-sig-v4-test-suite` (as shipped in
    //! botocore's `tests/unit/auth/aws4_testsuite`, credentials `AKIDEXAMPLE`, 2015-08-30,
    //! `us-east-1`, service `service`), the signing-key and IAM `ListUsers` examples of the
    //! `SigV4` docs, and RFC 4231 for HMAC-SHA256.
    use super::*;
    use std::time::Duration;

    const SECRET: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";
    const DATE: &str = "20150830T123600Z";
    const HOST: (&str, &str) = ("Host", "example.amazonaws.com");
    const XDATE: (&str, &str) = ("X-Amz-Date", DATE);
    const EMPTY_SHA: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const TOKEN: &str = "AQoDYXdzEPT//////////wEXAMPLEtc764bNrC9SAPBSM22wDOk4x4HIZ8j4FZTwdQWLWsKWHGBuFqwAeMicRXmxfpSPfIeoIYRqTflfKD8YUuwthAx7mSEI/qkPpKPi/kMcGdQrmGdeehM4IC1NtBmUpp2wUE8phUZampKsburEDy0KPkyQDYwT7WZ0wq5VSXDvp75YU9HFvlRd8Tx6q6fE8YQcHNVXAkiY9q6d+xo0rKwT38xVqr7ZD0u0iPPkUL64lIZbqBAz+scqKmlzm8FDrypNC9Yjc8fPOLn9FX9KSYvKTr4rvx3iSIlTJabIQwj2ICCR/oLxBA==";

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn creds() -> AwsCredentials {
        AwsCredentials::new("AKIDEXAMPLE", SECRET)
    }

    fn scope() -> Scope<'static> {
        Scope {
            amz_date: DATE,
            region: "us-east-1",
            service: "service",
        }
    }

    fn sig(hex: &str, signed: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
             SignedHeaders={signed}, Signature={hex}"
        )
    }

    fn sign(method: &str, path: &str, query: &str, headers: &[(&str, &str)], body: &str) -> String {
        let r = Signable {
            method,
            path,
            query,
            headers,
            payload: body.as_bytes(),
        };
        authorization(&creds(), &scope(), &r)
    }

    #[test]
    fn hmac_sha256_rfc4231() {
        // Test case 1 and test case 6 (a key longer than the block).
        assert_eq!(
            hex(&hmac(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        assert_eq!(
            hex(&hmac(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn signing_keys_of_the_docs_examples() {
        let key = |amz_date| {
            let s = Scope {
                amz_date,
                region: "us-east-1",
                service: "iam",
            };
            hex(&signing_key(&Secret::new(SECRET), &s))
        };
        assert_eq!(
            key("20120215"),
            "f4780e2d9f65fa895f9c67b32ce1baf0b0d8a43505a000a1a9e090d414db404d"
        );
        assert_eq!(
            key(DATE),
            "c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9"
        );
    }

    #[test]
    fn iam_list_users_example_of_the_docs() {
        let headers = [
            (
                "Content-Type",
                "application/x-www-form-urlencoded; charset=utf-8",
            ),
            ("Host", "iam.amazonaws.com"),
            XDATE,
        ];
        let r = Signable {
            method: "GET",
            path: "/",
            query: "Action=ListUsers&Version=2010-05-08",
            headers: &headers,
            payload: b"",
        };
        let s = Scope {
            amz_date: DATE,
            region: "us-east-1",
            service: "iam",
        };
        let (creq, _) = canonical_request(&r);
        assert_eq!(
            hex(&Sha256::digest(creq.as_bytes())),
            "f536975d06c0309214f805bb90ccff089219ecd68b2577efef23edd43b7e1a59"
        );
        assert!(authorization(&creds(), &s, &r).ends_with(
            "SignedHeaders=content-type;host;x-amz-date, \
             Signature=5d672d79c15b13162d9279b0855cfba6789a8edb4c82c400e06b5924a6f2b5d7"
        ));
    }

    #[test]
    fn get_vanilla() {
        let r = Signable {
            method: "GET",
            path: "/",
            query: "",
            headers: &[HOST, XDATE],
            payload: b"",
        };
        let (creq, signed) = canonical_request(&r);
        assert_eq!(
            creq,
            format!(
                "GET\n/\n\nhost:example.amazonaws.com\nx-amz-date:{DATE}\n\nhost;x-amz-date\n{EMPTY_SHA}"
            )
        );
        assert_eq!(signed, "host;x-amz-date");
        assert_eq!(
            sign("GET", "/", "", &[HOST, XDATE], ""),
            sig(
                "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31",
                "host;x-amz-date"
            )
        );
    }

    #[test]
    fn get_vanilla_query_order_key_case() {
        assert_eq!(
            canonical_query("Param2=value2&Param1=value1"),
            "Param1=value1&Param2=value2"
        );
        assert_eq!(
            sign(
                "GET",
                "/",
                "Param2=value2&Param1=value1",
                &[HOST, XDATE],
                ""
            ),
            sig(
                "b97d918cfa904a5beff61c982a1b6f458b799221646efd99d3219ec94cdf2500",
                "host;x-amz-date"
            )
        );
    }

    #[test]
    fn get_utf8() {
        assert_eq!(uri_encode("/\u{1234}", false), "/%E1%88%B4");
        assert_eq!(
            sign("GET", "/\u{1234}", "", &[HOST, XDATE], ""),
            sig(
                "8318018e0b0f223aa2bbf98705b62bb787dc9c0e678f255a891fd03141be5d85",
                "host;x-amz-date"
            )
        );
    }

    #[test]
    fn post_vanilla() {
        assert_eq!(
            sign("POST", "/", "", &[HOST, XDATE], ""),
            sig(
                "5da7c1a2acd57cee7505fc6676e4e544621c30862966e37dddb68e92efbe5d6b",
                "host;x-amz-date"
            )
        );
    }

    #[test]
    fn post_x_www_form_urlencoded() {
        let headers = [
            ("Content-Type", "application/x-www-form-urlencoded"),
            HOST,
            XDATE,
        ];
        let r = Signable {
            method: "POST",
            path: "/",
            query: "",
            headers: &headers,
            payload: b"Param1=value1",
        };
        let (creq, _) = canonical_request(&r);
        assert_eq!(
            string_to_sign(&scope(), &creq),
            format!(
                "AWS4-HMAC-SHA256\n{DATE}\n20150830/us-east-1/service/aws4_request\n\
                 42a5e5bb34198acb3e84da4f085bb7927f2bc277ca766e6d19c73c2154021281"
            )
        );
        assert_eq!(
            sign("POST", "/", "", &headers, "Param1=value1"),
            sig(
                "ff11897932ad3f4e8b18135d722051e5ac45fc38421b1da7b9d196a0fe09473a",
                "content-type;host;x-amz-date"
            )
        );
    }

    #[test]
    fn post_sts_header_before() {
        assert_eq!(
            sign(
                "POST",
                "/",
                "",
                &[HOST, XDATE, ("X-Amz-Security-Token", TOKEN)],
                ""
            ),
            sig(
                "85d96828115b5dc0cfc3bd16ad9e210dd772bbebba041836c64533a82be05ead",
                "host;x-amz-date;x-amz-security-token"
            )
        );
    }

    #[test]
    fn header_values_are_trimmed_and_repeats_joined() {
        let r = Signable {
            method: "get",
            path: "",
            query: "",
            headers: &[("My-Header1", "  a   b  "), ("my-header1", "c"), HOST],
            payload: b"",
        };
        let (creq, signed) = canonical_request(&r);
        assert!(creq.starts_with("GET\n/\n\nhost:example.amazonaws.com\nmy-header1:a b,c\n"));
        assert_eq!(signed, "host;my-header1");
    }

    #[test]
    fn bedrock_paths_are_encoded_twice() {
        // The path is sent with `%3A`; AWS signs every service but S3 with it encoded again.
        let r = Signable {
            method: "POST",
            path: "/model/qwen.q3%3A0/converse",
            query: "",
            headers: &[HOST],
            payload: b"{}",
        };
        assert!(
            canonical_request(&r)
                .0
                .starts_with("POST\n/model/qwen.q3%253A0/converse\n")
        );
    }

    #[test]
    fn dates_hosts_and_paths() {
        assert_eq!(amz_date(UNIX_EPOCH), "19700101T000000Z");
        let t = at(1_440_938_160); // the suite's date
        assert_eq!(amz_date(t), DATE);
        let leap = at(1_709_164_799); // 2024-02-28 23:59:59
        assert_eq!(amz_date(leap), "20240228T235959Z");
        assert_eq!(amz_date(at(1_709_164_799 + 86_400)), "20240229T235959Z");
        assert_eq!(
            signing_host("https://Bedrock-Runtime.eu-west-2.amazonaws.com:443/x"),
            "bedrock-runtime.eu-west-2.amazonaws.com"
        );
        assert_eq!(signing_host("http://127.0.0.1:8080"), "127.0.0.1:8080");
        assert_eq!(
            path_and_query("http://h:1/model/a%3A0/converse?x=1#f"),
            ("/model/a%3A0/converse", "x=1")
        );
        assert_eq!(path_and_query("https://h"), ("/", ""));
    }

    #[test]
    fn bedrock_headers_sign_the_token_and_hide_the_secret() {
        let c = AwsCredentials::new("AKIDEXAMPLE", SECRET).with_session_token(TOKEN);
        let t = at(1_440_938_160);
        let h = bedrock_headers(
            &c,
            "eu-west-2",
            "https://bedrock-runtime.eu-west-2.amazonaws.com/model/m/converse",
            "{}",
            t,
        );
        let names: Vec<&str> = h.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            names,
            [
                "host",
                "x-amz-date",
                "x-amz-security-token",
                "authorization"
            ]
        );
        let auth = &h[3].1;
        assert!(auth.starts_with(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/eu-west-2/bedrock/aws4_request, \
             SignedHeaders=content-type;host;x-amz-date;x-amz-security-token, Signature="
        ));
        assert!(!auth.contains(SECRET));
        let dbg = format!("{c:?}");
        assert!(
            !dbg.contains(SECRET) && !dbg.contains("AKID") && !dbg.contains("AQoD"),
            "{dbg}"
        );
    }
}
