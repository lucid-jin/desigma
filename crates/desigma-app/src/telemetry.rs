//! Error and import telemetry, sent to an Axiom dataset so failures from other machines can be
//! seen in one place.
//!
//! Off unless a token is present: a build gets one by stamping `DESIGMA_AXIOM_TOKEN` and
//! `DESIGMA_AXIOM_DATASET` at compile time (the token should be an ingest-only token for that one
//! dataset), a run by setting the same variables. A user opts out with `DESIGMA_TELEMETRY=0` or by
//! creating an empty `telemetry-off` file in the per-user data folder.
//!
//! What is sent: event name, app version, OS, architecture, a random per-run session id, counts,
//! sizes and timings, and an error message with anything that looks like a path replaced. Never
//! file contents, layer or file names, or paths. Sending never blocks the app and never fails
//! loudly: events go through a bounded queue to one background thread, a full queue or a failed
//! request drops them, and a run sends at most [`MAX_EVENTS_PER_RUN`] so the free quota lasts.

use serde_json::{json, Map, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::OnceLock;
use std::time::Duration;

const BAKED_TOKEN: Option<&str> = option_env!("DESIGMA_AXIOM_TOKEN");
const BAKED_DATASET: Option<&str> = option_env!("DESIGMA_AXIOM_DATASET");
const DEFAULT_URL: &str = "https://api.axiom.co";

const QUEUE_LEN: usize = 256;
const BATCH_LEN: usize = 50;
const FLUSH_EVERY: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// A run that sends more than this is probably in a loop; the rest is dropped.
pub const MAX_EVENTS_PER_RUN: usize = 500;
const MAX_ERROR_CHARS: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Config {
    url: String,
    token: String,
    dataset: String,
}

enum Msg {
    Event(Value),
    /// Send what is queued now, then signal.
    Flush(SyncSender<()>),
}

struct Hub {
    tx: SyncSender<Msg>,
    sent: AtomicUsize,
    session: String,
}

static HUB: OnceLock<Option<Hub>> = OnceLock::new();

fn opted_out() -> bool {
    if std::env::var("DESIGMA_TELEMETRY").is_ok_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "off" | "false" | "no")) {
        return true;
    }
    dirs::data_local_dir().is_some_and(|d| d.join("desigma").join("telemetry-off").exists())
}

fn env_or(name: &str, baked: Option<&str>) -> Option<String> {
    std::env::var(name).ok().or_else(|| baked.map(str::to_string)).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn config() -> Option<Config> {
    if opted_out() {
        return None;
    }
    Some(Config {
        url: std::env::var("DESIGMA_AXIOM_URL").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| DEFAULT_URL.to_string()),
        token: env_or("DESIGMA_AXIOM_TOKEN", BAKED_TOKEN)?,
        dataset: env_or("DESIGMA_AXIOM_DATASET", BAKED_DATASET)?,
    })
}

fn hub() -> Option<&'static Hub> {
    HUB.get_or_init(|| {
        let cfg = config()?;
        let (tx, rx) = mpsc::sync_channel(QUEUE_LEN);
        std::thread::Builder::new().name("telemetry".into()).spawn(move || worker(&cfg, &rx)).ok()?;
        Some(Hub { tx, sent: AtomicUsize::new(0), session: uuid::Uuid::new_v4().simple().to_string() })
    })
    .as_ref()
}

/// Queues one event. Cheap and never blocks; does nothing when telemetry is off.
pub fn event(name: &str, fields: Value) {
    let Some(hub) = hub() else { return };
    if hub.sent.fetch_add(1, Ordering::Relaxed) >= MAX_EVENTS_PER_RUN {
        return;
    }
    let _ = hub.tx.try_send(Msg::Event(build_event(name, &hub.session, fields)));
}

/// Sends what is queued and waits briefly for it. Call before the process ends.
pub fn flush() {
    let Some(hub) = hub() else { return };
    let (ack, done) = mpsc::sync_channel(1);
    if hub.tx.try_send(Msg::Flush(ack)).is_ok() {
        let _ = done.recv_timeout(REQUEST_TIMEOUT + Duration::from_secs(1));
    }
}

/// A panic, sent at once because the process may be about to end. The backtrace stays in the
/// local crash log; only the message and the source location leave.
pub fn panic_event(message: &str, location: Option<&str>) {
    if hub().is_none() {
        return;
    }
    event("panic", json!({ "message": scrub(message), "location": location.unwrap_or("unknown") }));
    flush();
}

