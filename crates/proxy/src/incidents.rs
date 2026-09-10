//! `docs/api.md` §3 — what each provider says about itself.
//!
//! A provider that is having an incident says so on its status page, and an
//! operator seeing every turn fail with a 529 wants that sentence beside the
//! quota before anything else: it is the difference between "the fleet broke"
//! and "wait". The page is public and account-free, so this asks it for every
//! provider a stored account is on and for none other, and keeps only what is
//! open. Nothing here is computed: a row is the provider's own words.
//!
//! A row carries the updates posted on it as well, because the document they
//! are read from carries them beside it. A front-end asking whether an
//! incident is moving then has the sentence without fetching a provider's
//! page itself, which is a second reader of a document this already polls and
//! a second allowlist of hosts to be wrong about.
//!
//! Two pages, two shapes. Claude's is Statuspage, whose unresolved list is the
//! one to read — its global indicator goes green while a major incident is
//! still being monitored. OpenAI's is Statuspage-shaped without an unresolved
//! feed, so its summary is read, and the closed rows it may carry are dropped.

use crate::auth::store::Provider;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

/// How often the pages are asked. An incident is opened and updated on the
/// scale of minutes; Claude's feed is CDN-cached at ten seconds.
pub const POLL: Duration = Duration::from_secs(60);
const FETCH_TIMEOUT: Duration = Duration::from_secs(8);

/// One update posted on an incident, in the provider's words.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Update {
    /// The state that update announced: `investigating`, `identified`,
    /// `monitoring`, `resolved`.
    pub status: String,
    /// What was posted, as the page states it.
    pub body: String,
    /// When it was posted, as the page states it.
    pub at: String,
}

/// One open incident, in the provider's words.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Incident {
    pub id: String,
    /// `anthropic` or `codex`, the name the store gives the provider.
    pub provider: &'static str,
    pub name: String,
    /// `investigating`, `identified`, `monitoring`; never a closed word.
    pub status: String,
    /// `none`, `minor`, `major`, `critical`, as the page states it.
    pub impact: String,
    /// The page's own link to the incident.
    pub url: String,
    /// When it was opened, as the page states it.
    pub since: Option<String>,
    /// What has been posted about it since it opened, newest first. The
    /// document this is read from carries them beside the row, so a reader
    /// asking whether an incident is moving needs no second request and no
    /// second allowlist of status hosts. Present and empty rather than
    /// absent: a caller reads its absence as a daemon older than the field
    /// (§12) rather than as an incident nothing has been said about.
    pub updates: Vec<Update>,
}

/// The open incidents in a Statuspage-shaped document: an `incidents` array
/// whose rows carry `id`, `name`, `status`, `impact`, and either a
/// `shortlink` or nothing, in which case the page's incident path is made.
/// A row already `resolved` or in `postmortem` is not open and is dropped.
#[must_use]
pub fn parse(provider: Provider, body: &str) -> Vec<Incident> {
    let Ok(document) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let Some(rows) = document.get("incidents").and_then(Value::as_array) else {
        return Vec::new();
    };
    let page = match provider {
        Provider::Anthropic => "https://status.claude.com",
        Provider::Codex => "https://status.openai.com",
    };
    rows.iter()
        .filter_map(|row| {
            let id = row.get("id")?.as_str()?;
            let name = row.get("name")?.as_str()?;
            let status = row.get("status").and_then(Value::as_str).unwrap_or("");
            if matches!(status, "resolved" | "postmortem") {
                return None;
            }
            let url = row
                .get("shortlink")
                .and_then(Value::as_str)
                .filter(|link| !link.is_empty())
                .map_or_else(|| format!("{page}/incidents/{id}"), str::to_owned);
            Some(Incident {
                id: id.to_owned(),
                provider: provider.as_str(),
                name: name.to_owned(),
                status: status.to_owned(),
                impact: row
                    .get("impact")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                url,
                since: ["started_at", "created_at"]
                    .iter()
                    .find_map(|key| row.get(key).and_then(Value::as_str))
                    .map(str::to_owned),
                updates: updates_of(row),
            })
        })
        .collect()
}

/// The updates posted on one incident row, newest first. A row with neither
/// a body nor a status says nothing and is dropped. The stamps are ISO-8601
/// in UTC, which sorts as text.
fn updates_of(row: &Value) -> Vec<Update> {
    let Some(posted) = row.get("incident_updates").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out: Vec<Update> = posted
        .iter()
        .map(|update| {
            let text = |key: &str| {
                update
                    .get(key)
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned()
            };
            Update {
                status: text("status"),
                body: text("body"),
                at: ["display_at", "created_at"]
                    .iter()
                    .find_map(|key| update.get(key).and_then(Value::as_str))
                    .unwrap_or("")
                    .to_owned(),
            }
        })
        .filter(|update| !update.body.is_empty() || !update.status.is_empty())
        .collect();
    out.sort_by(|a, b| b.at.cmp(&a.at));
    out
}

