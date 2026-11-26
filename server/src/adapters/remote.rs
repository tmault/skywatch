//! Last-known peer snapshots: a failed or partial poll is never a deletion.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Peer {
    pub environment_id: String,
    pub label: String,
    pub online: bool,
    pub threads: Option<Vec<Value>>,
    /// Safe scheduled-task summaries (`None` when the peer could not be listed).
    #[serde(default)]
    pub scheduled_tasks: Option<Vec<Value>>,
}
#[derive(Default)]
pub struct Cache {
    peers: BTreeMap<String, Peer>,
    pub local: Option<Value>,
}
impl Cache {
    pub fn merge(&mut self, updates: Vec<Peer>) -> Vec<Peer> {
        for peer in self.peers.values_mut() { peer.online = false; }
        for mut update in updates {
            if update.threads.is_none() {
                update.online = false;
                update.threads = self.peers.get(&update.environment_id).and_then(|p| p.threads.clone());
            }
            // A failed task listing keeps the last known jobs; a successful one is authoritative.
            if update.scheduled_tasks.is_none() {
                update.scheduled_tasks = self.peers.get(&update.environment_id).and_then(|p| p.scheduled_tasks.clone());
            }
            if let (Some(previous), Some(current)) = (self.peers.get(&update.environment_id).and_then(|p| p.threads.as_ref()), update.threads.as_mut()) {
                for old in previous {
                    if !current.iter().any(|new| new.get("threadId") == old.get("threadId")) { current.push(old.clone()); }
                }
            }
            self.peers.insert(update.environment_id.clone(), update);
        }
        self.peers.values().cloned().collect()
    }
}
pub fn key(environment: &str, thread: &str) -> String {
    serde_json::to_string(&(environment, thread)).expect("strings serialize")
}
pub fn owner(key: &str) -> Option<(String, String)> {
    serde_json::from_str(key).ok()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_job_listing_keeps_jobs_and_authoritative_empty_removes_them() {
        let mut cache = Cache::default();
        let peer = |jobs| Peer { environment_id: "anon-device".into(), label: "Anon".into(),
            online: true, threads: Some(vec![]), scheduled_tasks: jobs };
        cache.merge(vec![peer(Some(vec![serde_json::json!({"scheduledTaskId":"anon-job"})]))]);
        let failed = cache.merge(vec![peer(None)]);
        assert_eq!(failed[0].scheduled_tasks.as_ref().unwrap().len(), 1);
        let empty = cache.merge(vec![peer(Some(vec![]))]);
        assert!(empty[0].scheduled_tasks.as_ref().unwrap().is_empty());
    }

    #[test]
    fn partial_peer_polls_preserve_last_known_threads_and_identity() {
        let mut cache = Cache::default();
        let peer = |online, threads| Peer { environment_id: "mac".into(), label: "Mac".into(), online, threads, scheduled_tasks: None };
        cache.merge(vec![peer(true, Some(vec![serde_json::json!({"threadId":"one"})]))]);
        let failed = cache.merge(vec![peer(false, None)]);
        assert_eq!(failed[0].threads.as_ref().unwrap().len(), 1);
        assert!(!failed[0].online);
        let recovered = cache.merge(vec![peer(true, Some(vec![]))]);
        assert_eq!(recovered[0].threads.as_ref().unwrap().len(), 1, "absence is not authoritative deletion");
        assert_ne!(key("mac", "one"), key("pc", "one"));
        assert_eq!(owner(&key("mac", "one")), Some(("mac".into(), "one".into())));
    }
}

/// Invoke the bundled transport without placing credentials in command arguments.
pub fn bridge(request: &Value) -> Option<Value> {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = crate::platform::python()?
        .args(["-c", include_str!("../../../scripts/t3_bridge.py")])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().ok()?;
    child.stdin.take()?.write_all(serde_json::to_string(request).ok()?.as_bytes()).ok()?;
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        report_failure(String::from_utf8_lossy(&output.stderr).trim());
        return None;
    }
    report_failure("");
    serde_json::from_slice(&output.stdout).ok()
}

/// Log bridge failures once per distinct reason rather than every poll.
fn report_failure(reason: &str) {
    static LAST: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
    let Ok(mut last) = LAST.lock() else { return };
    if *last == reason { return; }
    if reason.is_empty() {
        tracing::info!("T3 MCP bridge recovered");
    } else {
        tracing::warn!("{reason}");
    }
    *last = reason.to_owned();
}
