//! Product mail templates (Vietnamese + English) with a tiny `{{var}}` renderer.
//!
//! HTML bodies are rendered with every value HTML-escaped; text bodies and subjects use raw
//! values (subjects additionally lose line breaks, so a name cannot inject headers). Values are
//! substituted in a single pass: a value containing `{{x}}` is never expanded again.

use crate::Email;

/// Mail language. Anything that is not Vietnamese renders in English.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    #[default]
    Vi,
    En,
}

impl Lang {
    /// From a BCP-47 tag such as `vi-VN` or `en-US`; `None` or unknown -> English only when it
    /// starts with `en`, otherwise the default (Vietnamese).
    pub fn from_tag(tag: Option<&str>) -> Self {
        match tag.map(|t| t.trim().to_ascii_lowercase()) {
            Some(t) if t == "en" || t.starts_with("en-") => Self::En,
            _ => Self::Vi,
        }
    }
}

/// Product name and the web app origin used for links (`APP_BASE_URL`).
#[derive(Debug, Clone)]
pub struct Brand {
    pub name: String,
    pub base_url: String,
}

impl Brand {
    pub fn new(name: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            base_url: base_url.into().trim_end_matches('/').to_owned(),
        }
    }

    /// `{base_url}{path}?{key}={percent-encoded value}`.
    pub fn link(&self, path: &str, key: &str, value: &str) -> String {
        format!("{}{path}?{key}={}", self.base_url, percent_encode(value))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub subject: String,
    pub html: String,
    pub text: String,
}

impl Rendered {
    /// Builds the [`Email`] for `to` with a provider tag naming the template.
    pub fn into_email(self, to: &str, kind: &str) -> Email {
        Email {
            to: to.to_owned(),
            subject: self.subject,
            html: self.html,
            text: self.text,
            tags: vec![("kind".to_owned(), kind.to_owned())],
        }
    }
}

pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Replaces `{{name}}` with the value of `name` (empty when unknown), optionally HTML-escaped.
pub fn render(template: &str, vars: &[(&str, &str)], escape: bool) -> String {
    let mut out = String::with_capacity(template.len() + 64);
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = after[..end].trim();
        let value = vars
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| *v)
            .unwrap_or("");
        if escape {
            out.push_str(&escape_html(value));
        } else {
            out.push_str(value);
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

fn one_line(s: &str) -> String {
    s.split(['\r', '\n']).collect::<Vec<_>>().join(" ")
}

const LAYOUT: &str = "<!doctype html><html><body style=\"margin:0;padding:24px;background:#f5f5f7;\
font-family:-apple-system,Segoe UI,Roboto,Arial,sans-serif;color:#1d1d1f\">\
<div style=\"max-width:520px;margin:0 auto;background:#fff;border-radius:12px;padding:32px\">\
<h2 style=\"margin:0 0 16px\">{{brand}}</h2>\
<p>{{greeting}}</p>\
<p>{{body}}</p>\
<p style=\"margin:24px 0\"><a href=\"{{link}}\" style=\"background:#0a6cff;color:#fff;\
padding:12px 20px;border-radius:8px;text-decoration:none\">{{cta}}</a></p>\
<p style=\"font-size:13px;color:#6e6e73\">{{link_hint}}<br>{{link}}</p>\
<p style=\"font-size:13px;color:#6e6e73\">{{footer}}</p>\
</div></body></html>";

struct Copy {
    subject: &'static str,
    greeting: &'static str,
    body: &'static str,
    cta: &'static str,
    link_hint: &'static str,
    footer: &'static str,
}

fn build(copy: &Copy, brand: &Brand, link: &str, extra: &[(&str, &str)]) -> Rendered {
    let mut vars: Vec<(&str, &str)> = vec![("brand", &brand.name), ("link", link)];
    vars.extend_from_slice(extra);
    // Copy strings may themselves contain `{{var}}`; expand them first (escaped for HTML), then
    // feed the results into the layout as already-final fragments.
    let body_html = render(copy.body, &vars, true);
    let footer_html = render(copy.footer, &vars, true);
    let greeting_html = render(copy.greeting, &vars, true);
    let cta_html = render(copy.cta, &vars, true);
    let hint_html = render(copy.link_hint, &vars, true);
    // Fragments are final HTML already; the layout pass must not escape or re-expand them.
    let brand_html = escape_html(&brand.name);
    let link_html = escape_html(link);
    let html = render(
        LAYOUT,
        &[
            ("brand", &brand_html),
            ("greeting", &greeting_html),
            ("body", &body_html),
            ("cta", &cta_html),
            ("link_hint", &hint_html),
            ("footer", &footer_html),
            ("link", &link_html),
        ],
        false,
    );
    let text = format!(
        "{}\n\n{}\n\n{}: {}\n\n{}",
        render(copy.greeting, &vars, false),
        render(copy.body, &vars, false),
        render(copy.cta, &vars, false),
        link,
        render(copy.footer, &vars, false),
    );
    Rendered {
        subject: one_line(&render(copy.subject, &vars, false)),
        html,
        text,
    }
}

pub fn verify_email(brand: &Brand, lang: Lang, token: &str) -> Rendered {
    let link = brand.link("/verify-email", "token", token);
    let copy = match lang {
        Lang::Vi => Copy {
            subject: "Xác minh email {{brand}} của bạn",
            greeting: "Xin chào,",
            body: "Cảm ơn bạn đã đăng ký {{brand}}. Hãy xác minh địa chỉ email để kích hoạt tài khoản. Liên kết có hiệu lực trong 60 phút.",
            cta: "Xác minh email",
            link_hint: "Nếu nút không hoạt động, hãy mở liên kết này:",
            footer: "Nếu bạn không tạo tài khoản, hãy bỏ qua email này.",
        },
        Lang::En => Copy {
            subject: "Verify your {{brand}} email",
            greeting: "Hello,",
            body: "Thanks for signing up for {{brand}}. Verify your email address to activate your account. The link is valid for 60 minutes.",
            cta: "Verify email",
            link_hint: "If the button does not work, open this link:",
            footer: "If you did not create an account, you can ignore this email.",
        },
    };
    build(&copy, brand, &link, &[])
}

pub fn reset_password(brand: &Brand, lang: Lang, token: &str) -> Rendered {
    let link = brand.link("/reset-password", "token", token);
    let copy = match lang {
        Lang::Vi => Copy {
            subject: "Đặt lại mật khẩu {{brand}}",
            greeting: "Xin chào,",
            body: "Chúng tôi nhận được yêu cầu đặt lại mật khẩu cho tài khoản {{brand}} của bạn. Liên kết có hiệu lực trong 30 phút và chỉ dùng được một lần.",
            cta: "Đặt lại mật khẩu",
            link_hint: "Nếu nút không hoạt động, hãy mở liên kết này:",
            footer: "Nếu bạn không yêu cầu, hãy bỏ qua email này; mật khẩu của bạn không thay đổi.",
        },
        Lang::En => Copy {
            subject: "Reset your {{brand}} password",
            greeting: "Hello,",
            body: "We received a request to reset the password of your {{brand}} account. The link is valid for 30 minutes and can be used once.",
            cta: "Reset password",
            link_hint: "If the button does not work, open this link:",
            footer: "If you did not ask for this, ignore this email; your password stays unchanged.",
        },
    };
    build(&copy, brand, &link, &[])
}

pub struct InvitationInfo<'a> {
    pub workspace: &'a str,
    pub inviter: &'a str,
    pub role: &'a str,
    pub token: &'a str,
    /// Human-readable expiry, e.g. `2026-12-01`.
    pub expires: &'a str,
}

pub fn workspace_invitation(brand: &Brand, lang: Lang, i: &InvitationInfo<'_>) -> Rendered {
    let link = brand.link("/invitations/accept", "token", i.token);
    let copy = match lang {
        Lang::Vi => Copy {
            subject: "{{inviter}} mời bạn tham gia {{workspace}} trên {{brand}}",
            greeting: "Xin chào,",
            body: "{{inviter}} đã mời bạn tham gia không gian làm việc {{workspace}} với vai trò {{role}}. Lời mời hết hạn vào {{expires}}.",
            cta: "Chấp nhận lời mời",
            link_hint: "Nếu nút không hoạt động, hãy mở liên kết này:",
            footer: "Nếu bạn không quen người mời, hãy bỏ qua email này.",
        },
        Lang::En => Copy {
            subject: "{{inviter}} invited you to {{workspace}} on {{brand}}",
            greeting: "Hello,",
            body: "{{inviter}} invited you to join the workspace {{workspace}} as {{role}}. The invitation expires on {{expires}}.",
            cta: "Accept invitation",
            link_hint: "If the button does not work, open this link:",
            footer: "If you do not know the sender, you can ignore this email.",
        },
    };
    build(
        &copy,
        brand,
        &link,
        &[
            ("workspace", i.workspace),
            ("inviter", i.inviter),
            ("role", i.role),
            ("expires", i.expires),
        ],
    )
}

