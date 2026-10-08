//! Local stdio MCP entrypoint. stdout is reserved for JSON-RPC.
use clap::Parser;

#[derive(Parser)]
#[command(
    name = "noh-mcp",
    version = noh::build_info::VERSION,
    about = "Local NOH media tools over MCP stdio"
)]
struct Options {
    /// Print the embedded build identity as JSON, without starting a server.
    #[arg(long, exclusive = true)]
    build_info: bool,
    /// Default FFmpeg executable (otherwise NOH_FFMPEG or local discovery).
    #[arg(long, value_name = "PATH")]
    ffmpeg: Option<std::ffi::OsString>,
}

fn main() {
    #[cfg(all(windows, feature = "updates"))]
    if let Some(code) = noh::update::lifecycle::hook_exit(std::env::args_os()) {
        std::process::exit(code);
    }
    #[cfg(all(windows, feature = "updates"))]
    let _update_lease = noh::update::windows::RuntimeLease::for_current_executable()
        .unwrap_or_else(|error| {
            eprintln!("noh-mcp: managed installation is unavailable: {error}");
            std::process::exit(1);
        });
    // Engine jobs execute this same binary. Dispatch before creating the MCP runtime.
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--engine-worker")
    {
        std::process::exit(noh::cli::run(std::env::args_os()));
    }
    let options = Options::parse();
    if options.build_info {
        println!("{}", noh::build_info::current().json());
        return;
    }
    let result = (|| {
        // Discovery/listing remains available without a media engine. An empty
        // default asks each media operation to repeat safe discovery; a bare
        // executable name would accidentally turn that absence into a choice.
        let ffmpeg = noh::find_ffmpeg(options.ffmpeg).unwrap_or_default();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let result = runtime.block_on(noh::mcp::run_stdio(ffmpeg));
        // Session cleanup has joined every owned job. Tokio's stdin reader can still
        // block while a client holds its pipe open; do not await that detached IO.
        runtime.shutdown_background();
        result
    })();
    if let Err(error) = result {
        eprintln!("noh-mcp: {error}");
        std::process::exit(1);
    }
}
