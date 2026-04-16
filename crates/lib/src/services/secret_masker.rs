//! Secret masking for persistence boundaries.
//!
//! Redacts sensitive patterns (API keys, passwords, private keys)
//! from any string before it crosses a persistence boundary:
//! disk writes, LLM summarization prompts, transcript exports.
//!
//! This is defence-in-depth. Tool results, user messages, and file
//! contents can contain secrets that should never be written to
//! session files, compression summaries, or output-store dumps.
//!
//! Matched content is replaced with `[REDACTED:<kind>]`.

use regex::Regex;
use std::sync::LazyLock;

/// Ordered list of (kind, pattern). Order matters: specific providers
/// match before the generic key/value rule so the kind label is useful.
struct Pattern {
    kind: &'static str,
    re: Regex,
}

static PATTERNS: LazyLock<Vec<Pattern>> = LazyLock::new(|| {
    let specs: &[(&str, &str)] = &[
        // PEM private keys — multiline, match whole block.
        (
            "private_key",
            r"(?s)-----BEGIN (?:RSA |EC |DSA |OPENSSH |PGP )?PRIVATE KEY-----.*?-----END (?:RSA |EC |DSA |OPENSSH |PGP )?PRIVATE KEY-----",
        ),
        // AWS access key ID.
        ("aws_access_key", r"AKIA[0-9A-Z]{16}"),
        // GitHub personal access token.
        ("github_pat", r"ghp_[a-zA-Z0-9]{36}"),
        // GitHub fine-grained token.
        ("github_token", r"github_pat_[a-zA-Z0-9_]{50,}"),
        // Provider API keys (OpenAI, Anthropic etc.) — long opaque prefix.
        ("provider_api_key", r"sk-[a-zA-Z0-9_\-]{20,}"),
        // URL-embedded credentials: postgres://user:password@host, redis://:pw@host, etc.
        // The password char class excludes `@`, whitespace, `"`, `'`, and `\`
        // so the match stops at the URL boundary and never crosses a
        // surrounding string delimiter.
        (
            "url_credential",
            r#"(?i)\b(postgres(?:ql)?|mysql|mariadb|redis|rediss|mongodb(?:\+srv)?|amqp|amqps|mqtt|mqtts|smtp|smtps|sftp|ssh|ldap|ldaps|https?)://([a-zA-Z0-9._-]*):([^@\s"'\\]{3,})@"#,
        ),
        // Generic assignment, quoted form: api_key = "sekrit", password: 'hunter2hunter2'.
        // Requires matching open/close quotes so this pattern can never
        // consume a stray delimiter (e.g. a JSON string's closing quote).
        // An optional leading backslash on each quote matches JSON-escaped
        // forms like `api_key = \"sekrit\"` that appear when a string-
        // valued message is serialized into an outer JSON document.
        (
            "credential",
            r#"(?i)\b((?:[a-z][a-z0-9_-]*[_-])?(?:api[_-]?key|secret|password|passwd|token|auth)[a-z0-9_-]*)\s*[:=]\s*(?:\\?"[A-Za-z0-9_\-./+=]{8,}\\?"|\\?'[A-Za-z0-9_\-./+=]{8,}\\?')"#,
        ),
        // Generic assignment, unquoted form: api_key=sekrit, auth_token = sekrit.
        // The value char class excludes `"` and `'`, so matching stops at
        // any surrounding JSON/TOML/YAML string delimiter rather than
        // eating it.
        (
            "credential",
            r#"(?i)\b((?:[a-z][a-z0-9_-]*[_-])?(?:api[_-]?key|secret|password|passwd|token|auth)[a-z0-9_-]*)\s*[:=]\s*[A-Za-z0-9_\-./+=]{8,}"#,
        ),
    ];

    specs
        .iter()
        .map(|(kind, pat)| Pattern {
            kind,
            re: Regex::new(pat).expect("secret_masker pattern must compile"),
        })
        .collect()
});

