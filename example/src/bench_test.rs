use fast_log::filter::NoFilter;
use fast_log::appender::{FastLogFormatRecord, LogAppender, FastLogRecord};
use std::time::{Instant, Duration};
use may::coroutine::sleep;

use fast_log::bencher::QPS;

struct BenchRecvLog {}

impl LogAppender for BenchRecvLog {
    fn do_log(&self, record: &mut FastLogRecord) {
        //do nothing
    }
}

// this example should be   "cargo run --release --package example --bin bench_test"
fn main(){
    fast_log::init_custom_log(
        vec![Box::new(BenchRecvLog {})],
        log::Level::Info,
        Box::new(NoFilter {}),
        Box::new(FastLogFormatRecord::new()),
    );
    let total = 10000;
    let now = Instant::now();
    for index in 0..total {
        log::info!("Commencing yak shaving{}", index);
    }
    now.time(total);
    now.qps(total);
    may::coroutine::sleep(Duration::from_secs(1));
}