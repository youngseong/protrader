use tracing_appender::rolling;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt};

/// Initialise tracing to stdout and a daily rotating file under `logs/`.
/// Returns the file-appender guard — keep alive for the program's duration.
pub fn init() -> tracing_appender::non_blocking::WorkerGuard {
    std::fs::create_dir_all("logs").expect("failed to create logs/ directory");

    let file_appender = rolling::daily("logs", "protrader.log");
    let (non_blocking_file, guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::registry()
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
