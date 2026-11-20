//! Input validation (BR-07). All functions return a client-safe message on failure.

pub fn normalize_email(raw: &str) -> Result<String, &'static str> {
    let e = raw.trim().to_lowercase();
    let ok = e.len() <= 320
        && !e.chars().any(|c| c.is_whitespace() || c.is_control())
        && e.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && !domain.contains('@')
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        });
    if ok { Ok(e) } else { Err("invalid email address") }
}

pub fn full_name(raw: &str) -> Result<String, &'static str> {
    let n = raw.trim();
    if n.is_empty() || n.chars().count() > 150 {
        Err("fullName must be 1-150 characters")
    } else {
        Ok(n.to_owned())
    }
}

/// Basic BCP-47 shape: `lang(-subtag)*`, e.g. `vi-VN`, `en`, `zh-Hant-TW`.
pub fn language_tag(raw: &str) -> Result<String, &'static str> {
    let t = raw.trim();
    let mut parts = t.split('-');
    let lang_ok = parts
        .next()
        .is_some_and(|l| (2..=3).contains(&l.len()) && l.chars().all(|c| c.is_ascii_alphabetic()));
    let rest_ok = parts.all(|p| (1..=8).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphanumeric()));
    if t.len() <= 15 && lang_ok && rest_ok {
        Ok(t.to_owned())
    } else {
        Err("invalid language tag (expected BCP-47, e.g. vi-VN)")
    }
}

/// `UTC` or an IANA-shaped identifier containing `/` (e.g. `Asia/Ho_Chi_Minh`).
pub fn timezone(raw: &str) -> Result<String, &'static str> {
    let t = raw.trim();
    let shape = t.len() <= 50
        && t.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '+'))
        && !t.starts_with('/')
        && !t.ends_with('/');
    if shape && (t == "UTC" || t.contains('/')) {
        Ok(t.to_owned())
    } else {
        Err("invalid timezone (expected UTC or an IANA id such as Asia/Ho_Chi_Minh)")
    }
}

pub fn avatar_url(raw: &str) -> Result<String, &'static str> {
    let u = raw.trim();
    if u.len() <= 500 && (u.starts_with("https://") || u.starts_with("http://")) {
        Ok(u.to_owned())
    } else {
        Err("avatarUrl must be an http(s) URL up to 500 characters")
    }
}

pub fn device_name(raw: &str) -> Result<String, &'static str> {
    let d = raw.trim();
    if d.len() <= 255 {
        Ok(d.to_owned())
    } else {
        Err("value too long (max 255 characters)")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn email() {
        assert_eq!(normalize_email("  A@B.Com ").unwrap(), "a@b.com");
        for bad in ["", "a", "a@b", "@b.com", "a@@b.com", "a b@c.com", "a@.com"] {
            assert!(normalize_email(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn language() {
        for ok in ["vi-VN", "en", "zh-Hant-TW", "fil"] {
            assert!(language_tag(ok).is_ok(), "{ok}");
        }
        for bad in ["", "v", "vi_VN", "vi-", "english-US", "vi-VN-1234567890"] {
            assert!(language_tag(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn tz() {
        for ok in ["UTC", "Asia/Ho_Chi_Minh", "America/Argentina/Buenos_Aires"] {
            assert!(timezone(ok).is_ok(), "{ok}");
        }
        for bad in ["", "utc", "Mars", "/Asia", "Asia/ Tokyo"] {
            assert!(timezone(bad).is_err(), "{bad}");
        }
    }
}
