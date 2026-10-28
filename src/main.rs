fn main() {
    println!(
        "time-cron {} (tzdata {})",
        env!("CARGO_PKG_VERSION"),
        jiff_tzdb::VERSION.unwrap_or("unknown")
    );
}
