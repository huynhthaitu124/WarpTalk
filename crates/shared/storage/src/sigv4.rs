//! AWS Signature Version 4 (header and query-string/presigned flavours), just enough for
//! S3-compatible PUT / GET / DELETE. Pure functions; the clock is a parameter (`amz_date`,
//! `YYYYMMDDTHHMMSSZ`) so the algorithm is testable against the published AWS examples.

use std::fmt::Write;

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

pub const ALGORITHM: &str = "AWS4-HMAC-SHA256";
pub const UNSIGNED_PAYLOAD: &str = "UNSIGNED-PAYLOAD";

#[derive(Debug, Clone, Copy)]
pub struct Credentials<'a> {
    pub access_key_id: &'a str,
    pub secret_access_key: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct Scope<'a> {
    pub region: &'a str,
    pub service: &'a str,
}

pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    // HMAC accepts keys of any length, so construction cannot fail.
    match Hmac::<Sha256>::new_from_slice(key) {
        Ok(mut mac) => {
            mac.update(data);
            mac.finalize().into_bytes().to_vec()
        }
        Err(_) => Vec::new(),
    }
}

/// Percent-encodes everything except unreserved characters (`A-Za-z0-9-_.~`); `/` is kept when
/// `keep_slash` (path encoding).
pub fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b'/' if keep_slash => out.push('/'),
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

