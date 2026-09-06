//! `docs/api.md` §3 — the providers' own incidents, as a table.

use super::table;
use serde_json::Value;

/// `docs/api.md` §3 — `incidents`, rendered.
///
/// One row per open incident, worst first as the daemon orders them; then a
/// line per provider whose page did not answer, and what was asked. No open
/// incident is a sentence naming the providers asked, so "nothing" is never
/// mistaken for "nobody was asked".
#[must_use]
pub fn incidents(result: &Value) -> String {
    let mut lines = Vec::new();
    let rows: Vec<Vec<String>> = result
        .get("incidents")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    let text = |key: &str| {
                        row.get(key)
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned()
                    };
                    vec![
                        text("provider"),
                        text("impact"),
                        text("status"),
                        text("since"),
                        text("name"),
                        text("url"),
                    ]
                })
                .collect()
        })
        .unwrap_or_default();
    let providers: Vec<&str> = result
        .get("providers")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if rows.is_empty() {
        if providers.is_empty() {
            lines.push("no provider asked: no stored account names one".to_owned());
        } else {
            lines.push(format!("no incident open at {}", providers.join(", ")));
        }
    } else {
        lines.push(table(
            &["PROVIDER", "IMPACT", "STATUS", "SINCE", "NAME", "URL"],
            &rows,
        ));
    }
    if let Some(errors) = result.get("errors").and_then(Value::as_object) {
        for (provider, error) in errors {
            lines.push(format!(
                "{provider}: its status page did not answer: {}",
                error.as_str().unwrap_or("")
            ));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn nothing_open_names_who_was_asked_and_a_page_that_failed() {
        let text = incidents(&json!({
            "incidents": [],
            "providers": ["anthropic", "codex"],
            "errors": {"codex": "HTTP 503"},
            "checked_at": 1,
        }));
        assert_eq!(
            text,
            "no incident open at anthropic, codex\ncodex: its status page did not answer: HTTP 503"
        );
        assert_eq!(
            incidents(&json!({"incidents": [], "providers": [], "errors": {}})),
            "no provider asked: no stored account names one"
        );
    }

    #[test]
    fn an_open_incident_is_a_row() {
        let text = incidents(&json!({
            "incidents": [{"provider": "anthropic", "impact": "major", "status": "monitoring",
                           "since": "2026-09-06T01:00:00Z", "name": "Elevated errors", "url": "https://stspg.io/a1"}],
            "providers": ["anthropic"],
            "errors": {},
        }));
        assert!(text.starts_with("PROVIDER"), "{text}");
        assert!(text.contains("anthropic"));
        assert!(text.contains("Elevated errors"));
        assert!(text.contains("https://stspg.io/a1"));
    }
}
