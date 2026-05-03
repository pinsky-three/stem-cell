/// Small redaction helper for subprocess errors and structured logs.
///
/// Callers add exact token strings or prefixes they know are sensitive. The
/// runner applies the redactor before returning stderr/stdout in errors.
#[derive(Debug, Clone, Default)]
pub struct Redactor {
    secrets: Vec<String>,
}

impl Redactor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_secret(mut self, secret: impl Into<String>) -> Self {
        self.add_secret(secret);
        self
    }

    pub fn add_secret(&mut self, secret: impl Into<String>) {
        let secret = secret.into();
        if !secret.is_empty() {
            self.secrets.push(secret);
        }
    }

    pub fn redact(&self, input: &str) -> String {
        let mut out = input.to_string();
        for secret in &self.secrets {
            out = out.replace(secret, "<redacted>");
        }
        redact_tokenized_urls(&out)
    }
}

pub fn redact_tokenized_urls(input: &str) -> String {
    let marker = "x-access-token:";
    let mut out = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(start) = rest.find(marker) {
        let token_start = start + marker.len();
        out.push_str(&rest[..token_start]);

        let Some(at_rel) = rest[token_start..].find('@') else {
            out.push_str(&rest[token_start..]);
            return out;
        };

        out.push_str("<redacted>");
        let at = token_start + at_rel;
        rest = &rest[at..];
    }

    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_exact_secret_and_token_url() {
        let r = Redactor::new().with_secret("secret-token");
        let s = r.redact("secret-token https://x-access-token:abc123@github.com/acme/repo.git");
        assert!(!s.contains("secret-token"));
        assert!(!s.contains("abc123"));
        assert!(s.contains("<redacted>"));
    }
}