/// Canonical query string: names and values encoded, sorted by name then value.
pub fn canonical_query(params: &[(&str, &str)]) -> String {
    let mut enc: Vec<(String, String)> = params
        .iter()
        .map(|(k, v)| (uri_encode(k, false), uri_encode(v, false)))
        .collect();
    enc.sort();
    enc.iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// Returns `(canonical request, signed headers)`. Header names are lower-cased, values trimmed
/// with inner whitespace runs collapsed.
pub fn canonical_request(
    method: &str,
    canonical_uri: &str,
    canonical_query: &str,
    headers: &[(&str, &str)],
    payload_hash: &str,
) -> (String, String) {
    let mut hs: Vec<(String, String)> = headers
        .iter()
        .map(|(k, v)| {
            (
                k.to_ascii_lowercase(),
                v.split_whitespace().collect::<Vec<_>>().join(" "),
            )
        })
        .collect();
    hs.sort();
    let canonical_headers: String = hs.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let signed = hs
        .iter()
        .map(|(k, _)| k.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let req = format!(
        "{method}\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\n{signed}\n{payload_hash}"
    );
    (req, signed)
}

fn credential_scope(amz_date: &str, scope: Scope<'_>) -> String {
    let date = amz_date.get(..8).unwrap_or(amz_date);
    format!("{date}/{}/{}/aws4_request", scope.region, scope.service)
}

/// Hex signature of a canonical request.
pub fn signature(
    canonical_request: &str,
    amz_date: &str,
    creds: Credentials<'_>,
    scope: Scope<'_>,
) -> String {
    let date = amz_date.get(..8).unwrap_or(amz_date);
    let string_to_sign = format!(
        "{ALGORITHM}\n{amz_date}\n{}\n{}",
        credential_scope(amz_date, scope),
        sha256_hex(canonical_request.as_bytes())
    );
    let k_date = hmac_sha256(
        format!("AWS4{}", creds.secret_access_key).as_bytes(),
        date.as_bytes(),
    );
    let k_region = hmac_sha256(&k_date, scope.region.as_bytes());
    let k_service = hmac_sha256(&k_region, scope.service.as_bytes());
    let k_signing = hmac_sha256(&k_service, b"aws4_request");
    hex(&hmac_sha256(&k_signing, string_to_sign.as_bytes()))
}

/// Value of the `Authorization` header for a request whose `headers` (including `host`,
/// `x-amz-date` and `x-amz-content-sha256`) are all signed.
#[allow(clippy::too_many_arguments)]
pub fn authorization_header(
    method: &str,
    canonical_uri: &str,
    canonical_query: &str,
    headers: &[(&str, &str)],
    payload_hash: &str,
    amz_date: &str,
    creds: Credentials<'_>,
    scope: Scope<'_>,
) -> String {
    let (req, signed) = canonical_request(
        method,
        canonical_uri,
        canonical_query,
        headers,
        payload_hash,
    );
    let sig = signature(&req, amz_date, creds, scope);
    format!(
        "{ALGORITHM} Credential={}/{}, SignedHeaders={signed}, Signature={sig}",
        creds.access_key_id,
        credential_scope(amz_date, scope),
    )
}

/// Query string (without leading `?`) of a presigned URL, signature included. Only the `host`
/// header is signed; the payload is unsigned.
pub fn presigned_query(
    method: &str,
    canonical_uri: &str,
    host: &str,
    amz_date: &str,
    expires_secs: u64,
    creds: Credentials<'_>,
    scope: Scope<'_>,
) -> String {
    let credential = format!(
        "{}/{}",
        creds.access_key_id,
        credential_scope(amz_date, scope)
    );
    let expires = expires_secs.to_string();
    let params = [
        ("X-Amz-Algorithm", ALGORITHM),
        ("X-Amz-Credential", credential.as_str()),
        ("X-Amz-Date", amz_date),
        ("X-Amz-Expires", expires.as_str()),
        ("X-Amz-SignedHeaders", "host"),
    ];
    let query = canonical_query(&params);
    let (req, _) = canonical_request(
        method,
        canonical_uri,
        &query,
        &[("host", host)],
        UNSIGNED_PAYLOAD,
    );
    let sig = signature(&req, amz_date, creds, scope);
    format!("{query}&X-Amz-Signature={sig}")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    // Examples from "Signature Calculations for the Authorization Header" and "Authenticating
    // Requests: Using Query Parameters" in the Amazon S3 API reference.
    const CREDS: Credentials<'static> = Credentials {
        access_key_id: "AKIAIOSFODNN7EXAMPLE",
        secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
    };
    const SCOPE: Scope<'static> = Scope {
        region: "us-east-1",
        service: "s3",
    };
    const EMPTY_SHA: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn empty_payload_hash() {
        assert_eq!(sha256_hex(b""), EMPTY_SHA);
    }

    #[test]
    fn aws_get_object_with_range() {
        let headers = [
            ("Host", "examplebucket.s3.amazonaws.com"),
            ("Range", "bytes=0-9"),
            ("x-amz-content-sha256", EMPTY_SHA),
            ("x-amz-date", "20130524T000000Z"),
        ];
        let (req, signed) = canonical_request("GET", "/test.txt", "", &headers, EMPTY_SHA);
        assert_eq!(signed, "host;range;x-amz-content-sha256;x-amz-date");
        assert_eq!(
            sha256_hex(req.as_bytes()),
            "7344ae5b7ee6c3e7e6b0fe0640412a37625d1fbfff95c48bbb2dc43964946972"
        );
        let auth = authorization_header(
            "GET",
            "/test.txt",
            "",
            &headers,
            EMPTY_SHA,
            "20130524T000000Z",
            CREDS,
            SCOPE,
        );
        assert_eq!(
            auth,
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, \
             SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, \
             Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    #[test]
    fn aws_put_object() {
        let payload = sha256_hex(b"Welcome to Amazon S3.");
        assert_eq!(
            payload,
            "44ce7dd67c959e0d3524ffac1771dfbba87d2b6b4b4e99e42034a8b803f8b072"
        );
        let headers = [
            ("Date", "Fri, 24 May 2013 00:00:00 GMT"),
            ("Host", "examplebucket.s3.amazonaws.com"),
            ("x-amz-date", "20130524T000000Z"),
            ("x-amz-storage-class", "REDUCED_REDUNDANCY"),
            ("x-amz-content-sha256", payload.as_str()),
        ];
        let uri = format!("/{}", uri_encode("test$file.text", true));
        assert_eq!(uri, "/test%24file.text");
        let auth = authorization_header(
            "PUT",
            &uri,
            "",
            &headers,
            &payload,
            "20130524T000000Z",
            CREDS,
            SCOPE,
        );
        assert!(
            auth.ends_with(
                "Signature=98ad721746da40c64f1a55b78f14c238d841ea1380cd77a1b5971af0ece108bd"
            ),
            "{auth}"
        );
    }

    #[test]
    fn aws_presigned_get() {
        let q = presigned_query(
            "GET",
            "/test.txt",
            "examplebucket.s3.amazonaws.com",
            "20130524T000000Z",
            86400,
            CREDS,
            SCOPE,
        );
        assert_eq!(
            q,
            "X-Amz-Algorithm=AWS4-HMAC-SHA256\
             &X-Amz-Credential=AKIAIOSFODNN7EXAMPLE%2F20130524%2Fus-east-1%2Fs3%2Faws4_request\
             &X-Amz-Date=20130524T000000Z&X-Amz-Expires=86400&X-Amz-SignedHeaders=host\
             &X-Amz-Signature=aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404"
        );
    }

    #[test]
    fn uri_encoding() {
        assert_eq!(uri_encode("a b/c~d", true), "a%20b/c~d");
        assert_eq!(uri_encode("a/b", false), "a%2Fb");
        assert_eq!(canonical_query(&[("b", "2"), ("a", "1 2")]), "a=1%202&b=2");
    }
}