/// Where a provider states its status.
#[must_use]
pub fn status_url(provider: Provider) -> &'static str {
    match provider {
        Provider::Anthropic => "https://status.claude.com/api/v2/incidents/unresolved.json",
        Provider::Codex => "https://status.openai.com/api/v2/summary.json",
    }
}

#[derive(Debug, Default, Clone)]
struct Reading {
    /// Per provider, the open incidents as of its last successful read.
    open: BTreeMap<&'static str, Vec<Incident>>,
    /// Per provider, why the last read failed, where it did. A page that
    /// cannot be reached keeps its last list and says so beside it.
    errors: BTreeMap<&'static str, String>,
    /// Epoch seconds of the last round, or `None` before the first.
    checked_at: Option<u64>,
}

/// What the pages last said, shared between the poller and the socket.
#[derive(Debug, Default)]
pub struct IncidentStore {
    reading: Mutex<Reading>,
}

impl IncidentStore {
    /// Record one provider's read: its open list, or why there is none.
    pub fn record(&self, provider: Provider, result: Result<Vec<Incident>, String>) {
        let mut reading = self
            .reading
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match result {
            Ok(open) => {
                reading.open.insert(provider.as_str(), open);
                reading.errors.remove(provider.as_str());
            }
            Err(error) => {
                reading.errors.insert(provider.as_str(), error);
            }
        }
    }

    /// Forget a provider no stored account is on any more.
    pub fn forget_except(&self, providers: &[Provider]) {
        let keep: Vec<&str> = providers.iter().map(|p| p.as_str()).collect();
        let mut reading = self
            .reading
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reading.open.retain(|name, _| keep.contains(name));
        reading.errors.retain(|name, _| keep.contains(name));
    }

    /// Mark the end of a round.
    pub fn checked(&self, now: u64) {
        let mut reading = self
            .reading
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reading.checked_at = Some(now);
    }

    /// The open incidents across every provider asked, most severe first.
    #[must_use]
    pub fn open(&self) -> Vec<Incident> {
        let reading = self
            .reading
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        open_of(&reading)
    }

    /// The §3 `incidents` answer: `incidents`, `providers` — the ones asked
    /// — `errors` per provider whose page did not answer, and `checked_at`.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let reading = self
            .reading
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut providers: Vec<&str> = reading.open.keys().copied().collect();
        for name in reading.errors.keys() {
            if !providers.contains(name) {
                providers.push(name);
            }
        }
        json!({
            "incidents": open_of(&reading),
            "providers": providers,
            "errors": reading.errors,
            "checked_at": reading.checked_at,
        })
    }
}

fn open_of(reading: &Reading) -> Vec<Incident> {
    let mut all: Vec<Incident> = reading.open.values().flatten().cloned().collect();
    all.sort_by_key(|i| std::cmp::Reverse(impact_rank(&i.impact)));
    all
}

fn impact_rank(impact: &str) -> u8 {
    match impact {
        "critical" => 3,
        "major" => 2,
        "minor" => 1,
        _ => 0,
    }
}

/// One round: ask each provider's page and record what it said.
pub async fn poll_once(
    store: &IncidentStore,
    providers: &[Provider],
    client: &reqwest::Client,
    url_for: impl Fn(Provider) -> String,
) {
    store.forget_except(providers);
    for &provider in providers {
        let url = url_for(provider);
        let result = async {
            let response = client
                .get(&url)
                .timeout(FETCH_TIMEOUT)
                .send()
                .await
                .map_err(|e| format!("{url}: {e}"))?;
            if !response.status().is_success() {
                return Err(format!("{url}: HTTP {}", response.status().as_u16()));
            }
            let body = response.text().await.map_err(|e| format!("{url}: {e}"))?;
            Ok(parse(provider, &body))
        }
        .await;
        store.record(provider, result);
    }
    store.checked(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    );
}

