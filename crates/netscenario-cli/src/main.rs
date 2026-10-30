//! The `netscenario` program: connects [`netscenario_cli::run`] to the real arguments and streams.
//!
//! The program is also the supervisor of a run and the holder of a node (D-022): a run starts it again, inside
//! its namespaces, with `supervise` or `hold` as the first argument. Those two are not commands for people and
//! are not in the help.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args_os()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    if matches!(args.get(1).map(String::as_str), Some("supervise" | "hold")) {
        return netscenario_backend_kernel::supervisor::main(args.into_iter().skip(1));
    }
    let code = netscenario_cli::run(
        args,
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    );
    ExitCode::from(code)
}
