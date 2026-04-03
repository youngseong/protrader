use tracing_appender::rolling;
use tracing_subscriber::{fmt, filter::LevelFilter, layer::SubscriberExt, util::SubscriberInitExt};

/// Initialise tracing to stdout and a daily rotating file under `logs/`.
/// `level` controls the global minimum log level (e.g. `"info"`, `"debug"`, `"warn"`).
/// Returns the file-appender guard — keep alive for the program's duration.
pub fn init(level: &str) -> tracing_appender::non_blocking::WorkerGuard {
    let level_filter: LevelFilter = level.parse().unwrap_or(LevelFilter::INFO);

    std::fs::create_dir_all("logs").expect("failed to create logs/ directory");

    let file_appender = rolling::daily("logs", "protrader.log");
    let (non_blocking_file, guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::registry()
        .with(level_filter)
        .with(
            fmt::layer()
                .with_target(false)
                .with_ansi(false)
                .with_writer(non_blocking_file),
        )
        .with(fmt::layer().with_target(false).with_writer(std::io::stdout))
        .init();

    guard
}
