//! The process that owns a run (D-022). It is started by `unshare` as the first process of a PID namespace in
//! a network namespace of its own, so when it dies the kernel ends everything that belongs to the run. It
//! reads requests from its standard input, one JSON object per line, and answers each on its standard output.

use std::io::{BufRead, Write};
use std::process::ExitCode;
use std::time::Duration;

use crate::command::Toolbox;
use crate::protocol::{Request, Response};
use crate::provisioner::{Failpoints, ProgramSpec, Provisioner, disable_ipv6};

/// The environment variable that names a failpoint of the crash tests (see `provisioner::Failpoints`).
pub const FAIL_AT: &str = "NETSCENARIO_FAIL_AT";

/// The entry point of the helper binary. The arguments are `supervise <run id> <most nodes>` or `hold <run
/// id>`. The run id only appears in the command line, so that the processes of a run can be recognised.
pub fn main(arguments: impl Iterator<Item = String>) -> ExitCode {
    let arguments: Vec<String> = arguments.collect();
    match arguments
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["supervise", run_id, max_nodes] => match max_nodes.parse() {
            Ok(max_nodes) => supervise(run_id, max_nodes),
            Err(_) => fail(&format!("'{max_nodes}' is not a number of nodes")),
        },
        ["hold", _run_id] => hold(),
        ["sysctl", settings @ ..] if !settings.is_empty() => sysctl(settings),
        _ => {
            fail("usage: supervise <run id> <most nodes> | hold <run id> | sysctl <name=value>...")
        }
    }
}

fn fail(message: &str) -> ExitCode {
    let _ = writeln!(std::io::stderr(), "netscenario supervisor: {message}");
    ExitCode::FAILURE
}

fn send(response: &Response) {
    let mut out = std::io::stdout().lock();
    if let Ok(text) = serde_json::to_string(response) {
        let _ = writeln!(out, "{text}");
        let _ = out.flush();
    }
}

fn answer(result: Result<Option<u32>, String>) -> Response {
    match result {
        Ok(id) => Response::Ok { id },
        Err(message) => Response::Error { message },
    }
}

/// Holds a network namespace: the process that was started in it with `unshare --net` stays, and the
/// namespace lives as long as it does. It tells the supervisor when the namespace is ready.
fn hold() -> ExitCode {
    if let Err(message) = disable_ipv6() {
        return fail(&message);
    }
    let mut out = std::io::stdout().lock();
    if writeln!(out, "ready").and_then(|()| out.flush()).is_err() {
        return fail("the supervisor is gone");
    }
    drop(out);
    loop {
        std::thread::park();
    }
}

/// Writes kernel settings of the network namespace this process is in: `net.ipv4.ip_forward=1` writes `1` to
/// `/proc/sys/net/ipv4/ip_forward`. The backend starts it with `nsenter` in a node, because it has no shell and no
/// `sysctl` to rely on (D-078). Only names of the networking tables are accepted.
fn sysctl(settings: &[&str]) -> ExitCode {
    for setting in settings {
        let Some((name, value)) = setting.split_once('=') else {
            return fail(&format!("'{setting}' is not name=value"));
        };
        if !name.starts_with("net.")
            || name.split('.').any(|part| {
                part.is_empty()
                    || part == ".."
                    || !part
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            })
        {
            return fail(&format!("'{name}' is not the name of a network setting"));
        }
        let path = format!("/proc/sys/{}", name.replace('.', "/"));
        if let Err(error) = std::fs::write(&path, value) {
            return fail(&format!("cannot write {value} to {path}: {error}"));
        }
    }
    ExitCode::SUCCESS
}

/// Puts the signals that a program may be sent (`TERM` by default, and the others of `shutdown`) back to what a program
/// expects. The supervisor is the first process of a pid namespace and may have been started with `SIGTERM` and `SIGINT`
/// ignored (the `unshare` of util-linux 2.37, on Ubuntu 22.04, does). An ignored signal stays ignored across `exec`, so
/// every program would ignore `TERM` and none could be stopped but with `KILL` (D-072). A handler, unlike "ignored", is
/// reset to the default by `exec`; the flag that the handlers set is never read, because the supervisor ignored the
/// signals before as well.
fn restore_signals() {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGQUIT, SIGTERM, SIGUSR1, SIGUSR2};
    let unused = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    for signal in [SIGTERM, SIGINT, SIGHUP, SIGQUIT, SIGUSR1, SIGUSR2] {
        // A signal that cannot get a handler stays as it is; the test of the programs says if that matters
        let _ = signal_hook::flag::register(signal, std::sync::Arc::clone(&unused));
    }
}

