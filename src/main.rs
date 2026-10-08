fn main() {
    #[cfg(all(windows, feature = "updates"))]
    if let Some(code) = noh::update::lifecycle::hook_exit(std::env::args_os()) {
        std::process::exit(code);
    }
    #[cfg(all(windows, feature = "updates"))]
    let _update_lease = noh::update::windows::RuntimeLease::for_current_executable()
        .unwrap_or_else(|error| {
            eprintln!("noh: managed installation is unavailable: {error}");
            std::process::exit(1);
        });
    std::process::exit(noh::cli::run(std::env::args_os()));
}