/// Mask all known secret patterns in `input`.
///
/// Idempotent: running twice produces the same output (the replacement
/// `[REDACTED:<kind>]` contains no characters that match any pattern).
pub fn mask(input: &str) -> String {
    let mut out = input.to_string();
    for p in PATTERNS.iter() {
        match p.kind {
            // Keep the identifier visible but scrub the value.
            "credential" => {
                out =
                    p.re.replace_all(&out, "${1}=[REDACTED:credential]")
                        .into_owned();
            }
            // Preserve scheme and username so the URL shape survives
            // for debugging; only the password segment is masked.
            "url_credential" => {
                out =
                    p.re.replace_all(&out, "${1}://${2}:[REDACTED:url_credential]@")
                        .into_owned();
            }
            _ => {
                let replacement = format!("[REDACTED:{}]", p.kind);
                out = p.re.replace_all(&out, replacement.as_str()).into_owned();
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_aws_access_key() {
        let input = "aws key is AKIAIOSFODNN7EXAMPLE in the config";
        let out = mask(input);
        assert!(out.contains("[REDACTED:aws_access_key]"));
        assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn masks_github_pat() {
        let input = "token=ghp_abcdefghijklmnopqrstuvwxyz0123456789";
        let out = mask(input);
        assert!(!out.contains("ghp_abcdefghijklmnopqrstuvwxyz0123456789"));
        // The generic credential rule runs after the github_pat rule —
        // either label is acceptable so long as the secret is gone.
        assert!(out.contains("REDACTED"));
    }

    #[test]
    fn masks_openai_style_key() {
        let input = "OPENAI_API_KEY=sk-proj-abcdefghijklmnopqrstuvwxyz1234567890";
        let out = mask(input);
        assert!(!out.contains("sk-proj-abcdefghijklmnopqrstuvwxyz1234567890"));
        assert!(out.contains("REDACTED"));
    }

    #[test]
    fn masks_pem_private_key() {
        let input = "config:\n-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA\ngarbage\n-----END RSA PRIVATE KEY-----\nend";
        let out = mask(input);
        assert!(out.contains("[REDACTED:private_key]"));
        assert!(!out.contains("MIIEpAIBAAKCAQEA"));
        assert!(out.starts_with("config:"));
        assert!(out.trim_end().ends_with("end"));
    }

    #[test]
    fn masks_generic_credential_assignment() {
        let cases = [
            "api_key = \"abcd1234efgh5678\"",
            "API-KEY: s3cret_v@lue_12345",
            "password=hunter2hunter2",
            "auth_token = longlonglonglonglonglonglongvalue",
        ];
        for c in cases {
            let out = mask(c);
            assert!(
                out.contains("[REDACTED:credential]"),
                "failed to mask: {c} -> {out}",
            );
        }
    }

    #[test]
    fn preserves_non_secret_text() {
        let input = "the quick brown fox jumps over the lazy dog 42 times";
        let out = mask(input);
        assert_eq!(out, input);
    }

    #[test]
    fn does_not_mask_short_values() {
        // 4-char password should not trip the generic credential rule
        // (requires 8+ chars to avoid masking common identifiers).
        let input = "id = abcd";
        let out = mask(input);
        assert_eq!(out, input);
    }

    #[test]
    fn idempotent() {
        let input = "key=sk-abcdefghijklmnopqrstuvwxyz and AKIAIOSFODNN7EXAMPLE";
        let once = mask(input);
        let twice = mask(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn masks_multiple_secrets_in_one_string() {
        let input = "aws=AKIAIOSFODNN7EXAMPLE gh=ghp_abcdefghijklmnopqrstuvwxyz0123456789";
        let out = mask(input);
        assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(!out.contains("ghp_abcdefghijklmnopqrstuvwxyz0123456789"));
    }

    #[test]
    fn masks_single_quoted_credential() {
        let input = "api_key = 'hunter2hunter2'";
        let out = mask(input);
        assert!(!out.contains("hunter2hunter2"));
        assert!(out.contains("[REDACTED:credential]"));
    }

    #[test]
    fn masks_mixed_quoted_and_unquoted_in_one_input() {
        // Quoted alternative runs first; unquoted second. Both must fire.
        let input = r#"password = "firstsecretvalue1" and token=secondsecretvalue2"#;
        let out = mask(input);
        assert!(!out.contains("firstsecretvalue1"));
        assert!(!out.contains("secondsecretvalue2"));
        // Two distinct credential matches.
        assert_eq!(out.matches("[REDACTED:credential]").count(), 2);
    }

    #[test]
    fn does_not_consume_surrounding_json_quote() {
        // Regression test for the JSON corruption fix. An unquoted inner
        // secret inside a JSON string literal must not eat the closing
        // JSON quote — the result must remain structurally valid.
        let input = r#"{"text": "api_key=hunter2hunter2hunter2"}"#;
        let out = mask(input);
        assert!(out.contains("[REDACTED:credential]"));
        // The closing `"` before `}` must survive.
        assert!(out.ends_with(r#""}"#));
        // And the whole thing must still parse as JSON.
        serde_json::from_str::<serde_json::Value>(&out)
            .expect("masked JSON fragment must still parse");
    }

    #[test]
    fn does_not_mask_json_key_form() {
        // `"api_key": "value"` should not trigger the credential rule
        // because the JSON `"` separates the key from the `:`.
        let input = r#"{"api_key": "hunter2hunter2"}"#;
        let out = mask(input);
        // The value is still a secret-looking string, but the regex
        // can't tell a JSON field key from an assignment statement.
        // What we're asserting: the result is still valid JSON.
        // (Whether it's masked is a secondary question — the important
        // invariant is we don't corrupt structure.)
        serde_json::from_str::<serde_json::Value>(&out)
            .expect("masked JSON key form must still parse");
    }

    #[test]
    fn empty_input_does_not_panic() {
        assert_eq!(mask(""), "");
    }

    #[test]
    fn strengthened_idempotency_across_split_pattern() {
        // Run the masker multiple times on a cocktail of secret shapes
        // and confirm the result converges after the first pass.
        let input = concat!(
            "AKIAIOSFODNN7EXAMPLE ",
            "ghp_abcdefghijklmnopqrstuvwxyz0123456789 ",
            "sk-proj-abcdefghijklmnopqrstuvwxyz12345 ",
            r#"password = "firstsecretvalue1" "#,
            "token=secondsecretvalue2 ",
            "auth_token: thirdsecretvalue3",
        );
        let once = mask(input);
        let twice = mask(&once);
        let thrice = mask(&twice);
        assert_eq!(once, twice, "mask is not idempotent");
        assert_eq!(twice, thrice);
        // Confirm each shape hit something.
        assert!(!once.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(!once.contains("ghp_abcdefghijklmnopqrstuvwxyz0123456789"));
        assert!(!once.contains("firstsecretvalue1"));
        assert!(!once.contains("secondsecretvalue2"));
        assert!(!once.contains("thirdsecretvalue3"));
    }

    #[test]
    fn masks_url_embedded_password_postgres() {
        let input = "postgres://admin:hunter2hunter2@db.internal:5432/prod";
        let out = mask(input);
        assert!(!out.contains("hunter2hunter2"));
        assert!(out.contains("[REDACTED:url_credential]"));
        // Scheme and username must survive for debugging.
        assert!(out.starts_with("postgres://admin:"));
        assert!(out.contains("@db.internal:5432/prod"));
    }

    #[test]
    fn masks_url_embedded_password_redis_without_user() {
        let input = "redis://:supersecretvalue@cache.local:6379";
        let out = mask(input);
        assert!(!out.contains("supersecretvalue"));
        assert!(out.contains("[REDACTED:url_credential]"));
    }

    #[test]
    fn masks_url_embedded_password_inside_json_escape() {
        // Tool result that's been embedded into a JSON string — quotes
        // around the URL are escaped. The URL regex must stop at the
        // escaped closing quote, not eat past it.
        let input = r#"{"text": "DATABASE_URL=postgres://user:hunter2hunter2@host/db"}"#;
        let out = mask(input);
        assert!(!out.contains("hunter2hunter2"));
        // Result must remain valid JSON (closing `"` and `}` intact).
        serde_json::from_str::<serde_json::Value>(&out)
            .expect("masked URL-credential JSON must still parse");
    }

    #[test]
    fn does_not_mask_url_without_password() {
        let input = "visit https://example.com/path for docs";
        let out = mask(input);
        assert_eq!(out, input);
    }

    #[test]
    fn masks_uppercase_env_var_style() {
        let input = "API_KEY=verylongprovidersecretvalue123";
        let out = mask(input);
        assert!(!out.contains("verylongprovidersecretvalue123"));
        assert!(out.contains("REDACTED"));
    }

    #[test]
    fn allows_legitimate_code_through() {
        let code = r#"
            fn add(a: i32, b: i32) -> i32 { a + b }
            let x = vec![1, 2, 3];
            println!("{}", x.len());
        "#;
        let out = mask(code);
        assert_eq!(out, code);
    }
}
