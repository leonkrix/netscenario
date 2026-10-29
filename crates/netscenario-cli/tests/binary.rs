//! Tests of the real binary: the exit code and the streams as a shell sees them.

use std::io;
use std::process::{Command, Output};

fn netscenario(args: &[&str]) -> io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_netscenario"))
        .args(args)
        .output()
}

#[test]
fn version_exits_with_zero() -> io::Result<()> {
    let output = netscenario(&["--version"])?;
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("netscenario "));
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    Ok(())
}

#[test]
fn no_command_exits_with_two() -> io::Result<()> {
    let output = netscenario(&[])?;
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    assert!(String::from_utf8_lossy(&output.stderr).contains("no command given"));
    Ok(())
}
