//! OpenCode 2.4.1 HTTP acquisition. Raw upstream payloads never enter snapshots.
use super::{fleet::*, scheduled};
use crate::{
    time::format_rfc3339,
    world::{Airport, ChatLinks, ScheduledJob, World, SATELLITE_SLOTS},
    ws::{ClientFrame, SharedWorld},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use reqwest::{Client, Url};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};
use tokio::sync::{broadcast, mpsc};

type Result<T> = std::result::Result<T, &'static str>;

fn origin(raw: &str) -> Result<Url> {
    let url = Url::parse(raw).map_err(|_| "invalid OpenCode origin")?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || (!loopback && url.scheme() != "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err("OpenCode origins require HTTPS (or loopback HTTP), without credentials, paths, query or fragment");
    }
    Ok(url)
}

pub struct Config {
    url: Url,
    web: Option<Url>,
    public_server: Option<Url>,
    password: Option<String>,
}
impl Config {
    pub fn from_env() -> Result<Self> {
        Self::load(|key| std::env::var(key).ok())
    }
    fn load(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let url =
            origin(&get("SKYWATCH_OPENCODE_URL").ok_or("SKYWATCH_OPENCODE_URL is required")?)?;
        let read_origin = |key| get(key).map(|v| origin(&v)).transpose();
        let web = read_origin("SKYWATCH_OPENCODE_WEB_URL")?;
        let public_server = read_origin("SKYWATCH_OPENCODE_PUBLIC_SERVER_URL")?;
        if web.is_some() != public_server.is_some() {
            return Err("OpenCode web and public server origins must be configured together");
        }
        let password = get("SKYWATCH_OPENCODE_PASSWORD_FILE")
            .map(|path| {
                let meta =
                    std::fs::metadata(&path).map_err(|_| "cannot read OpenCode password file")?;
                if !meta.is_file() {
                    return Err("OpenCode password must be a regular file");
                }
                if meta.len() > 16384 {
                    return Err("OpenCode password file too large");
                }
                use std::io::Read;
                let mut value = String::new();
                std::fs::File::open(path)
                    .map_err(|_| "cannot read OpenCode password file")?
                    .take(16385)
                    .read_to_string(&mut value)
                    .map_err(|_| "cannot read OpenCode password file")?;
                if value.len() > 16384 {
                    return Err("OpenCode password file too large");
                }
                let value = value.trim_end_matches(['\r', '\n']).to_owned();
                if value.is_empty() {
                    return Err("OpenCode password file is empty");
                }
                Ok(value)
            })
            .transpose()?;
        Ok(Self {
            url,
            web,
            public_server,
            password,
        })
    }
    fn links(&self, session: &str) -> Option<ChatLinks> {
        if matches!(session, "" | "." | "..") {
            return None;
        }
        let mut url = self.web.clone()?;
        let key = self.public_server.as_ref()?.as_str().trim_end_matches('/');
        url.path_segments_mut().ok()?.extend([
            "server",
            &URL_SAFE_NO_PAD.encode(key),
            "session",
            session,
        ]);
        Some(ChatLinks {
            web: url.to_string(),
            desktop: String::new(),
        })
    }
}
struct Api {
    config: Config,
    client: Client,
}
impl Api {
    fn new(config: Config) -> Result<Self> {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(8))
            .connect_timeout(Duration::from_secs(3))
            .build()
            .map_err(|_| "cannot initialize OpenCode HTTP client")?;
        Ok(Self { config, client })
    }
    async fn request<T: DeserializeOwned>(
        &self,
        segments: &[&str],
        query: &[(&str, &str)],
        body: Option<Value>,
    ) -> Result<T> {
        if segments.iter().any(|s| matches!(*s, "" | "." | "..")) {
            return Err("invalid OpenCode API path");
        }
        let mut url = self.config.url.clone();
        url.path_segments_mut()
            .map_err(|_| "invalid API origin")?
            .extend(segments);
        url.query_pairs_mut().extend_pairs(query);
        let mut req = if let Some(body) = body {
            self.client.post(url).json(&body)
        } else {
            self.client.get(url)
        };
        if let Some(password) = &self.config.password {
            req = req.basic_auth("opencode", Some(password));
        }
        let mut response = req
            .send()
            .await
            .map_err(|_| "OpenCode connection failed or timed out")?;
        if !response.status().is_success() {
            return Err("OpenCode HTTP request rejected; state retained");
        }
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            return serde_json::from_value(Value::Null)
                .map_err(|_| "unexpected empty OpenCode response");
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "OpenCode response interrupted")?
        {
            if bytes.len() + chunk.len() > 8 * 1024 * 1024 {
                return Err("OpenCode response exceeds limit");
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| "OpenCode response does not match v2.4.1")
    }
    async fn get<T: DeserializeOwned>(&self, path: &[&str]) -> Result<T> {
        self.request(path, &[], None).await
    }
    async fn sessions(&self, archived: bool) -> Result<Vec<Session>> {
        let archived = if archived { "true" } else { "false" };
        let mut result = Vec::new();
        let mut cursor = None::<String>;
        let mut seen = HashSet::new();
        for _ in 0..100 {
            let query = cursor
                .as_deref()
                .map(|c| vec![("cursor", c), ("limit", "100"), ("archived", archived)])
                .unwrap_or_else(|| vec![("limit", "100"), ("archived", archived)]);
            let page: Page<Session> = self.request(&["api", "session"], &query, None).await?;
            result.extend(page.data);
            let Some(next) = page.cursor.next else {
                return Ok(result);
            };
            if !seen.insert(next.clone()) {
                return Err("OpenCode pagination repeated a cursor");
            }
            cursor = Some(next);
        }
        Err("OpenCode pagination exceeds limit; state retained")
    }
    async fn permissions(&self, id: &str) -> Result<Vec<Permission>> {
        let data: Data<Vec<Permission>> = self.get(&["api", "session", id, "permission"]).await?;
        if data.data.iter().any(|p| p.session_id != id) {
            return Err("OpenCode permission ownership mismatch");
        }
        Ok(data.data)
    }
}
#[derive(Deserialize)]
struct Data<T> {
    data: T,
}
#[derive(Deserialize)]
struct Cursor {
    next: Option<String>,
}
#[derive(Deserialize)]
struct Page<T> {
    data: Vec<T>,
    cursor: Cursor,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    id: String,
    #[serde(rename = "parentID")]
    parent_id: Option<String>,
    #[serde(rename = "projectID")]
    project_id: String,
    title: Option<String>,
    model: Option<Model>,
    time: SessionTime,
    outcome: Option<String>,
}
#[derive(Deserialize)]
struct Model {
    id: String,
    #[serde(rename = "providerID")]
    provider_id: String,
}
#[derive(Deserialize)]
struct SessionTime {
    updated: i64,
    archived: Option<i64>,
}
#[derive(Deserialize)]
struct Project {
    id: String,
    name: Option<String>,
    canonical: String,
}
#[derive(Deserialize)]
struct Machine {
    id: String,
    online: bool,
    paired: bool,
}
#[derive(Deserialize)]
struct Permission {
    id: String,
    #[serde(rename = "sessionID")]
    session_id: String,
}
fn current_permission<'a>(
    permissions: &'a [Permission],
    session: &str,
    expected: Option<&str>,
) -> Result<&'a str> {
    let first = permissions
        .first()
        .ok_or("No current permission request; open OpenCode for forms")?;
    if first.session_id != session || Some(first.id.as_str()) != expected {
        return Err("Permission changed; refresh before replying");
    }
    Ok(&first.id)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Automation {
    id: String,
    name: String,
    #[serde(rename = "projectID")]
    project_id: String,
    #[serde(rename = "sessionID")]
    session_id: Option<String>,
    status: String,
    trigger: Trigger,
    next_due_at: Option<i64>,
    last_run_at: Option<i64>,
    last_outcome: Option<String>,
    run_count: Option<u64>,
}
// An allowlist: event arguments, action, program, prompt, output and state are never retained.
#[derive(Deserialize)]
struct Trigger {
    #[serde(rename = "type")]
    kind: String,
    at: Option<String>,
    every: Option<String>,
    cron: Option<String>,
}
impl Automation {
    fn job(self) -> scheduled::JobEntry {
        let schedule = match self.trigger.kind.as_str() {
            "at" => format!("Once at {}", self.trigger.at.unwrap_or_default()),
            "every" => format!("Every {}", self.trigger.every.unwrap_or_default()),
            "cron" => format!("Cron {}", self.trigger.cron.unwrap_or_default()),
            _ => "Event trigger".into(),
        };
        scheduled::JobEntry {
            key: self.id,
            title: self.name,
            project: Some(self.project_id),
            thread_id: self.session_id,
            model: None,
            job: ScheduledJob {
                schedule,
                enabled: self.status == "active",
                running: false,
                next_run_at: self.next_due_at.map(|t| format_rfc3339(t / 1000)),
                last_run_at: self.last_run_at.map(|t| format_rfc3339(t / 1000)),
                last_run_status: self.last_outcome.unwrap_or_else(|| "never".into()),
                run_count: self.run_count,
                ..Default::default()
            },
        }
    }
}
fn quota_retry(message: &Value, now_ms: i64) -> bool {
    let retry = &message["retry"];
    let error = &retry["error"];
    retry["at"].as_i64().is_some_and(|at| at > now_ms) && (error["status"].as_u64() == Some(429))
}
struct Snapshot {
    entries: Vec<SessionEntry>,
    children: Vec<ChildEntry>,
    archived: HashSet<String>,
    permissions: HashMap<String, String>,
    airports: Vec<Airport>,
}
impl Api {
    async fn poll(&self, old_airports: &[Airport]) -> Result<Snapshot> {
        let mut sessions = self.sessions(false).await?;
        sessions.extend(self.sessions(true).await?);
        let active: Data<HashMap<String, Value>> = self.get(&["api", "session", "active"]).await?;
        let projects: Vec<Project> = self.get(&["api", "project"]).await?;
        let projects: HashMap<_, _> = projects
            .into_iter()
            .map(|p| (p.id, p.name.unwrap_or(p.canonical)))
            .collect();
        let mut airports = vec![Airport::home("local", "OpenCode")];
        match self
            .get::<Data<Vec<Machine>>>(&["api", "experimental", "machine"])
            .await
        {
            Ok(machines) => {
                for (i, m) in machines
                    .data
                    .into_iter()
                    .filter(|m| m.paired)
                    .take(SATELLITE_SLOTS.len())
                    .enumerate()
                {
                    airports.push(Airport::satellite(
                        format!("opencode-machine:{}", m.id),
                        m.id,
                        SATELLITE_SLOTS[i],
                        m.online,
                    ));
                }
            }
            Err(_) => airports.extend(old_airports.iter().filter(|a| !a.is_home).cloned().map(
                |mut a| {
                    a.online = false;
                    a
                },
            )),
        }
        let now = super::t3::unix_now() * 1000;
        let mut entries = Vec::new();
        let mut children = Vec::new();
        let mut archived = HashSet::new();
        let mut permissions = HashMap::new();
        let ids: HashSet<_> = sessions.iter().map(|s| s.id.clone()).collect();
        let parents: HashMap<_, _> = sessions
            .iter()
            .filter_map(|s| s.parent_id.as_ref().map(|p| (s.id.clone(), p.clone())))
            .collect();
        for s in sessions {
            if s.time.archived.is_some() {
                archived.insert(s.id);
                continue;
            }
            let pending = self.permissions(&s.id).await?;
            let forms: Data<Vec<Value>> = self.get(&["api", "session", &s.id, "form"]).await?;
            if let Some(p) = pending.first() {
                permissions.insert(s.id.clone(), p.id.clone());
            }
            let running = active.data.contains_key(&s.id);
            let mut e = SessionEntry {
                thread_id: s.id.clone(),
                title: s.title.unwrap_or_else(|| "OpenCode session".into()),
                project: projects.get(&s.project_id).cloned(),
                status: if running {
                    "running"
                } else {
                    match s.outcome.as_deref() {
                        Some("failed") => "failed",
                        Some("interrupted") => "interrupted",
                        _ => "idle",
                    }
                }
                .into(),
                model: s.model.as_ref().map(|m| m.id.clone()),
                latest_run_provider: s.model.as_ref().map(|m| m.provider_id.clone()),
                updated_at: Some(format_rfc3339(s.time.updated / 1000)),
                ..Default::default()
            };
            if !pending.is_empty() || !forms.data.is_empty() {
                e.status = "running".into();
                e.pending = 1;
                e.pending_kind = Some(
                    if pending.is_empty() {
                        "user_input"
                    } else {
                        "permission"
                    }
                    .into(),
                );
            } else if running {
                let messages: Page<Value> = self
                    .request(
                        &["api", "session", &s.id, "message"],
                        &[("type", "assistant"), ("order", "desc"), ("limit", "1")],
                        None,
                    )
                    .await?;
                if messages.data.first().is_some_and(|m| quota_retry(m, now)) {
                    e.status = "waiting".into();
                    e.usage_limit_code = Some("api_error_429".into());
                    // Retry deadline is deliberately NOT put in the quota-reset field.
                }
            }
            if let Some(root) = root_parent(&s.id, &parents, &ids) {
                if !is_working_status(&e.status)
                    && now - s.time.updated > super::t3::subagent_linger_secs() * 1000
                {
                    continue;
                }
                children.push(ChildEntry {
                    subagent_id: s.id.clone(),
                    thread_id: root,
                    child_thread_id: Some(s.id.clone()),
                    chat_thread_id: Some(s.id),
                    environment_id: Some("local".into()),
                    status: e.status.clone(),
                    updated_at: e.updated_at.clone(),
                    title: Some(e.title.clone()),
                    model: e.model.clone(),
                });
            } else {
                entries.push(e);
            }
        }
        Ok(Snapshot {
            entries,
            children,
            archived,
            permissions,
            airports,
        })
    }
}
fn root_parent(
    id: &str,
    parents: &HashMap<String, String>,
    ids: &HashSet<String>,
) -> Option<String> {
    let mut at = id;
    let mut seen = HashSet::from([id]);
    while let Some(parent) = parents.get(at) {
        if !ids.contains(parent) || !seen.insert(parent) {
            return None;
        }
        at = parent;
    }
    (at != id).then(|| at.to_owned())
}

