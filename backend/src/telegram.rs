//! Forwards ERROR events to a Telegram chat (docs/08-backend.md "Errors").
//!
//! Errors are batched: the first one opens a short window, everything arriving in it goes out as one
//! message, and messages are at least [`MIN_INTERVAL`] apart. An error storm therefore costs a handful
//! of requests, not one per event, and stays under Telegram's limits (~20 msg/min per chat).

use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, sleep, timeout_at};

use crate::logging::NO_TELEGRAM_TARGET;

pub const QUEUE: usize = 256;
const BATCH_WINDOW: Duration = Duration::from_secs(2);
const MIN_INTERVAL: Duration = Duration::from_secs(5);
/// Telegram's limit is 4096 characters.
const MAX_CHARS: usize = 3900;

pub struct Telegram {
    client: reqwest::Client,
    url: String,
    chat_id: String,
    host: String,
}

impl Telegram {
    pub fn new(token: &str, chat_id: &str) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder().timeout(Duration::from_secs(15)).build()?;
        // /etc/hostname: the unit hides /proc/sys (ProcSubset=pid).
        let host = ["/etc/hostname", "/proc/sys/kernel/hostname"]
            .iter()
            .find_map(|p| std::fs::read_to_string(p).ok())
            .map(|h| h.trim().to_owned())
            .unwrap_or_default();
        Ok(Telegram {
            client,
            url: format!("https://api.telegram.org/bot{token}/sendMessage"),
            chat_id: chat_id.to_owned(),
            host,
        })
    }

    /// Runs until `stop` fires; then whatever is queued goes out at once (errors raised while the
    /// service fails to start or shuts down are the ones that matter most).
    pub async fn run(self, mut rx: mpsc::Receiver<String>, mut stop: oneshot::Receiver<()>) {
        let mut next_send = Instant::now();
        loop {
            let first = tokio::select! {
                line = rx.recv() => match line { Some(line) => line, None => return },
                _ = &mut stop => break,
            };
            let mut lines = vec![first];
            let deadline = Instant::now().max(next_send) + BATCH_WINDOW;
            let stopping = loop {
                tokio::select! {
                    line = timeout_at(deadline, rx.recv()) => match line {
                        Ok(Some(line)) => lines.push(line),
                        _ => break false,
                    },
                    _ = &mut stop => break true,
                }
            };
            if stopping {
                while let Ok(line) = rx.try_recv() {
                    lines.push(line);
                }
                self.send(&self.format(&lines)).await;
                return;
            }
            let retry_after = self.send(&self.format(&lines)).await;
            next_send = Instant::now() + MIN_INTERVAL.max(retry_after);
        }
        let mut lines = Vec::new();
        while let Ok(line) = rx.try_recv() {
            lines.push(line);
        }
        if !lines.is_empty() {
            self.send(&self.format(&lines)).await;
        }
    }

    pub async fn send_test(&self) -> Result<(), String> {
        let response = self.post(&format!("aerolink@{}: test message", self.host)).await.map_err(|e| e.to_string())?;
        let status = response.status();
        if status.is_success() { Ok(()) } else { Err(format!("{status}: {}", response.text().await.unwrap_or_default())) }
    }

    fn format(&self, lines: &[String]) -> String {
        let mut text = format!("⚠ aerolink@{}: {} error(s)\n", self.host, lines.len());
        for line in lines {
            if text.chars().count() + line.chars().count() + 1 > MAX_CHARS {
                text.push_str("… (see error log)");
                break;
            }
            text.push_str(line);
            text.push('\n');
        }
        text
    }

    /// Returns how long Telegram asked us to back off (429), zero otherwise.
    async fn send(&self, text: &str) -> Duration {
        match self.post(text).await {
            Ok(r) if r.status().is_success() => Duration::ZERO,
            Ok(r) if r.status() == reqwest::StatusCode::TOO_MANY_REQUESTS => {
                let body: serde_json::Value = r.json().await.unwrap_or_default();
                let secs = body["parameters"]["retry_after"].as_u64().unwrap_or(30);
                tracing::warn!(target: NO_TELEGRAM_TARGET, "telegram rate limit, retry after {secs} s");
                sleep(Duration::from_secs(secs.min(300))).await;
                // One retry; if it fails as well the batch is dropped (it is in the error log anyway).
                let _ = self.post(text).await;
                Duration::from_secs(secs)
            }
            Ok(r) => {
                let status = r.status();
                tracing::error!(target: NO_TELEGRAM_TARGET, "telegram: {status} {}", r.text().await.unwrap_or_default());
                Duration::ZERO
            }
            Err(e) => {
                // Strip the URL: it contains the bot token.
                tracing::error!(target: NO_TELEGRAM_TARGET, "telegram: {}", e.without_url());
                Duration::ZERO
            }
        }
    }

    async fn post(&self, text: &str) -> Result<reqwest::Response, reqwest::Error> {
        let body = serde_json::json!({ "chat_id": self.chat_id, "text": text, "disable_web_page_preview": true });
        self.client.post(&self.url).json(&body).send().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_batches_are_truncated() {
        let tg = Telegram::new("t", "1").unwrap();
        let lines = vec!["x".repeat(1000); 10];
        let text = tg.format(&lines);
        assert!(text.chars().count() <= MAX_CHARS + 20);
        assert!(text.ends_with("(see error log)"));
        assert!(text.contains("10 error(s)"));
    }
}
