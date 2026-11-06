//! The program `netscenario-agent`: connects [`netscenario_agent::cli::run`] to the real arguments and streams.

use std::process::ExitCode;

fn main() -> ExitCode {
    // Before anything opens a file: a workload with a thousand connections needs more than the usual 1024
    let _ = netscenario_agent::limits::raise_open_files();
    let args: Vec<String> = std::env::args_os()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let code = netscenario_agent::cli::run(
        args,
        std::io::BufReader::new(std::io::stdin()),
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    );
    ExitCode::from(code)
}
