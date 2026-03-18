use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

use crate::text_utils::summarize_text_for_trace;

#[derive(Clone)]
pub(crate) struct InputLogger {
    sender: mpsc::Sender<LogRecord>,
    raw: bool,
}

#[derive(Debug)]
struct LogRecord {
    ts_ms: u128,
    event: String,
    fields: Value,
}

impl InputLogger {
    pub(crate) fn new(path: &Path, raw: bool) -> std::io::Result<Self> {
        let writer = OpenOptions::new().create(true).append(true).open(path)?;
        let (sender, receiver) = mpsc::channel::<LogRecord>();

        let _ = thread::Builder::new()
            .name("agent-input-log".to_string())
            .spawn(move || {
                let mut writer = BufWriter::new(writer);
                while let Ok(record) = receiver.recv() {
                    let mut payload = Map::new();
                    payload.insert("ts_ms".to_string(), json!(record.ts_ms));
                    payload.insert("event".to_string(), json!(record.event));

                    match record.fields {
                        Value::Object(map) => {
                            for (k, v) in map {
                                payload.insert(k, v);
                            }
                        }
                        other => {
                            payload.insert("fields".to_string(), other);
                        }
                    }

                    if let Ok(line) = serde_json::to_string(&Value::Object(payload)) {
                        let _ = writeln!(writer, "{line}");
                    }
                }
                let _ = writer.flush();
            });

        Ok(Self {
            sender,
            raw,
        })
    }

    pub(crate) fn text_value(&self, text: &str) -> Value {
        if self.raw {
            json!(text)
        } else {
            json!(summarize_text_for_trace(text))
        }
    }

    pub(crate) fn log_event(&self, event: &str, fields: Value) {
        let _ = self.sender.send(LogRecord {
            ts_ms: now_unix_ms(),
            event: event.to_string(),
            fields,
        });
    }
}

fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}
