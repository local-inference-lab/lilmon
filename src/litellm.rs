// Copyright 2026 Local Inference Lab, Inc.
// SPDX-License-Identifier: Apache-2.0

//! Recent per-request rows from LiteLLM's spend log, read through a long-lived `psql` session
//! started by a user-configured command (typically ssh + docker exec). Each query returns one
//! line of JSON, so no output escaping is involved.

use serde::Deserialize;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Deserialize)]
pub struct Row {
    pub start: f64,
    pub first: Option<f64>,
    pub end: f64,
    pub prompt: i64,
    pub completion: i64,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub client: String,
    #[serde(default)]
    pub model_group: String,
}

impl Row {
    /// Non-streaming responses record completionStartTime == endTime, so there is no TTFT.
    pub fn ttft(&self) -> Option<f64> {
        let f = self.first.filter(|f| *f >= self.start)?;
        (self.end - f > 0.005 || self.completion <= 1).then_some(f - self.start)
    }
    pub fn decode_rate(&self) -> Option<f64> {
        let f = self.first?;
        let d = self.end - f;
        (d > 0.05 && self.completion > 1).then(|| (self.completion - 1) as f64 / d)
    }
}

const MARK: &str = "__LILMON_END__";

fn query(pattern: &str) -> String {
    let pat = pattern.replace('\'', "''");
    // Client label: key alias, then team alias, then end user, then the last User-Agent tag.
    format!(
        r#"select coalesce(json_agg(r), '[]'::json) from (
  select extract(epoch from "startTime")::float8 as start,
         extract(epoch from "completionStartTime")::float8 as first,
         extract(epoch from "endTime")::float8 as "end",
         prompt_tokens as prompt, completion_tokens as completion,
         coalesce(status, '') as status,
         coalesce(nullif(metadata->>'user_api_key_alias', ''), nullif(metadata->>'user_api_key_team_alias', ''),
                  nullif(end_user, ''),
                  (select regexp_replace(x, '^User-Agent: ', '') from jsonb_array_elements_text(
                      case when jsonb_typeof(request_tags) = 'array' then request_tags else '[]'::jsonb end) x
                   where x like 'User-Agent:%' order by length(x) desc limit 1), '') as client,
         coalesce(model_group, '') as model_group
  from "LiteLLM_SpendLogs"
  where model_group like '{pat}' and "startTime" > (now() at time zone 'utc') - interval '6 hours'
  order by "startTime" desc limit 60) r;
\echo {MARK}
"#
    )
}

struct Session {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
}

impl Session {
    fn start(cmd: &[String]) -> std::io::Result<Session> {
        let mut child = Command::new(&cmd[0])
            .args(&cmd[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(Session { child, stdin, lines: rx })
    }

    fn run(&mut self, q: &str, timeout: Duration) -> Result<String, String> {
        self.stdin.write_all(q.as_bytes()).map_err(|e| e.to_string())?;
        self.stdin.flush().map_err(|e| e.to_string())?;
        let deadline = Instant::now() + timeout;
        let mut out = String::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(l) if l == MARK => return Ok(out),
                Ok(l) => {
                    out.push_str(&l);
                    out.push('\n');
                }
                Err(RecvTimeoutError::Timeout) => return Err("query timed out".into()),
                Err(RecvTimeoutError::Disconnected) => return Err("psql session ended".into()),
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `targets`: (endpoint index, model_group LIKE pattern).
pub fn spawn(tx: Sender<crate::Msg>, cmd: Vec<String>, poll: Duration, targets: Vec<(usize, String)>) {
    if cmd.is_empty() || targets.is_empty() {
        return;
    }
    std::thread::Builder::new()
        .name("litellm".into())
        .spawn(move || {
            let mut session: Option<Session> = None;
            let mut backoff = Duration::from_secs(2);
            loop {
                let t0 = Instant::now();
                if session.is_none() {
                    match Session::start(&cmd) {
                        Ok(s) => session = Some(s),
                        Err(e) => {
                            for (ep, _) in &targets {
                                let _ = tx.send(crate::Msg::LiteLlmErr(*ep, format!("start: {e}")));
                            }
                        }
                    }
                }
                let mut failed = false;
                if let Some(s) = session.as_mut() {
                    for (ep, pat) in &targets {
                        match s.run(&query(pat), Duration::from_secs(15)) {
                            Ok(json) => match serde_json::from_str::<Vec<Row>>(&json) {
                                Ok(rows) => {
                                    if tx.send(crate::Msg::LiteLlm(*ep, rows, crate::state::now())).is_err() {
                                        return;
                                    }
                                }
                                Err(e) => {
                                    let _ = tx.send(crate::Msg::LiteLlmErr(*ep, format!("bad reply: {e}")));
                                }
                            },
                            Err(e) => {
                                let _ = tx.send(crate::Msg::LiteLlmErr(*ep, e));
                                failed = true;
                                break;
                            }
                        }
                    }
                }
                if failed || session.is_none() {
                    session = None;
                    std::thread::sleep(backoff);
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                    continue;
                }
                backoff = Duration::from_secs(2);
                std::thread::sleep(poll.saturating_sub(t0.elapsed()));
            }
        })
        .expect("spawn litellm thread");
}