/// The providers whose pages are worth asking: one per provider a stored
/// account is on, in store order, each once.
#[must_use]
pub fn providers_of(accounts: &[crate::auth::store::Account]) -> Vec<Provider> {
    let mut out = Vec::new();
    for account in accounts {
        let provider = match account.provider {
            "anthropic" => Provider::Anthropic,
            "codex" => Provider::Codex,
            _ => continue,
        };
        if !out.contains(&provider) {
            out.push(provider);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_open_rows_and_makes_a_link_where_the_page_gives_none() {
        let body = r#"{"incidents":[
          {"id":"a1","name":"Elevated errors on Claude API","status":"monitoring","impact":"major","shortlink":"https://stspg.io/a1","started_at":"2026-09-06T01:00:00Z"},
          {"id":"a2","name":"Old one","status":"resolved","impact":"minor"},
          {"id":"a3","name":"Login degraded","status":"investigating","impact":"minor","created_at":"2026-09-06T02:00:00Z"}
        ]}"#;
        let open = parse(Provider::Anthropic, body);
        assert_eq!(open.len(), 2);
        assert_eq!(open[0].url, "https://stspg.io/a1");
        assert_eq!(open[0].since.as_deref(), Some("2026-09-06T01:00:00Z"));
        assert_eq!(open[1].url, "https://status.claude.com/incidents/a3");
        assert_eq!(open[1].provider, "anthropic");
        assert!(parse(Provider::Codex, "not json").is_empty());
        assert!(parse(Provider::Codex, r#"{"status":{"indicator":"none"}}"#).is_empty());
    }

    #[test]
    fn an_open_row_carries_what_has_been_posted_about_it_newest_first() {
        let body = r#"{"incidents":[{"id":"a1","name":"Elevated errors","status":"monitoring","impact":"major",
          "incident_updates":[
            {"status":"investigating","body":"We are looking into it.","display_at":"2026-09-06T01:00:00Z"},
            {"status":"monitoring","body":"A fix is in place.","display_at":"2026-09-06T02:00:00Z"},
            {"status":"","body":"","created_at":"2026-09-06T03:00:00Z"}
          ]}]}"#;
        let open = parse(Provider::Anthropic, body);
        let updates = &open[0].updates;
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].status, "monitoring");
        assert_eq!(updates[0].body, "A fix is in place.");
        assert_eq!(updates[0].at, "2026-09-06T02:00:00Z");
        assert_eq!(updates[1].status, "investigating");
        // A row the page carries nothing for is an empty list, never a
        // missing field: absent is how a caller reads a daemon older than
        // this, and a page that has posted nothing is not that.
        let bare = parse(
            Provider::Anthropic,
            r#"{"incidents":[{"id":"b","name":"Bare","status":"identified","impact":"minor"}]}"#,
        );
        assert!(bare[0].updates.is_empty());
        let answer = serde_json::to_value(&bare[0]).unwrap();
        assert_eq!(answer["updates"], serde_json::json!([]));
    }

    #[test]
    fn the_store_keeps_the_last_list_through_a_failed_read_and_says_so() {
        let store = IncidentStore::default();
        store.record(
            Provider::Anthropic,
            Ok(parse(
                Provider::Anthropic,
                r#"{"incidents":[{"id":"x","name":"Slow","status":"identified","impact":"minor"}]}"#,
            )),
        );
        store.record(Provider::Codex, Err("timed out".to_owned()));
        store.checked(1_700_000_000);
        let answer = store.to_json();
        assert_eq!(answer["incidents"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            answer["providers"],
            serde_json::json!(["anthropic", "codex"])
        );
        assert_eq!(answer["errors"]["codex"], "timed out");
        assert_eq!(answer["checked_at"], 1_700_000_000);
        // A later failure at anthropic keeps its row and names the failure.
        store.record(Provider::Anthropic, Err("HTTP 503".to_owned()));
        let answer = store.to_json();
        assert_eq!(answer["incidents"].as_array().map(Vec::len), Some(1));
        assert_eq!(answer["errors"]["anthropic"], "HTTP 503");
        // A provider no account is on any more is dropped whole.
        store.forget_except(&[Provider::Codex]);
        let answer = store.to_json();
        assert_eq!(answer["incidents"].as_array().map(Vec::len), Some(0));
        assert_eq!(answer["providers"], serde_json::json!(["codex"]));
    }

    #[test]
    fn open_incidents_come_worst_first() {
        let store = IncidentStore::default();
        store.record(
            Provider::Codex,
            Ok(parse(
                Provider::Codex,
                r#"{"incidents":[{"id":"m","name":"Minor","status":"monitoring","impact":"minor"},{"id":"c","name":"Critical","status":"investigating","impact":"critical"}]}"#,
            )),
        );
        let ids: Vec<String> = store.open().into_iter().map(|i| i.id).collect();
        assert_eq!(ids, ["c", "m"]);
    }
}