// One arm for each request of the protocol: splitting it would only hide the list
#[allow(clippy::too_many_lines)]
fn supervise(run_id: &str, max_nodes: usize) -> ExitCode {
    restore_signals();
    let tools = match Toolbox::locate() {
        Ok(tools) => tools,
        Err(missing) => {
            send(&Response::Error {
                message: format!("the tools {} were not found", missing.join(", ")),
            });
            return ExitCode::FAILURE;
        }
    };
    let failpoints = Failpoints::new(std::env::var(FAIL_AT).ok().as_deref());
    let mut provisioner = match Provisioner::new(tools, run_id.to_owned(), max_nodes, failpoints) {
        Ok(provisioner) => provisioner,
        Err(message) => {
            send(&Response::Error { message });
            return ExitCode::FAILURE;
        }
    };
    if let Err(message) = provisioner.check_isolated() {
        send(&Response::Error { message });
        return ExitCode::FAILURE;
    }
    send(&Response::Ok { id: None });
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let request = match serde_json::from_str::<Request>(&line) {
            Ok(request) => request,
            Err(error) => {
                send(&Response::Error {
                    message: format!("cannot read the request: {error}"),
                });
                continue;
            }
        };
        let finished = request == Request::Teardown;
        let response = answer(match request {
            Request::CreateNode { name } => provisioner.create_node(&name).map(Some),
            Request::CreateLink { name, between } => {
                provisioner.create_link(&name, between).map(Some)
            }
            Request::SetNodeAddress { node, address } => {
                provisioner.set_node_address(node, &address).map(|()| None)
            }
            Request::SetMtu { link, mtu } => provisioner.set_mtu(link, mtu).map(|()| None),
            Request::SetForwarding { node, on } => {
                provisioner.set_forwarding(node, on).map(|()| None)
            }
            Request::AddRoute {
                node,
                v6,
                destination,
                metric,
                next_hops,
            } => provisioner
                .add_route(node, v6, destination.as_deref(), metric, &next_hops)
                .map(|()| None),
            Request::SetFamilies { v4, v6 } => provisioner.set_families(v4, v6).map(|()| None),
            Request::FinishNetwork => {
                provisioner.finish_network();
                Ok(None)
            }
            Request::Apply {
                link,
                direction,
                impairments,
                seed,
            } => provisioner
                .apply(link, direction, &impairments, seed)
                .map(|()| None),
            Request::Plan {
                link,
                direction,
                states,
            } => provisioner.plan(link, direction, &states).map(|()| None),
            Request::PlanOutage { link, direction } => {
                provisioner.plan_outage(link, direction).map(|()| None)
            }
            Request::SetOutage {
                link,
                direction,
                on,
            } => provisioner.set_outage(link, direction, on).map(|()| None),
            Request::Change {
                link,
                direction,
                impairments,
            } => provisioner
                .change(link, direction, &impairments)
                .map(|()| None),
            Request::SetLink { link, up } => provisioner.set_link(link, up).map(|()| None),
            Request::QueuePackets { link, direction } => {
                send(&match provisioner.queue_packets(link, direction) {
                    Ok(packets) => Response::Count { packets },
                    Err(message) => Response::Error { message },
                });
                continue;
            }
            Request::QueueBacklog { link, direction } => {
                send(&match provisioner.queue_backlog(link, direction) {
                    Ok(packets) => Response::Count { packets },
                    Err(message) => Response::Error { message },
                });
                continue;
            }
            Request::NodeCounts { link, direction } => {
                send(&match provisioner.node_counts(link, direction) {
                    Ok((frames_received, receive_buffer_drops)) => Response::NodeCounts {
                        frames_received,
                        receive_buffer_drops,
                    },
                    Err(message) => Response::Error { message },
                });
                continue;
            }
            Request::StartProgram {
                node,
                argv,
                memory_limit,
                env,
                cwd,
                own_group,
                stdout_file,
                stderr_file,
            } => provisioner
                .start_program(
                    node,
                    ProgramSpec {
                        argv,
                        memory_limit,
                        env,
                        cwd,
                        own_group,
                        stdout_file,
                        stderr_file,
                    },
                )
                .map(Some),
            Request::Signal { program, signal } => {
                provisioner.signal(program, signal).map(|()| None)
            }
            Request::PortBound { node, udp, port } => {
                send(&match provisioner.port_bound(node, udp, port) {
                    Ok(value) => Response::Bool { value },
                    Err(message) => Response::Error { message },
                });
                continue;
            }
            Request::Poll { program, wait_ms } => {
                send(
                    &match provisioner.poll(program, Duration::from_millis(wait_ms)) {
                        Ok(report) => Response::Program {
                            lines: report.lines,
                            exit: report.exit,
                            stderr: report.stderr,
                            truncated: report.truncated,
                            output: report.output,
                        },
                        Err(message) => Response::Error { message },
                    },
                );
                continue;
            }
            Request::SendLine { program, line } => {
                provisioner.send_line(program, &line).map(|()| None)
            }
            Request::Teardown => provisioner.teardown().map(|()| None),
            Request::Notes => {
                send(&Response::Notes {
                    lines: provisioner.notes(),
                });
                continue;
            }
        });
        send(&response);
        if finished {
            return ExitCode::SUCCESS;
        }
    }
    // The caller is gone without a teardown: clean up as far as we can, and the kernel does the rest
    let _ = provisioner.teardown();
    ExitCode::SUCCESS
}