#[derive(Default)]
struct DriverState {
    sky: Callsigns,
    taxi: TaxiwayCallsigns,
    gates: GateCallsigns,
    wings: Wingmen,
    runway: RunwayOps,
    reapers: scheduled::ReaperCallsigns,
    permissions: HashMap<String, String>,
    attempted_permissions: HashSet<(String, String)>,
    jobs: Vec<scheduled::JobEntry>,
}
impl DriverState {
    async fn refresh(&mut self, api: &Api, world: &SharedWorld) {
        let airports = world.lock().await.airports.clone();
        let snapshot = tokio::time::timeout(Duration::from_secs(25), api.poll(&airports)).await;
        let Ok(Ok(snapshot)) = snapshot else {
            tracing::warn!("OpenCode refresh incomplete; retaining stale world");
            mark_stale(&mut *world.lock().await);
            self.permissions.clear();
            return;
        };
        let mut jobs_stale = true;
        if let Ok(Ok(data)) = tokio::time::timeout(
            Duration::from_secs(8),
            api.get::<Data<Vec<Automation>>>(&["api", "experimental", "automation"]),
        )
        .await
        {
            self.jobs = data.data.into_iter().map(Automation::job).collect();
            jobs_stale = false;
        }
        self.permissions = snapshot.permissions;
        self.attempted_permissions
            .retain(|(session, request)| self.permissions.get(session) == Some(request));
        let now = super::t3::unix_now();
        let (flying, taxiway, stale, subagents, omitted) =
            partition_pools(snapshot.entries, snapshot.children, &snapshot.airports, now);
        let mut w = world.lock().await;
        apply_poll(
            &mut w,
            "OpenCode",
            PollData {
                omitted,
                local_environment_id: Some("local".into()),
                airports: snapshot.airports,
                flying,
                taxiway,
                stale,
                done_or_archived: snapshot.archived,
                subagents,
            },
            &mut self.sky,
            &mut self.taxi,
            &mut self.gates,
            &mut self.wings,
            &mut self.runway,
        );
        scheduled::apply_jobs(
            &mut w,
            "OpenCode",
            Some("local"),
            self.jobs.clone(),
            &mut self.reapers,
            now,
        );
        for a in &mut w.aircraft {
            if jobs_stale && a.scheduled_job.is_some() {
                a.data_stale = true;
                a.chat_links = None;
                continue;
            }
            if let Some(id) = a.thread_id.as_deref() {
                a.driver = Some("opencode".into());
                a.chat_links = api.config.links(id);
                a.approval_request_id = self
                    .permissions
                    .get(id)
                    .filter(|request| {
                        !self
                            .attempted_permissions
                            .contains(&(id.to_owned(), (*request).clone()))
                    })
                    .cloned();
            }
        }
    }
    async fn command(&mut self, api: &Api, world: &SharedWorld, frame: &ClientFrame) -> Result<()> {
        let id = aircraft_id(frame);
        let session = {
            let w = world.lock().await;
            w.aircraft
                .iter()
                .find(|a| {
                    a.id == id
                        && !a.data_stale
                        && a.scheduled_job.is_none()
                        && a.driver.as_deref() == Some("opencode")
                })
                .and_then(|a| a.thread_id.clone())
        }
        .ok_or("Target is unavailable or read-only")?;
        match frame {
            ClientFrame::Message { text, .. } => {
                if text.trim().is_empty() || text.len() > 65536 {
                    return Err("Message is empty or too large");
                }
                let _: Value = api
                    .request(
                        &["api", "session", &session, "prompt"],
                        &[],
                        Some(json!({"text":text})),
                    )
                    .await?;
            }
            ClientFrame::Approve { request_id, .. } | ClientFrame::Reject { request_id, .. } => {
                let expected = request_id.as_deref();
                if self.permissions.get(&session).map(String::as_str) != expected
                    || expected.is_none()
                {
                    return Err("Permission changed; refresh before replying");
                }
                let pending = api.permissions(&session).await?;
                let request = current_permission(&pending, &session, expected)?.to_owned();
                if !self
                    .attempted_permissions
                    .insert((session.clone(), request.clone()))
                {
                    return Err("Permission reply already attempted; check OpenCode");
                }
                // Consume before posting, including uncertain network outcomes.
                self.permissions.remove(&session);
                let decision = if matches!(frame, ClientFrame::Reject { .. }) {
                    "reject"
                } else {
                    "once"
                };
                let _: Value = api
                    .request(
                        &["api", "session", &session, "permission", &request, "reply"],
                        &[],
                        Some(json!({"decision":decision})),
                    )
                    .await?;
            }
        }
        Ok(())
    }
}
fn aircraft_id(frame: &ClientFrame) -> u64 {
    match frame {
        ClientFrame::Message { aircraft_id, .. }
        | ClientFrame::Approve { aircraft_id, .. }
        | ClientFrame::Reject { aircraft_id, .. } => *aircraft_id,
    }
}
pub async fn run(
    world: SharedWorld,
    mut rx: mpsc::Receiver<ClientFrame>,
    config: Config,
    results: broadcast::Sender<String>,
) {
    let api = match Api::new(config) {
        Ok(api) => api,
        Err(error) => {
            tracing::error!(error);
            return;
        }
    };
    let mut state = DriverState::default();
    let mut interval = tokio::time::interval(Duration::from_secs(3));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = interval.tick() => state.refresh(&api, &world).await,
            frame = rx.recv() => {
                let Some(frame) = frame else {return};
                let result = state.command(&api, &world, &frame).await;
                let _ = results.send(json!({"type":"command_result", "aircraft_id":aircraft_id(&frame),
                    "ok":result.is_ok(), "message":result.err().unwrap_or("Accepted by OpenCode")}).to_string());
            }
        }
    }
}
fn mark_stale(world: &mut World) {
    for a in &mut world.aircraft {
        a.data_stale = true;
        a.chat_links = None;
        a.approval_request_id = None;
    }
    for a in &mut world.airports {
        a.online = false;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn config(url: &str) -> Config {
        Config {
            url: origin(url).unwrap(),
            web: None,
            public_server: None,
            password: Some("test-password".into()),
        }
    }
    async fn server(router: axum::Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (url, task)
    }
    #[tokio::test]
    async fn pagination_is_complete_and_failure_never_becomes_empty_success() {
        use axum::{extract::Query, routing::get, Json};
        let (url, task) = server(axum::Router::new().route(
            "/api/session",
            get(|Query(q): Query<HashMap<String, String>>| async move {
                let mut page: Value = serde_json::from_str(include_str!(
                    "../../tests/fixtures/opencode/sessions.json"
                ))
                .unwrap();
                if let Some(cursor) = q.get("cursor") {
                    assert_eq!(cursor, "opaque&cursor");
                }
                if !q.contains_key("cursor") {
                    page["cursor"]["next"] = json!("opaque&cursor");
                }
                Json(page)
            }),
        ))
        .await;
        let api = Api::new(config(&url)).unwrap();
        assert_eq!(api.sessions(false).await.unwrap().len(), 6);
        assert!(api.get::<Value>(&["missing"]).await.is_err());
        task.abort();
    }
    #[tokio::test]
    async fn permission_reply_accepts_pinned_204_response() {
        let (url, task) = server(axum::Router::new().route(
            "/reply",
            axum::routing::post(|| async { axum::http::StatusCode::NO_CONTENT }),
        ))
        .await;
        let api = Api::new(config(&url)).unwrap();
        assert!(api
            .request::<Value>(&["reply"], &[], Some(json!({"decision":"once"})))
            .await
            .is_ok());
        task.abort();
    }
    #[tokio::test]
    async fn redirects_and_http_failure_are_not_command_success() {
        use axum::{http::StatusCode, response::Redirect, routing::get};
        let (url, task) = server(
            axum::Router::new()
                .route(
                    "/redirect",
                    get(|| async { Redirect::temporary("http://127.0.0.1:1/secret") }),
                )
                .route(
                    "/fail",
                    axum::routing::post(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
                ),
        )
        .await;
        let api = Api::new(config(&url)).unwrap();
        assert!(api.get::<Value>(&["redirect"]).await.is_err());
        assert!(api
            .request::<Value>(&["fail"], &[], Some(json!({"text":"hello"})))
            .await
            .is_err());
        task.abort();
    }
    #[tokio::test]
    async fn poll_uses_active_state_preserves_archives_and_rejects_partial_data() {
        use axum::{extract::OriginalUri, routing::get, Json};
        let fail = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = fail.clone();
        let (url,task)=server(axum::Router::new().fallback(get(move |OriginalUri(uri):OriginalUri| {let flag=flag.clone();async move {
            let path=uri.path();
            Json(if flag.load(std::sync::atomic::Ordering::SeqCst) && path.ends_with("/permission") {json!({"invalid":"partial"})} else if path=="/api/session" {
                let mut v:Value=serde_json::from_str(include_str!("../../tests/fixtures/opencode/sessions.json")).unwrap();
                if uri.query().unwrap_or("").contains("archived=true") {v["data"]=json!([{"id":"ses_archived","projectID":"prj_1","time":{"updated":1795564800000_i64,"archived":1795564800000_i64}}]);}
                v
            } else if path=="/api/session/active" {json!({"data":{"ses_child":{"type":"running"}}})}
            else if path=="/api/project" {json!([{"id":"prj_1","name":"Fixture project","canonical":"/workspace"}])}
            else if path.ends_with("/message") {json!({"data":[],"cursor":{"next":null}})}
            else {json!({"data":[]})})
        }}))).await;
        let api = Api::new(config(&url)).unwrap();
        let s = api.poll(&[]).await.unwrap();
        assert!(s.archived.contains("ses_archived"));
        assert_eq!(s.children.len(), 1);
        assert_eq!(s.children[0].thread_id, "ses_root");
        let (flying, taxi, _, _, _) =
            partition_pools(s.entries, s.children, &s.airports, 1795564800);
        assert!(flying.iter().any(|(e, _)| e.thread_id == "ses_root"));
        assert!(taxi.iter().any(|e| e.thread_id == "ses_fork"));
        fail.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(api.poll(&s.airports).await.is_err());
        task.abort();
    }
    #[test]
    fn fixture_hierarchy_only_uses_parent_id_and_handles_cycles() {
        let page: Page<Session> =
            serde_json::from_str(include_str!("../../tests/fixtures/opencode/sessions.json"))
                .unwrap();
        let ids = page.data.iter().map(|s| s.id.clone()).collect();
        let mut parents = page
            .data
            .iter()
            .filter_map(|s| s.parent_id.clone().map(|p| (s.id.clone(), p)))
            .collect::<HashMap<_, _>>();
        assert_eq!(
            root_parent("ses_child", &parents, &ids).as_deref(),
            Some("ses_root")
        );
        assert_eq!(root_parent("ses_fork", &parents, &ids), None);
        parents.insert("ses_root".into(), "ses_child".into());
        assert_eq!(root_parent("ses_child", &parents, &ids), None);
        parents.insert("ses_child".into(), "missing".into());
        assert_eq!(root_parent("ses_child", &parents, &ids), None);
    }
    #[test]
    fn links_use_official_base64_without_credentials_or_desktop_claim() {
        let mut c = config("http://127.0.0.1:4096");
        c.web = Some(origin("https://app.example").unwrap());
        c.public_server = Some(origin("https://api.example").unwrap());
        let links = c.links("ses_1").unwrap();
        assert_eq!(
            links.web,
            "https://app.example/server/aHR0cHM6Ly9hcGkuZXhhbXBsZQ/session/ses_1"
        );
        assert!(links.desktop.is_empty());
        assert!(c.links("..").is_none());
        assert!(!links.web.contains("password"));
    }
    #[test]
    fn failed_poll_disables_actions_without_erasing_aircraft() {
        let mut w = World::new();
        w.add(crate::world::Aircraft::new(
            "SKY1",
            crate::world::Kind::Agent,
        ));
        mark_stale(&mut w);
        assert_eq!(w.aircraft.len(), 1);
        assert!(w.aircraft[0].data_stale);
    }
    #[test]
    fn configuration_requires_url_pairs_public_links_and_keeps_errors_secret_safe() {
        assert!(Config::load(|_| None).is_err());
        let values = HashMap::from([
            ("SKYWATCH_OPENCODE_URL", "http://localhost:4096"),
            ("SKYWATCH_OPENCODE_WEB_URL", "https://app.example"),
        ]);
        assert!(Config::load(|k| values.get(k).map(|v| v.to_string())).is_err());
        let error = Config::load(|k| {
            (k == "SKYWATCH_OPENCODE_URL").then(|| "https://user:PRIVATE@example.com".into())
        })
        .err()
        .unwrap();
        assert!(!error.contains("PRIVATE"));
        let path =
            std::env::temp_dir().join(format!("skywatch-password-test-{}", std::process::id()));
        std::fs::write(&path, "TEST-SECRET\n").unwrap();
        let c = Config::load(|k| match k {
            "SKYWATCH_OPENCODE_URL" => Some("http://localhost:4096".into()),
            "SKYWATCH_OPENCODE_PASSWORD_FILE" => Some(path.to_str().unwrap().into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(c.password.as_deref(), Some("TEST-SECRET"));
        std::fs::remove_file(path).unwrap();
    }
    #[tokio::test]
    async fn adapter_replies_once_to_current_owned_permission_without_optimistic_release() {
        use axum::{
            extract::OriginalUri,
            http::StatusCode,
            routing::{get, post},
            Json,
        };
        let posts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let capture = posts.clone();
        let router = axum::Router::new()
            .route(
                "/api/session/ses_root/permission/per_1/reply",
                post(move |Json(body): Json<Value>| {
                    let capture = capture.clone();
                    async move {
                        capture.lock().unwrap().push(body);
                        StatusCode::NO_CONTENT
                    }
                }),
            )
            .fallback(get(|OriginalUri(uri): OriginalUri| async move {
                Json(match uri.path() {
                    "/api/session" => {
                        if uri.query().unwrap_or("").contains("archived=true") {
                            json!({"data":[],"cursor":{"next":null}})
                        } else {
                            let mut v: Value = serde_json::from_str(include_str!(
                                "../../tests/fixtures/opencode/sessions.json"
                            ))
                            .unwrap();
                            v["data"].as_array_mut().unwrap().truncate(1);
                            v
                        }
                    }
                    "/api/session/active" => json!({"data":{"ses_root":{"type":"running"}}}),
                    "/api/project" => json!([{"id":"prj_1","canonical":"/workspace"}]),
                    "/api/session/ses_root/permission" => {
                        json!({"data":[{"id":"per_1","sessionID":"ses_root"}]})
                    }
                    _ => json!({"data":[]}),
                })
            }));
        let (url, server_task) = server(router).await;
        let world = std::sync::Arc::new(tokio::sync::Mutex::new(World::new()));
        let (tx, rx) = mpsc::channel(8);
        let (results, mut result_rx) = broadcast::channel(8);
        let adapter = tokio::spawn(run(world.clone(), rx, config(&url), results));
        let id = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(a) = world
                    .lock()
                    .await
                    .aircraft
                    .iter()
                    .find(|a| a.approval_request_id.as_deref() == Some("per_1"))
                {
                    break a.id;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        for (request, ok) in [("old", false), ("per_1", true), ("per_1", false)] {
            tx.send(ClientFrame::Approve {
                aircraft_id: id,
                request_id: Some(request.into()),
            })
            .await
            .unwrap();
            let response = tokio::time::timeout(Duration::from_secs(2), result_rx.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(serde_json::from_str::<Value>(&response).unwrap()["ok"], ok);
        }
        assert_eq!(*posts.lock().unwrap(), vec![json!({"decision":"once"})]);
        assert_eq!(
            world
                .lock()
                .await
                .aircraft
                .iter()
                .find(|a| a.id == id)
                .unwrap()
                .state,
            crate::world::State::Holding
        );
        adapter.abort();
        server_task.abort();
    }
    #[test]
    fn origins_reject_secrets_paths_and_cleartext_remote() {
        for url in [
            "http://user:secret@localhost",
            "http://example.com",
            "http://localhost/a",
            "http://localhost/?token=x",
            "file:///tmp/x",
            "http://localhost/#x",
        ] {
            assert!(origin(url).is_err(), "{url}");
        }
        assert!(origin("http://127.0.0.1:4096").is_ok());
        assert!(origin("https://example.com").is_ok());
    }
    #[test]
    fn quota_requires_structured_rate_error_and_live_retry() {
        assert!(quota_retry(
            &serde_json::json!({"retry":{"at":2000,"error":{"status":429}}}),
            1000
        ));
        assert!(!quota_retry(
            &serde_json::json!({"retry":{"at":2000,"error":{"status":503,"message":"quota"}}}),
            1000
        ));
        assert!(!quota_retry(
            &serde_json::json!({"error":{"status":429}}),
            1000
        ));
        assert!(!quota_retry(
            &serde_json::json!({"retry":{"at":900,"error":{"status":429}}}),
            1000
        ));
    }
    #[test]
    fn automation_projection_drops_payload_and_does_not_infer_running() {
        let a: Automation = serde_json::from_value(serde_json::json!({"id":"auto1","name":"backup","projectID":"p1","sessionID":"ses1","status":"active","trigger":{"type":"event","secret":"hidden"},"action":{"prompt":"SECRET"},"state":"SECRET","lastOutput":"SECRET","lastOutcome":"fired","runCount":2})).unwrap();
        let entry = a.job();
        assert!(!entry.job.running);
        let wire = serde_json::to_string(&entry.job).unwrap();
        assert!(!wire.contains("SECRET"));
        assert!(!wire.contains("hidden"));
        assert_eq!(entry.job.last_run_status, "fired");
    }
    #[test]
    fn permissions_are_bound_to_exact_session() {
        let p = vec![Permission {
            id: "per1".into(),
            session_id: "ses-other".into(),
        }];
        assert!(current_permission(&p, "ses1", Some("per1")).is_err());
        assert!(current_permission(&p, "ses-other", Some("old")).is_err());
        assert_eq!(
            current_permission(&p, "ses-other", Some("per1")).unwrap(),
            "per1"
        );
    }
}