/// `link_path` is an app path such as `/meetings/{id}`; no secret is embedded.
pub fn summary_ready(brand: &Brand, lang: Lang, meeting_title: &str, link_path: &str) -> Rendered {
    let link = format!("{}{link_path}", brand.base_url);
    let copy = match lang {
        Lang::Vi => Copy {
            subject: "Bản tóm tắt cuộc họp \"{{title}}\" đã sẵn sàng",
            greeting: "Xin chào,",
            body: "Bản tóm tắt AI của cuộc họp {{title}} đã sẵn sàng để xem trên {{brand}}.",
            cta: "Xem tóm tắt",
            link_hint: "Nếu nút không hoạt động, hãy mở liên kết này:",
            footer: "Bạn nhận email này vì bạn tham gia cuộc họp.",
        },
        Lang::En => Copy {
            subject: "Your summary for \"{{title}}\" is ready",
            greeting: "Hello,",
            body: "The AI summary of the meeting {{title}} is ready to view on {{brand}}.",
            cta: "View summary",
            link_hint: "If the button does not work, open this link:",
            footer: "You received this because you attended the meeting.",
        },
    };
    build(&copy, brand, &link, &[("title", meeting_title)])
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn brand() -> Brand {
        Brand::new("WarpTalk", "https://app.example.test/")
    }

    #[test]
    fn render_substitutes_and_escapes_once() {
        let v = [("a", "<b>\"x\" & {{b}}"), ("b", "BAD")];
        assert_eq!(
            render("x {{a}} y {{ b }} z {{nope}}", &v, true),
            "x &lt;b&gt;&quot;x&quot; &amp; {{b}} y BAD z "
        );
        assert_eq!(render("{{a}}", &v, false), "<b>\"x\" & {{b}}");
        assert_eq!(render("open {{ x", &v, true), "open {{ x");
    }

    #[test]
    fn verify_template_has_link_and_both_languages() {
        for lang in [Lang::Vi, Lang::En] {
            let r = verify_email(&brand(), lang, "tok-en_1");
            assert!(
                r.html
                    .contains("https://app.example.test/verify-email?token=tok-en_1")
            );
            assert!(
                r.text
                    .contains("https://app.example.test/verify-email?token=tok-en_1")
            );
            assert!(r.subject.contains("WarpTalk"));
        }
        assert!(
            verify_email(&brand(), Lang::Vi, "t")
                .text
                .contains("Xác minh")
        );
        assert!(
            verify_email(&brand(), Lang::En, "t")
                .text
                .contains("Verify")
        );
    }

    #[test]
    fn reset_link_path_and_encoding() {
        let r = reset_password(&brand(), Lang::En, "a b&c");
        assert!(r.text.contains("/reset-password?token=a%20b%26c"));
        assert!(r.html.contains("30 minutes"));
    }

    #[test]
    fn invitation_escapes_html_and_strips_subject_newlines() {
        let r = workspace_invitation(
            &brand(),
            Lang::En,
            &InvitationInfo {
                workspace: "<script>alert(1)</script>",
                inviter: "Eve\r\nBcc: x@evil.test",
                role: "member",
                token: "tk",
                expires: "2026-12-01",
            },
        );
        assert!(!r.html.contains("<script>"));
        assert!(r.html.contains("&lt;script&gt;"));
        assert!(!r.subject.contains('\n') && !r.subject.contains('\r'));
        assert!(r.html.contains("/invitations/accept?token=tk"));
    }

    #[test]
    fn brand_is_escaped_in_html() {
        let b = Brand::new("A&B<i>", "https://x.test");
        let r = summary_ready(&b, Lang::Vi, "T<1>", "/meetings/1");
        assert!(r.html.contains("A&amp;B&lt;i&gt;"));
        assert!(!r.html.contains("<i>"));
        assert!(r.html.contains("T&lt;1&gt;"));
        assert!(r.text.contains("https://x.test/meetings/1"));
    }

    #[test]
    fn lang_from_tag() {
        assert_eq!(Lang::from_tag(Some("en-US")), Lang::En);
        assert_eq!(Lang::from_tag(Some("vi-VN")), Lang::Vi);
        assert_eq!(Lang::from_tag(None), Lang::Vi);
        assert_eq!(Lang::from_tag(Some("fr")), Lang::Vi);
    }
}
