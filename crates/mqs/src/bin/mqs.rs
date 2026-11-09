//! `mqs` binary entry point.

fn main() {
    // Rust ignores SIGPIPE by default, which turns `mqs ... | head` into a
    // println! panic. Restore the usual Unix behaviour for a CLI.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("mqs: cannot start the async runtime: {err}");
            std::process::exit(1);
        }
    };
    let code = runtime.block_on(mqs::cli::main());
    runtime.shutdown_background();
    std::process::exit(code);
}
