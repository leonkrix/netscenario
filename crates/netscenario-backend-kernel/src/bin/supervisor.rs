//! The binary that a run starts as its supervisor, and as the holder of a node. Tests use it; an application
//! calls [`netscenario_backend_kernel::supervisor::main`] from its own binary instead.

use std::process::ExitCode;

fn main() -> ExitCode {
    netscenario_backend_kernel::supervisor::main(std::env::args().skip(1))
}