/// A finished import: how long it took, how big it was, and whether it worked.
pub fn import_event(path: &std::path::Path, started: std::time::Instant, outcome: &Result<usize, &str>) {
    if hub().is_none() {
        return;
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut f = json!({ "ext": ext, "file_bytes": bytes, "elapsed_ms": started.elapsed().as_millis() as u64 });
    match outcome {
        Ok(nodes) => {
            f["ok"] = json!(true);
            f["nodes"] = json!(nodes);
        }
        Err(e) => {
            f["ok"] = json!(false);
            f["error"] = json!(scrub(e));
        }
    }
    event("import", f);
}

fn build_event(name: &str, session: &str, fields: Value) -> Value {
    let mut m = Map::new();
    if let Value::Object(f) = fields {
        m.extend(f);
    }
    m.insert("event".into(), json!(name));
    m.insert("version".into(), json!(crate::VERSION));
    m.insert("release_build".into(), json!(crate::RELEASE_BUILD));
    m.insert("os".into(), json!(std::env::consts::OS));
    m.insert("arch".into(), json!(std::env::consts::ARCH));
    m.insert("session".into(), json!(session));
    Value::Object(m)
}

/// Replaces anything that looks like a path (a word with a slash in it) and cuts the text short,
/// so an error message cannot carry a user's folder or file names out.
pub fn scrub(text: &str) -> String {
    let cleaned = text.split_whitespace().map(|w| if w.contains('/') || w.contains('\\') { "<path>" } else { w }).collect::<Vec<_>>().join(" ");
    cleaned.chars().take(MAX_ERROR_CHARS).collect()
}

fn worker(cfg: &Config, rx: &Receiver<Msg>) {
    let mut batch: Vec<Value> = Vec::new();
    loop {
        match rx.recv_timeout(FLUSH_EVERY) {
            Ok(Msg::Event(e)) => {
                batch.push(e);
                if batch.len() >= BATCH_LEN {
                    send(cfg, &mut batch);
                }
            }
            Ok(Msg::Flush(ack)) => {
                send(cfg, &mut batch);
                let _ = ack.send(());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => send(cfg, &mut batch),
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Posts the batch and empties it. A failure drops the batch: telemetry must never pile up.
fn send(cfg: &Config, batch: &mut Vec<Value>) {
    if batch.is_empty() {
        return;
    }
    let body = Value::Array(std::mem::take(batch)).to_string();
    let url = format!("{}/v1/datasets/{}/ingest", cfg.url.trim_end_matches('/'), cfg.dataset);
    let agent = ureq::AgentBuilder::new().timeout(REQUEST_TIMEOUT).build();
    let _ = agent.post(&url).set("Authorization", &format!("Bearer {}", cfg.token)).set("Content-Type", "application/json").send_string(&body);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;

    #[test]
    fn scrub_hides_paths_and_cuts_long_text() {
        assert_eq!(scrub("Cannot read /Users/me/secret plan.fig: not found"), "Cannot read <path> plan.fig: not found");
        assert_eq!(scrub("C:\\Users\\me\\a.fig failed"), "<path> failed");
        assert_eq!(scrub("Input file exceeds the 256 MiB import limit"), "Input file exceeds the 256 MiB import limit");
        assert_eq!(scrub(&"x ".repeat(500)).chars().count(), MAX_ERROR_CHARS);
    }

    #[test]
    fn events_carry_context_and_no_caller_can_override_it() {
        let e = build_event("import", "abc", json!({ "nodes": 3, "version": "forged" }));
        assert_eq!(e["event"], "import");
        assert_eq!(e["nodes"], 3);
        assert_eq!(e["session"], "abc");
        assert_eq!(e["version"], crate::VERSION);
        assert!(e["os"].is_string() && e["arch"].is_string());
    }

    #[test]
    fn a_batch_is_posted_to_the_dataset_with_the_token() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = vec![0u8; 8192];
            let mut got = String::new();
            while !got.contains("\r\n\r\n") || !got.ends_with(']') {
                let n = s.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                got.push_str(&String::from_utf8_lossy(&buf[..n]));
            }
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").unwrap();
            got
        });
        let cfg = Config { url, token: "tok-123".into(), dataset: "desigma-test".into() };
        let mut batch = vec![build_event("import", "s1", json!({ "ok": true, "nodes": 25749 }))];
        send(&cfg, &mut batch);
        assert!(batch.is_empty());
        let req = server.join().unwrap();
        assert!(req.starts_with("POST /v1/datasets/desigma-test/ingest HTTP/1.1"), "{req}");
        assert!(req.to_ascii_lowercase().contains("authorization: bearer tok-123"), "{req}");
        let body = &req[req.find("\r\n\r\n").unwrap() + 4..];
        let parsed: Value = serde_json::from_str(body).unwrap();
        assert_eq!(parsed[0]["nodes"], 25749);
        assert_eq!(parsed[0]["session"], "s1");
    }

    #[test]
    fn an_unreachable_server_drops_the_batch_and_does_not_panic() {
        let cfg = Config { url: "http://127.0.0.1:1".into(), token: "t".into(), dataset: "d".into() };
        let mut batch = vec![json!({ "event": "x" })];
        send(&cfg, &mut batch);
        assert!(batch.is_empty());
    }

    // `config` reads process-wide environment variables, so the cases share one test.
    #[test]
    fn config_needs_a_token_and_a_dataset_and_respects_the_opt_out() {
        static LOCK: Mutex<()> = Mutex::new(());
        let _guard = LOCK.lock().unwrap();
        for v in ["DESIGMA_AXIOM_TOKEN", "DESIGMA_AXIOM_DATASET", "DESIGMA_AXIOM_URL", "DESIGMA_TELEMETRY"] {
            // SAFETY: serialized by LOCK; no other test in this module touches these variables.
            unsafe { std::env::remove_var(v) };
        }
        if BAKED_TOKEN.is_none() {
            assert_eq!(config(), None);
        }
        unsafe {
            std::env::set_var("DESIGMA_AXIOM_TOKEN", "t");
            std::env::set_var("DESIGMA_AXIOM_DATASET", "d");
        }
        let cfg = config().unwrap();
        assert_eq!((cfg.url.as_str(), cfg.token.as_str(), cfg.dataset.as_str()), (DEFAULT_URL, "t", "d"));
        unsafe { std::env::set_var("DESIGMA_TELEMETRY", "0") };
        assert_eq!(config(), None);
        for v in ["DESIGMA_AXIOM_TOKEN", "DESIGMA_AXIOM_DATASET", "DESIGMA_TELEMETRY"] {
            unsafe { std::env::remove_var(v) };
        }
    }
}
