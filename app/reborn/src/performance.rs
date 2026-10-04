//! Opt-in, bounded hardware trace. No trace allocation or I/O in normal runs.
use serde_json::{json, Value};
use std::{collections::VecDeque, path::PathBuf};

pub struct Trace {
    path: Option<PathBuf>,
    samples: VecDeque<Value>,
    dropped: usize,
}
impl Trace {
    pub fn new(path: Option<String>) -> Self {
        Self {
            path: path.map(PathBuf::from),
            samples: VecDeque::new(),
            dropped: 0,
        }
    }
    pub fn enabled(&self) -> bool {
        self.path.is_some()
    }
    pub fn record(&mut self, kind: &str, value: Value) {
        if !self.enabled() {
            return;
        }
        if self.samples.len() == 8192 {
            self.samples.pop_front();
            self.dropped += 1;
        }
        self.samples.push_back(
            json!({"kind":kind,"monotonic_s":reborn_platform::monotonic_seconds(),"data":value}),
        );
    }
}
impl Drop for Trace {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let result = serde_json::to_vec(
                &json!({"schema":1,"dropped":self.dropped,"samples":self.samples}),
            )
            .map_err(std::io::Error::other)
            .and_then(|bytes| std::fs::write(path, bytes));
            if let Err(error) = result {
                eprintln!("performance trace: {error}");
            }
        }
    }
}
