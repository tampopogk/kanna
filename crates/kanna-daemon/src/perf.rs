use kanna_mux::json::{json, Map, Value};
use kanna_mux::{Core, Responder};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

const KEPT: usize = 200;

#[derive(Default)]
struct Ring {
    samples: Vec<f64>,
    next: usize,
    count: u64,
}

impl Ring {
    fn add(&mut self, ms: f64) {
        self.count += 1;
        if self.samples.len() < KEPT {
            self.samples.push(ms);
        } else {
            self.samples[self.next] = ms;
        }
        self.next = (self.next + 1) % KEPT;
    }

    fn last(&self) -> f64 {
        self.samples[(self.next + self.samples.len() - 1) % self.samples.len()]
    }
}

static BOOK: Mutex<BTreeMap<String, Ring>> = Mutex::new(BTreeMap::new());

pub fn record(name: &str, took: Duration) {
    let ms = took.as_secs_f64() * 1000.0;
    let mut book = BOOK.lock().unwrap_or_else(PoisonError::into_inner);
    match book.get_mut(name) {
        Some(ring) => ring.add(ms),
        None => book.entry(name.to_string()).or_default().add(ms),
    }
}

pub fn since(name: &str, start: Instant) {
    record(name, start.elapsed());
}

pub fn reset() {
    BOOK.lock().unwrap_or_else(PoisonError::into_inner).clear();
}

pub fn quantile(sorted: &[f64], q: f64) -> f64 {
    let at = ((sorted.len() as f64 * q).ceil() as usize).clamp(1, sorted.len()) - 1;
    sorted[at]
}

pub fn stats() -> Value {
    let book = BOOK.lock().unwrap_or_else(PoisonError::into_inner);
    let round = |ms: f64| (ms * 100.0).round() / 100.0;
    let intervals: Vec<Value> = book
        .iter()
        .filter(|(_, ring)| !ring.samples.is_empty())
        .map(|(name, ring)| {
            let mut sorted = ring.samples.clone();
            sorted.sort_by(f64::total_cmp);
            json!({
                "name": name,
                "count": ring.count,
                "recent": sorted.len(),
                "p50": round(quantile(&sorted, 0.5)),
                "p95": round(quantile(&sorted, 0.95)),
                "p99": round(quantile(&sorted, 0.99)),
                "max": round(sorted[sorted.len() - 1]),
                "last": round(ring.last()),
            })
        })
        .collect();
    json!({ "intervals": intervals })
}

pub fn install(core: &Core) {
    if std::env::var("KANNA_DEBUG").as_deref() != Ok("1") {
        return;
    }
    core.add_command(
        "debug.daemon.perf",
        Rc::new(|args: Value, responder: Responder| {
            if args.get("reset").and_then(Value::as_bool) == Some(true) {
                reset();
            }
            let Value::Object(fields) = stats() else { return responder.reply(Ok(Map::new())) };
            responder.reply(Ok(fields))
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantiles_take_the_sample_at_or_above_the_rank() {
        let sorted: Vec<f64> = (1..=200).map(f64::from).collect();
        assert_eq!(quantile(&sorted, 0.5), 100.0);
        assert_eq!(quantile(&sorted, 0.99), 198.0);
        assert_eq!(quantile(&[3.0], 0.99), 3.0);
    }

    #[test]
    fn a_ring_keeps_the_last_samples() {
        let mut ring = Ring::default();
        for ms in 0..450 {
            ring.add(f64::from(ms));
        }
        assert_eq!((ring.count, ring.samples.len(), ring.last()), (450, KEPT, 449.0));
    }
}
