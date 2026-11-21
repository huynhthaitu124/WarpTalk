//! Versioned prompt templates with `{{variable}}` substitution.
//!
//! Defaults are embedded from `prompts/*.md`; `SuggestionConfig::prompt_templates` (versioned) and
//! `SuggestionConfig::prompts` (text only, base-compatible) override them by name.
//! Substitution is single-pass: substituted values are never re-scanned, so transcript text
//! containing `{{...}}` cannot inject further variables.

use std::collections::BTreeMap;

use super::config::SuggestionConfig;

pub const TEMPLATE_NAMES: [&str; 8] = [
    "decide",
    "decide_user",
    "generate",
    "generate_user",
    "untrusted_rule",
    "translations_rule",
    "general_knowledge_rule",
    "follow_up_rule",
];

pub const KNOWN_VARS: [&str; 17] = [
    "untrusted_rule",
    "category_descriptions",
    "category_ids",
    "category",
    "contract",
    "max_chars",
    "language",
    "general_knowledge_rule",
    "follow_up_rule",
    "extra_fields",
    "translations_rule",
    "wanted_languages",
    "transcript",
    "reason",
    "documents",
    "reader_languages",
    "segment_language",
];

pub const BUILTIN_VERSION: &str = "builtin-1";

fn builtin(name: &str) -> &'static str {
    match name {
        "decide" => include_str!("prompts/decide.md"),
        "decide_user" => include_str!("prompts/decide_user.md"),
        "generate" => include_str!("prompts/generate.md"),
        "generate_user" => include_str!("prompts/generate_user.md"),
        "untrusted_rule" => include_str!("prompts/untrusted_rule.md"),
        "translations_rule" => include_str!("prompts/translations_rule.md"),
        "general_knowledge_rule" => include_str!("prompts/general_knowledge_rule.md"),
        "follow_up_rule" => include_str!("prompts/follow_up_rule.md"),
        _ => "",
    }
}

/// A template text together with the version that identifies it in traces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pub version: String,
    pub text: String,
}

/// Resolve a template by name: `prompt_templates` > `prompts` > embedded default.
pub fn resolve(cfg: &SuggestionConfig, name: &str) -> Template {
    if let Some(t) = cfg.prompt_templates.get(name) {
        return Template {
            version: t.version.clone(),
            text: t.text.clone(),
        };
    }
    if let Some(t) = cfg.prompts.get(name)
        && !t.trim().is_empty()
    {
        return Template {
            version: "override".into(),
            text: t.clone(),
        };
    }
    let text = builtin(name).trim_end_matches('\n').to_string();
    Template {
        version: BUILTIN_VERSION.into(),
        text,
    }
}

/// Names of `{{variables}}` used in `template`.
pub fn variables_in(template: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        let name = after[..end].trim();
        if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            out.push(name.to_string());
        }
        rest = &after[end + 2..];
    }
    out
}

/// Single-pass substitution. Unknown variables render as the empty string.
pub fn render(template: &str, vars: &BTreeMap<&str, String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let name = after[..end].trim();
                let is_var =
                    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                if is_var {
                    if let Some(v) = vars.get(name) {
                        out.push_str(v);
                    }
                } else {
                    out.push_str("{{");
                    out.push_str(&after[..end]);
                    out.push_str("}}");
                }
                rest = &after[end + 2..];
            }
            None => {
                out.push_str("{{");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_template_exists_and_uses_known_vars() {
        for name in TEMPLATE_NAMES {
            let t = resolve(&SuggestionConfig::default(), name);
            assert!(!t.text.trim().is_empty(), "{name} is empty");
            for v in variables_in(&t.text) {
                assert!(KNOWN_VARS.contains(&v.as_str()), "{name} uses {v}");
            }
        }
    }

    #[test]
    fn render_is_single_pass() {
        let mut vars = BTreeMap::new();
        vars.insert("a", "{{b}}".to_string());
        vars.insert("b", "BAD".to_string());
        assert_eq!(
            render("x {{a}} y {{ b }} {{nope}}!", &vars),
            "x {{b}} y BAD !"
        );
        assert_eq!(render("json {\"k\": 1} {{", &vars), "json {\"k\": 1} {{");
    }

    #[test]
    fn override_precedence() {
        let mut cfg = SuggestionConfig::default();
        cfg.prompts.insert("decide".into(), "from prompts".into());
        assert_eq!(resolve(&cfg, "decide").version, "override");
        cfg.prompt_templates.insert(
            "decide".into(),
            super::super::config::PromptTemplate {
                version: "v7".into(),
                text: "from templates".into(),
            },
        );
        let t = resolve(&cfg, "decide");
        assert_eq!(
            (t.version.as_str(), t.text.as_str()),
            ("v7", "from templates")
        );
    }
}
