//! Authoritative allowance seam, independent of the thread/flight driver.
//!
//! Currently disabled: the server does not spawn `run`, so snapshots carry the
//! default unavailable reading. The module is kept dormant for re-enabling.
#![allow(dead_code)]
use crate::time::{format_rfc3339, parse_rfc3339};
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

pub const STALE_AFTER_SECS: i64 = 600;
#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Available,
    Unknown,
    Stale,
    Unavailable,
}
#[derive(Clone, Debug, Serialize)]
pub struct UsageSnapshot {
    pub status: Status,
    pub source_kind: String,
    pub last_attempt_at: String,
    pub accounts: Vec<UsageAccount>,
}
#[derive(Clone, Debug, Serialize)]
pub struct UsageAccount {
    pub account_key: String,
    pub provider_label: String,
    pub account_label: String,
    pub checked_at: Option<String>,
    pub status: Status,
    pub windows: Vec<UsageWindow>,
}
#[derive(Clone, Debug, Serialize)]
pub struct UsageWindow {
    pub kind: String,
    pub label: String,
    pub remaining_percent: Option<f64>,
    pub resets_at: Option<String>,
    pub duration_minutes: Option<u32>,
    pub model_ids: Vec<String>,
    pub provider_ids: Vec<String>,
}
#[derive(Deserialize)]
struct Input {
    source_id: String,
    environment_id: String,
    accounts: Vec<AccountInput>,
}
#[derive(Deserialize)]
struct AccountInput {
    id: String,
    provider_id: String,
    checked_at: Option<String>,
    #[serde(default)]
    unavailable: bool,
    #[serde(default)]
    probe_failed: bool,
    #[serde(default)]
    windows: Vec<WindowInput>,
}
#[derive(Deserialize)]
struct WindowInput {
    id: String,
    kind: String,
    used_percent: Option<f64>,
    resets_at: Option<String>,
    duration_minutes: Option<u32>,
    #[serde(default)]
    model_ids: Vec<String>,
    #[serde(default)]
    provider_ids: Vec<String>,
}
fn timestamp(s: &str) -> Option<i64> {
    // Narrow producer contract is canonical UTC RFC3339; reject ambiguous/local dates.
    let base = s.get(..19)?;
    if !s.ends_with('Z')
        || !(s.len() == 20
            || (s.as_bytes().get(19) == Some(&b'.')
                && s[20..s.len() - 1].bytes().all(|b| b.is_ascii_digit())
                && (22..=30).contains(&s.len())))
    {
        return None;
    }
    let ts = parse_rfc3339(s)?;
    (format_rfc3339(ts).get(..19) == Some(base)).then_some(ts)
}
fn opaque(parts: &[&str]) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut h);
    format!("account-{:016x}", h.finish())
}
fn scope_ids(ids: Vec<String>) -> Vec<String> {
    ids.into_iter()
        .map(|s| {
            if !s.is_empty()
                && s.len() <= 100
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.:/".contains(&b))
            {
                s
            } else {
                "unrecognized-scope".into()
            }
        })
        .collect()
}
pub fn normalize(value: serde_json::Value, now: i64) -> Result<UsageSnapshot, ()> {
    let input: Input = serde_json::from_value(value).map_err(|_| ())?;
    if input.source_id.is_empty() || input.environment_id.is_empty() || input.accounts.len() > 64 {
        return Err(());
    }
    let mut accounts = Vec::new();
    let mut keys = std::collections::HashSet::new();
    for a in input.accounts.into_iter() {
        if a.id.is_empty() || a.windows.len() > 64 {
            return Err(());
        }
        let key = opaque(&[
            &input.source_id,
            &input.environment_id,
            &a.provider_id,
            &a.id,
        ]);
        if !keys.insert(key.clone()) {
            return Err(());
        }
        let checked = a
            .checked_at
            .as_deref()
            .and_then(timestamp)
            .filter(|t| *t <= now + 60);
        let mut window_ids = std::collections::HashSet::new();
        let windows: Vec<_> = a
            .windows
            .into_iter()
            .map(|w| {
                let valid_id = !w.id.is_empty() && window_ids.insert(w.id);
                let kind = match w.kind.as_str() {
                    "session" => "session",
                    "weekly" => "weekly",
                    "monthly" => "monthly",
                    _ => "other",
                }
                .to_string();
                let label = if w.duration_minutes == Some(300) && kind != "monthly" {
                    "5-hour"
                } else {
                    match kind.as_str() {
                        "session" => "Session",
                        "weekly" => "Weekly",
                        "monthly" => "Monthly",
                        _ => "Other allowance",
                    }
                }
                .to_string();
                UsageWindow {
                    kind,
                    label,
                    remaining_percent: w
                        .used_percent
                        .filter(|v| {
                            valid_id
                                && checked.is_some()
                                && !a.unavailable
                                && v.is_finite()
                                && (0.0..=100.0).contains(v)
                        })
                        .map(|v| 100.0 - v),
                    resets_at: w.resets_at.filter(|s| timestamp(s).is_some()),
                    duration_minutes: w.duration_minutes,
                    model_ids: scope_ids(w.model_ids),
                    provider_ids: scope_ids(w.provider_ids),
                }
            })
            .collect();
        let status = if a.unavailable {
            Status::Unavailable
        } else if checked.is_some()
            && (a.probe_failed || now - checked.unwrap_or(now) > STALE_AFTER_SECS)
        {
            Status::Stale
        } else if checked.is_none()
            || windows.is_empty()
            || windows.iter().any(|w| w.remaining_percent.is_none())
        {
            Status::Unknown
        } else {
            Status::Available
        };
        let provider_label = match a.provider_id.as_str() {
            "codex" => "Codex",
            "claude" => "Claude",
            "cursor" => "Cursor",
            "antigravity" => "Antigravity",
            "opencode" => "OpenCode",
            _ => "Provider",
        }
        .to_string();
        let suffix = key[key.len() - 4..].to_ascii_uppercase();
        accounts.push(UsageAccount {
            account_key: key,
            provider_label,
            account_label: format!("Account {suffix}"),
            checked_at: checked.and(a.checked_at),
            status,
            windows,
        });
    }
    let status = if accounts.is_empty() {
        Status::Unknown
    } else if accounts.iter().all(|a| a.status == Status::Unavailable) {
        Status::Unavailable
    } else if accounts.iter().any(|a| a.status == Status::Stale) {
        Status::Stale
    } else if accounts.iter().all(|a| a.status == Status::Available) {
        Status::Available
    } else {
        Status::Unknown
    };
    Ok(UsageSnapshot {
        status,
        source_kind: "authoritative-input".into(),
        last_attempt_at: format_rfc3339(now),
        accounts,
    })
}
impl UsageSnapshot {
    pub fn unavailable(now: i64) -> Self {
        Self {
            status: Status::Unavailable,
            source_kind: "live-transport-unavailable".into(),
            last_attempt_at: format_rfc3339(now),
            accounts: Vec::new(),
        }
    }
}
/// Implementations may read only authoritative quota data, never token/cost telemetry.
pub trait UsageSource: Send {
    fn read(&mut self, now: i64) -> UsageSnapshot;
}
pub struct FileUsageSource {
    pub path: Option<PathBuf>,
    previous: Option<UsageSnapshot>,
}
impl FileUsageSource {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            previous: None,
        }
    }
}
impl UsageSource for FileUsageSource {
    fn read(&mut self, now: i64) -> UsageSnapshot {
        use std::io::Read;
        let result = self.path.as_ref().and_then(|p| {
            let mut bytes = Vec::new();
            std::fs::File::open(p)
                .ok()?
                .take(262145)
                .read_to_end(&mut bytes)
                .ok()?;
            if bytes.len() > 262144 {
                return None;
            }
            normalize(serde_json::from_slice(&bytes).ok()?, now).ok()
        });
        if let Some(s) = result {
            self.previous = Some(s.clone());
            return s;
        }
        if let Some(mut s) = self.previous.clone() {
            s.status = Status::Stale;
            s.last_attempt_at = format_rfc3339(now);
            for a in &mut s.accounts {
                if a.status != Status::Unavailable {
                    a.status = Status::Stale;
                }
            }
            return s;
        }
        UsageSnapshot::unavailable(now)
    }
}
pub async fn run(world: crate::ws::SharedWorld) {
    let mut source =
        FileUsageSource::new(std::env::var_os("SKYWATCH_USAGE_FILE").map(PathBuf::from));
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
    loop {
        interval.tick().await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let snapshot = source.read(now);
        world.lock().await.usage = snapshot;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input(used: serde_json::Value) -> serde_json::Value {
        serde_json::json!({"source_id":"t3", "environment_id":"local", "accounts":[{"id":"one", "provider_id":"codex", "checked_at":"2026-11-25T10:00:00Z", "windows":[{"id":"primary","kind":"session","used_percent":used,"duration_minutes":300,"resets_at":"2026-11-25T10:01:00Z"}]}]})
    }
    const NOW: i64 = 1795600800;
    #[test]
    fn validates_percentages_and_preserves_true_empty_full_unknown() {
        for (used, remaining) in [(0., 100.), (100., 0.), (37., 63.)] {
            let s = normalize(input(serde_json::json!(used)), NOW).unwrap();
            assert_eq!(s.accounts[0].windows[0].remaining_percent, Some(remaining));
        }
        for used in [
            serde_json::json!(-1),
            serde_json::json!(101),
            serde_json::Value::Null,
        ] {
            let s = normalize(input(used), NOW).unwrap();
            assert_eq!(s.accounts[0].windows[0].remaining_percent, None);
            assert_eq!(s.accounts[0].status, Status::Unknown);
        }
    }
    #[test]
    fn source_time_staleness_and_expiry_do_not_refill() {
        let s = normalize(input(serde_json::json!(100)), NOW + 601).unwrap();
        assert_eq!(s.accounts[0].status, Status::Stale);
        assert_eq!(s.accounts[0].windows[0].remaining_percent, Some(0.));
        assert_eq!(
            s.accounts[0].checked_at.as_deref(),
            Some("2026-11-25T10:00:00Z")
        );
        let mut v = input(serde_json::json!(37));
        v["accounts"][0]["checked_at"] = serde_json::json!("2026-13-99T10:00:00Z");
        assert_eq!(
            normalize(v, NOW).unwrap().accounts[0].status,
            Status::Unknown
        );
    }
    #[test]
    fn duplicate_identity_is_rejected_and_invalid_scope_stays_scoped() {
        let mut v = input(serde_json::json!(37));
        v["accounts"][0]["windows"][0]["model_ids"] = serde_json::json!(["not an identifier"]);
        let s = normalize(v.clone(), NOW).unwrap();
        assert_eq!(s.accounts[0].windows[0].model_ids, ["unrecognized-scope"]);
        let a = v["accounts"][0].clone();
        v["accounts"].as_array_mut().unwrap().push(a);
        assert!(normalize(v, NOW).is_err());
    }
    #[test]
    fn file_failure_retains_stale_values_and_never_reassigns_them() {
        let path =
            std::env::temp_dir().join(format!("skywatch-usage-test-{}.json", std::process::id()));
        std::fs::write(&path, input(serde_json::json!(100)).to_string()).unwrap();
        let mut source = FileUsageSource::new(Some(path.clone()));
        let first = source.read(NOW);
        std::fs::remove_file(&path).unwrap();
        let failed = source.read(NOW + 1);
        assert_eq!(failed.accounts[0].status, Status::Stale);
        assert_eq!(failed.accounts[0].windows[0].remaining_percent, Some(0.));
        assert_eq!(failed.accounts[0].checked_at, first.accounts[0].checked_at);
        assert_eq!(
            failed.accounts[0].account_key,
            first.accounts[0].account_key
        );
        assert_eq!(
            FileUsageSource::new(None).read(NOW).status,
            Status::Unavailable
        );
    }
    #[test]
    fn identities_are_scoped_and_metadata_is_redacted() {
        let mut v = input(serde_json::json!(37));
        v["accounts"][0]["email"] = serde_json::json!("private@example.com");
        v["accounts"][0]["credentialFingerprint"] = serde_json::json!("SECRET");
        v["accounts"][0]["label"] = serde_json::json!("SECRET");
        let a = normalize(v.clone(), NOW).unwrap();
        v["environment_id"] = serde_json::json!("remote");
        let b = normalize(v, NOW).unwrap();
        assert_ne!(a.accounts[0].account_key, b.accounts[0].account_key);
        let json = serde_json::to_string(&a).unwrap();
        assert!(
            !json.contains("SECRET") && !json.contains("private@") && !json.contains("fingerprint")
        );
    }
    #[test]
    fn account_labels_stay_stable_across_reordering_and_removal() {
        let two = serde_json::json!({
            "source_id": "t3",
            "environment_id": "local",
            "accounts": [
                {"id": "first", "provider_id": "codex", "checked_at": "2026-11-25T10:00:00Z", "windows": []},
                {"id": "second", "provider_id": "claude", "checked_at": "2026-11-25T10:00:00Z", "windows": []}
            ]
        });
        let swapped = serde_json::json!({
            "source_id": "t3",
            "environment_id": "local",
            "accounts": [
                {"id": "second", "provider_id": "claude", "checked_at": "2026-11-25T10:00:00Z", "windows": []},
                {"id": "first", "provider_id": "codex", "checked_at": "2026-11-25T10:00:00Z", "windows": []}
            ]
        });
        let only_second = serde_json::json!({
            "source_id": "t3",
            "environment_id": "local",
            "accounts": [
                {"id": "second", "provider_id": "claude", "checked_at": "2026-11-25T10:00:00Z", "windows": []}
            ]
        });
        let a = normalize(two, NOW).unwrap();
        let b = normalize(swapped, NOW).unwrap();
        let c = normalize(only_second, NOW).unwrap();
        assert_eq!(a.accounts[1].account_key, b.accounts[0].account_key);
        assert_eq!(a.accounts[1].account_label, b.accounts[0].account_label);
        assert_eq!(a.accounts[1].account_label, c.accounts[0].account_label);
        assert_eq!(a.accounts[0].account_label, b.accounts[1].account_label);
    }
    #[test]
    fn scope_ids_are_not_truncated_at_32() {
        let mut v = input(serde_json::json!(25));
        let scopes: Vec<String> = (0..35).map(|i| format!("model-{i}")).collect();
        v["accounts"][0]["windows"][0]["model_ids"] = serde_json::json!(scopes);
        let s = normalize(v, NOW).unwrap();
        assert_eq!(s.accounts[0].windows[0].model_ids.len(), 35);
        assert_eq!(s.accounts[0].windows[0].model_ids[34], "model-34");
    }
}
