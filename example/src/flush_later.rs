use fast_log::appender::{FastLogFormatRecord, FastLogRecord, LogAppender};
use fast_log::filter::NoFilter;
use may::coroutine::sleep;
use std::time::{Duration, Instant};

use fast_log::bencher::QPS;

struct BenchRecvLog {}

impl LogAppender for BenchRecvLog {
    fn do_log(&self, record: &mut FastLogRecord) {
        //do nothing
    }
}

// this example should be   "cargo run --release --package example --bin bench_test"
fn main() {
    let waiter = fast_log::init_log(
        &"/tmp/flush_later_log.log",
        log::Level::Trace,
        None,
        true,
    )
    .unwrap();
    let total = 10000;
    let now = Instant::now();
    for index in 0..total {
        log::info!("Commencing yak shaving{}", index);
    }
    now.time(total);
    now.qps(total);
    may::coroutine::sleep(Duration::from_secs(1));
    fast_log::flush().unwrap();
    waiter.wait();
}
