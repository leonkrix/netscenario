//! `netscenario doctor`: what this machine can do for the Kernel backend, and what it cannot (ENV-04).
//!
//! The work is in two steps so that the judging can be tested without a machine that is broken in the right
//! way: [`inspect`] looks at the machine (tools, the rights to create namespaces, what the kernel and `tc`
//! accept) and returns [`Findings`]; [`assess`] turns findings into a [`Report`] with one line per check and,
//! where something is missing, how to fix it. [`diagnose`] does both.
//!
//! The rights are not guessed from the user id: the probe runs the same `unshare` command as a run, once
//! as an unprivileged user namespace and once with the rights of the process, inside a namespace of its own,
//! so nothing of the host is touched (SAF-01).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use netscenario_core::{Feature, Support};
use serde::Serialize;

use crate::capabilities::{Environment, Probed, capabilities, capabilities_probed};
use crate::command::{Toolbox, find, has_own_rights, namespace_flags, run, searched};
use crate::tools::{Tool, Version, parse_tc_version};

/// The first release of iproute2 whose `tc` knows the `seed` of `netem`.
const SEED_SINCE: Version = Version::new(6, 6, 0);

/// How a check turned out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// As it should be.
    Ok,
    /// Works, but something is limited or may surprise; the check says what.
    Warn,
    /// Scenarios cannot run on this machine until it is fixed.
    Fail,
    /// For information: not tested, or an alternative that is not available.
    Info,
}

/// One line of the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    /// The group: `tools`, `rights`, `kernel` or `features`.
    pub section: &'static str,
    /// A stable name, for example `tool-tc` or `seed`.
    pub id: String,
    /// How it turned out.
    pub status: Status,
    /// A short name for people.
    pub title: String,
    /// What was found, in a sentence.
    pub detail: String,
    /// What to do about it, when something is not as it should be.
    pub fix: Option<String>,
}

/// The result of `doctor`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    /// The version of this form (the `--json` output).
    pub version: u32,
    /// Whether scenarios can run on this machine: no check failed.
    pub ok: bool,
    /// The checks in the order they are shown.
    pub checks: Vec<Check>,
}

/// The ways to get the rights for the namespaces of a run (D-022).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Way {
    /// In an unprivileged user namespace, where the process is root only inside.
    UserNamespace,
    /// With the rights of the process: root, or the capabilities `CAP_NET_ADMIN` and `CAP_SYS_ADMIN`.
    OwnRights,
}

impl Way {
    fn id(self) -> &'static str {
        match self {
            Way::UserNamespace => "way-user-namespace",
            Way::OwnRights => "way-own-rights",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Way::UserNamespace => "user namespace",
            Way::OwnRights => "own rights",
        }
    }
}

/// A tool as found on the machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolFinding {
    /// The name of the program.
    pub name: &'static str,
    /// Whether runs cannot work without it (`kill` is only needed by `clean`).
    pub required: bool,
    /// Where it was found.
    pub path: Option<PathBuf>,
    /// The first line of its version output, or what went wrong when asking.
    pub printed: String,
}

/// One thing that the probe tried inside a namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// `veth`, `netem`, `tables` or `seed`.
    pub name: String,
    /// Whether the command was accepted, or what it answered.
    pub result: Result<(), String>,
}

/// What came of trying one way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WayFinding {
    /// The way that was tried.
    pub way: Way,
    /// The steps the probe ran, or why the namespaces could not be created.
    pub outcome: Result<Vec<Step>, String>,
}

/// What is known about the system that explains a refusal. Each field is one fact, so they are flags.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct System {
    /// Whether the process has rights of its own for the namespaces of a run: it is root, or it has
    /// `CAP_NET_ADMIN` and `CAP_SYS_ADMIN` in its ambient set.
    pub own_rights: bool,
    /// `kernel.apparmor_restrict_unprivileged_userns` is 1 (Ubuntu 24.04 and newer).
    pub apparmor_restricts_userns: bool,
    /// `kernel.unprivileged_userns_clone` is 0 (some Debian and Arch kernels).
    pub userns_clone_disabled: bool,
    /// `user.max_user_namespaces` is 0.
    pub no_user_namespaces: bool,
    /// The process runs in a container.
    pub container: bool,
    /// The value of `TC_LIB_DIR`, where `tc` looks for the tables of the distributions.
    pub tc_lib_dir: Option<String>,
}

/// What `doctor` saw on the machine, before it judges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Findings {
    /// `ip`, `tc`, `unshare`, `nsenter` and `kill`.
    pub tools: Vec<ToolFinding>,
    /// The ways that were tried, in the order a run would try them. Empty if a tool is missing.
    pub ways: Vec<WayFinding>,
    /// Facts that explain a refusal.
    pub system: System,
}

/// Looks at the machine and judges what it finds.
#[must_use]
pub fn diagnose() -> Report {
    assess(&inspect())
}

/// What the probe runs inside the namespace: one line per step, `ok <name>` or `fail <name> <answer>`. `$1`
/// is `ip` and `$2` is `tc`. The interface lives in the probe's own network namespace and goes away with it.
const PROBE: &str = r#"
step() {
  name=$1; shift
  if out=$("$@" 2>&1); then echo "ok $name"; else echo "fail $name $(printf %s "$out" | tr '\n' ' ')"; fi
}
step veth "$1" link add np0 type veth peer name np1
# The stack of a node must not build segments larger than a frame for a link, or `netem` handles super-packets (D-086)
step segments "$1" link set np1 gso_max_size 1514 gso_max_segs 1
# netem keeps corrupt, reorder, rate and slot across a change, so every step starts from a bare interface (D-048)
step netem "$2" qdisc replace dev np0 root netem limit 1000 delay 1000us loss random 1% rate 1mbit
"$2" qdisc del dev np0 root >/dev/null 2>&1
step tables "$2" qdisc replace dev np0 root netem limit 1000 delay 1000us 500us distribution normal
step pareto "$2" qdisc replace dev np0 root netem limit 1000 delay 1000us 500us distribution pareto
step paretonormal "$2" qdisc replace dev np0 root netem limit 1000 delay 1000us 500us distribution paretonormal
# A table of our own (D-084): `tc` takes NAME.dist from the directory of TC_LIB_DIR
step tablecustom sh -c 'd=$(mktemp -d) || exit 1; yes 0 | head -n 4096 > "$d/probe.dist"; TC_LIB_DIR="$d" "$0" qdisc replace dev np0 root netem limit 1000 delay 1000us 500us distribution probe; r=$?; rm -rf "$d"; exit $r' "$2"
"$2" qdisc del dev np0 root >/dev/null 2>&1
step seed "$2" qdisc replace dev np0 root netem limit 1000 loss random 1% seed 1
"$2" qdisc del dev np0 root >/dev/null 2>&1
step burst "$2" qdisc replace dev np0 root netem limit 1000 loss gemodel 1% 20% 100% 0% seed 1
"$2" qdisc del dev np0 root >/dev/null 2>&1
step chain "$2" qdisc replace dev np0 root netem limit 1000 loss state 2% 30% 20% 10% 1% seed 1
"$2" qdisc del dev np0 root >/dev/null 2>&1
step reorder "$2" qdisc replace dev np0 root netem limit 1000 delay 1000us reorder 25% gap 3 seed 1
"$2" qdisc del dev np0 root >/dev/null 2>&1
step duplicate "$2" qdisc replace dev np0 root netem limit 1000 duplicate 20% seed 1
"$2" qdisc del dev np0 root >/dev/null 2>&1
step corrupt "$2" qdisc replace dev np0 root netem limit 1000 corrupt 10% seed 1
"$2" qdisc del dev np0 root >/dev/null 2>&1
# A duplicating netem next to another netem in one tree: some kernels refuse it (D-052). This is the tree that
# the backend builds for a direction with a rate and a jitter or a reordering.
step treeroot "$2" qdisc replace dev np0 root handle 1: netem limit 1000 duplicate 20% rate 1mbit
step tree "$2" qdisc replace dev np0 parent 1:1 handle 2: netem limit 1000 delay 1000us 500us
# A link with a queue is built through the fabric (D-079): a `netem` root with a delay of 1 microsecond, an `htb` class with the
# rate below it, the discipline as the leaf of the class, and in the fabric an ingress filter that redirects every frame to the
# other port. The tree is built as the backend builds it, and each discipline is tried as its leaf. The leaf is removed before the
# next one: a `replace` of the handle `3:` with a qdisc of another kind is refused by the kernel ("Invalid qdisc name").
"$2" qdisc del dev np0 root >/dev/null 2>&1
step fabric sh -c '"$0" qdisc add dev np0 ingress && "$0" filter add dev np0 parent ffff: protocol all u32 match u32 0 0 action mirred egress redirect dev np1' "$2"
step queuetree sh -c '"$0" qdisc replace dev np0 root handle 1: netem limit 1000 delay 1us && "$0" qdisc replace dev np0 parent 1:1 handle 2: htb default 1 r2q 10 && "$0" class replace dev np0 parent 2: classid 2:1 htb rate 1mbit ceil 1mbit burst 1600 cburst 1600 quantum 1514' "$2"
step pfifo "$2" qdisc replace dev np0 parent 2:1 handle 3: pfifo limit 100
"$2" qdisc del dev np0 parent 2:1 >/dev/null 2>&1
step codel "$2" qdisc replace dev np0 parent 2:1 handle 3: codel limit 100 target 5000us interval 100000us
"$2" qdisc del dev np0 parent 2:1 >/dev/null 2>&1
step fqcodel "$2" qdisc replace dev np0 parent 2:1 handle 3: fq_codel limit 100 flows 64 quantum 1514 target 5000us interval 100000us
"$2" qdisc del dev np0 parent 2:1 >/dev/null 2>&1
step pie "$2" qdisc replace dev np0 parent 2:1 handle 3: pie limit 100 target 15000us tupdate 15000us
"$2" qdisc del dev np0 root >/dev/null 2>&1
# A network of several nodes needs forwarding and routes with a next hop (D-078), and IPv6 needs a kernel that has it (D-081).
# The interface gets an address and comes up, so that the route has a connected route to the next hop.
step forwarding sh -c 'echo 1 > /proc/sys/net/ipv4/ip_forward'
"$1" addr add 10.250.0.1/30 dev np0 >/dev/null 2>&1
"$1" link set np0 up >/dev/null 2>&1
"$1" link set np1 up >/dev/null 2>&1
step route "$1" route add 10.251.0.1/32 via 10.250.0.2 dev np0
step ipv6 sh -c '[ -d /proc/sys/net/ipv6/conf ] || { echo "the kernel has no IPv6 (/proc/sys/net/ipv6 is missing)"; exit 1; }; "$0" -6 addr add fd00:250::1/64 dev np0 nodad' "$1"
# A stream without the checksum of UDP is sent through a raw socket, which needs CAP_NET_RAW (capability 13).
# The effective set of this shell is the one the agent gets: all of it in a user namespace, and in a run with the
# rights of the process what the process has. The low 32 bits are enough: the capability is among them.
caps=$(sed -n 's/^CapEff:[[:space:]]*//p' /proc/self/status)
if [ -n "$caps" ] && [ $(( 0x${caps#????????} >> 13 & 1 )) = 1 ]; then
  echo "ok rawsocket"
else
  echo "fail rawsocket CAP_NET_RAW is not among the effective capabilities of a node"
fi
"#;

fn read_trimmed(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_owned())
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_owned()
}

/// Looks at the machine: finds the tools, tries the ways to get namespaces and reads the facts that explain
/// a refusal. It creates nothing outside the probe's own namespace.
#[must_use]
pub fn inspect() -> Findings {
    let tools: Vec<ToolFinding> = [
        ("ip", true, "-V"),
        ("tc", true, "-V"),
        ("unshare", true, "--version"),
        ("nsenter", true, "--version"),
        ("prlimit", true, "--version"),
        ("kill", false, ""),
    ]
    .into_iter()
    .map(|(name, required, flag)| {
        let path = find(name);
        let printed = match (&path, flag) {
            (Some(path), flag) if !flag.is_empty() => {
                run(path, &[flag]).map_or_else(|error| error, |out| first_line(&out))
            }
            _ => String::new(),
        };
        ToolFinding {
            name,
            required,
            path,
            printed,
        }
    })
    .collect();
    let system = System {
        own_rights: has_own_rights(),
        apparmor_restricts_userns: read_trimmed(
            "/proc/sys/kernel/apparmor_restrict_unprivileged_userns",
        )
        .is_some_and(|value| value == "1"),
        userns_clone_disabled: read_trimmed("/proc/sys/kernel/unprivileged_userns_clone")
            .is_some_and(|value| value == "0"),
        no_user_namespaces: read_trimmed("/proc/sys/user/max_user_namespaces")
            .is_some_and(|value| value == "0"),
        container: Path::new("/.dockerenv").exists() || Path::new("/run/.containerenv").exists(),
        tc_lib_dir: std::env::var("TC_LIB_DIR").ok(),
    };
    let located = |name: &str| {
        tools
            .iter()
            .find(|t| t.name == name)
            .and_then(|t| t.path.clone())
    };
    let ways = match (located("unshare"), located("ip"), located("tc"), find("sh")) {
        (Some(unshare), Some(ip), Some(tc), Some(sh)) => {
            let order = if system.own_rights {
                [Way::OwnRights, Way::UserNamespace]
            } else {
                [Way::UserNamespace, Way::OwnRights]
            };
            order
                .into_iter()
                .map(|way| WayFinding {
                    way,
                    outcome: probe(way, &unshare, &sh, &ip, &tc),
                })
                .collect()
        }
        _ => Vec::new(),
    };
    Findings {
        tools,
        ways,
        system,
    }
}

/// Runs the probe in the namespaces of `way`, with the flags of a run.
fn probe(way: Way, unshare: &Path, sh: &Path, ip: &Path, tc: &Path) -> Result<Vec<Step>, String> {
    let mut arguments: Vec<String> = namespace_flags(way == Way::UserNamespace)
        .into_iter()
        .map(String::from)
        .collect();
    arguments.push("--".to_owned());
    arguments.push(sh.display().to_string());
    arguments.extend(["-c", PROBE, "probe"].map(String::from));
    arguments.push(ip.display().to_string());
    arguments.push(tc.display().to_string());
    let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
    run(unshare, &arguments).map(|out| parse_probe(&out))
}

/// Reads the lines that the probe printed.
#[must_use]
pub fn parse_probe(output: &str) -> Vec<Step> {
    output
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, ' ');
            let (status, name) = (parts.next()?, parts.next()?);
            let answer = parts.next().unwrap_or("").trim().to_owned();
            match status {
                "ok" => Some(Ok(())),
                "fail" => Some(Err(answer)),
                _ => None,
            }
            .map(|result| Step {
                name: name.to_owned(),
                result,
            })
        })
        .collect()
}

fn check(
    section: &'static str,
    id: &str,
    status: Status,
    title: &str,
    detail: impl Into<String>,
    fix: Option<String>,
) -> Check {
    Check {
        section,
        id: id.to_owned(),
        status,
        title: title.to_owned(),
        detail: detail.into(),
        fix,
    }
}

/// The reason of a failed command without the part that says what was run: what the tool answered.
fn answer_of(error: &str) -> &str {
    error
        .rsplit_once("): ")
        .map_or(error, |(_, answer)| answer)
        .trim()
}

/// What to do when no way to get namespaces works.
fn how_to_get_namespaces(system: &System) -> String {
    let mut options = Vec::new();
    if system.apparmor_restricts_userns {
        options.push(
            "allow unprivileged user namespaces (Ubuntu 24.04 and newer restrict them): `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0`, and to keep it after a reboot put `kernel.apparmor_restrict_unprivileged_userns=0` in a file in /etc/sysctl.d/"
                .to_owned(),
        );
    }
    if system.userns_clone_disabled {
        options.push(
            "allow unprivileged user namespaces: `sudo sysctl -w kernel.unprivileged_userns_clone=1`"
                .to_owned(),
        );
    }
    if system.no_user_namespaces {
        options.push(
            "allow user namespaces: `sudo sysctl -w user.max_user_namespaces=15000`".to_owned(),
        );
    }
    if system.container {
        options.push(
            "start the container with the capability SYS_ADMIN and a seccomp profile that allows `unshare` (for example `docker run --cap-add SYS_ADMIN --security-opt seccomp=unconfined`)"
                .to_owned(),
        );
    }
    options.push(if system.own_rights {
        "this process has the rights of root (it is root, or has CAP_NET_ADMIN and CAP_SYS_ADMIN as ambient capabilities) and is still refused, so the kernel or a security module forbids namespaces here (see the answers above)"
            .to_owned()
    } else {
        "run netscenario as root (`sudo`), or with CAP_NET_ADMIN and CAP_SYS_ADMIN as ambient capabilities (`setpriv --ambient-caps`, `AmbientCapabilities=` in systemd), which need no user namespace"
            .to_owned()
    });
    options.join("; or ")
}

/// The lines about the tools, and the `tc` that was found.
fn tool_checks(findings: &Findings, checks: &mut Vec<Check>) -> Tool {
    let mut tc = Tool::Missing;
    for tool in &findings.tools {
        let id = format!("tool-{}", tool.name);
        let package = match tool.name {
            "ip" | "tc" => "iproute2",
            _ => "util-linux",
        };
        if let Some(path) = &tool.path {
            let detail = if tool.printed.is_empty() {
                path.display().to_string()
            } else {
                format!("{} ({})", path.display(), tool.printed)
            };
            checks.push(check("tools", &id, Status::Ok, tool.name, detail, None));
            if tool.name == "tc" {
                tc = parse_tc_version(&tool.printed)
                    .map_or_else(|| Tool::Unreadable(tool.printed.clone()), Tool::Found);
            }
        } else {
            {
                let (status, consequence) = if tool.required {
                    (Status::Fail, "")
                } else {
                    (Status::Warn, "; `netscenario clean` needs it")
                };
                checks.push(check(
                    "tools",
                    &id,
                    status,
                    tool.name,
                    format!("not found; looked in {}{consequence}", searched()),
                    Some(format!("install the package `{package}`")),
                ));
            }
        }
    }
    tc
}

/// The lines about the ways to get namespaces.
fn rights_checks(findings: &Findings, checks: &mut Vec<Check>) {
    let tools_missing = findings
        .tools
        .iter()
        .any(|t| t.required && t.path.is_none());
    // The ways to get namespaces
    let working: Vec<&WayFinding> = findings.ways.iter().filter(|w| w.outcome.is_ok()).collect();
    for way in [Way::UserNamespace, Way::OwnRights] {
        let tried = findings.ways.iter().find(|w| w.way == way);
        let (status, detail) = match tried.map(|w| &w.outcome) {
            Some(Ok(_)) => (Status::Ok, "works".to_owned()),
            Some(Err(error)) => (Status::Info, format!("not available: {}", answer_of(error))),
            None => (
                Status::Info,
                "not tested because a tool is missing".to_owned(),
            ),
        };
        checks.push(check("rights", way.id(), status, way.title(), detail, None));
    }
    if working.is_empty() && !tools_missing {
        checks.push(check(
            "rights",
            "can-create-namespaces",
            Status::Fail,
            "network namespaces",
            "no way to create the network namespaces of a run works on this machine",
            Some(how_to_get_namespaces(&findings.system)),
        ));
    } else if !working.is_empty() {
        let first = working[0].way;
        checks.push(check(
            "rights",
            "can-create-namespaces",
            Status::Ok,
            "network namespaces",
            format!("a run uses the {}", first.title()),
            None,
        ));
    }
}

/// What one step of the probe came to: `None` if it was not tested.
type Outcome = Option<Result<(), String>>;

fn kernel(id: &str, title: &str, status: Status, detail: &str, fix: Option<String>) -> Check {
    check("kernel", id, status, title, detail, fix)
}

fn veth_check(outcome: &Outcome, not_tested: &str) -> Check {
    let title = "virtual Ethernet pair";
    match outcome {
        Some(Ok(())) => kernel("veth", title, Status::Ok, "a pair can be created", None),
        Some(Err(error)) => kernel(
            "veth",
            title,
            Status::Fail,
            error,
            Some("the kernel needs the module `veth` (`sudo modprobe veth`)".to_owned()),
        ),
        None => kernel("veth", title, Status::Info, not_tested, None),
    }
}

fn netem_check(outcome: &Outcome, not_tested: &str) -> Check {
    match outcome {
        Some(Ok(())) => kernel("netem", "netem", Status::Ok, "delay, loss and rate are accepted", None),
        Some(Err(error)) => kernel(
            "netem",
            "netem",
            Status::Fail,
            error,
            Some("the kernel needs the module `sch_netem` (`sudo modprobe sch_netem`); on Ubuntu cloud kernels it is in the package `linux-modules-extra-$(uname -r)`".to_owned()),
        ),
        None => kernel("netem", "netem", Status::Info, not_tested, None),
    }
}

/// The tables of iproute2 that the distributions of a jitter come from: one outcome for each of `normal`, `pareto` and
/// `paretonormal`.
fn tables_check(
    outcomes: [(&str, &Outcome); 3],
    tc_lib_dir: Option<&str>,
    not_tested: &str,
) -> Check {
    let title = "jitter tables";
    if outcomes.iter().all(|(_, outcome)| outcome.is_none()) {
        return kernel("tables", title, Status::Info, not_tested, None);
    }
    let missing: Vec<(&str, &String)> = outcomes
        .iter()
        .filter_map(|(name, outcome)| match outcome {
            Some(Err(error)) => Some((*name, error)),
            _ => None,
        })
        .collect();
    if missing.is_empty() {
        return kernel(
            "tables",
            title,
            Status::Ok,
            "the distributions `normal`, `pareto` and `paretonormal` are found",
            None,
        );
    }
    let names: Vec<String> = missing
        .iter()
        .map(|(name, _)| format!("`{name}`"))
        .collect();
    let first = missing[0].1;
    kernel(
        "tables",
        title,
        Status::Warn,
        &format!(
            "{first}; a jitter with the distribution {} fails",
            names.join(" or ")
        ),
        Some(match tc_lib_dir {
            Some(dir) => format!(
                "TC_LIB_DIR is set to {dir}: put {}.dist there or point it to the directory that has it",
                missing[0].0
            ),
            None => format!(
                "install the iproute2 package that has the tables (usually in /usr/lib/tc or /usr/lib/<arch>/tc), or set TC_LIB_DIR to the directory with {}.dist (a tc built from source looks in /usr/lib/tc)",
                missing[0].0
            ),
        }),
    )
}

/// A table of our own, loaded from the directory of `TC_LIB_DIR` (D-084).
fn custom_tables_check(outcome: &Outcome, not_tested: &str) -> Check {
    let title = "jitter tables from samples";
    match outcome {
        Some(Ok(())) => kernel(
            "custom-tables",
            title,
            Status::Ok,
            "`tc` loads a table from the directory of TC_LIB_DIR",
            None,
        ),
        Some(Err(error)) => kernel(
            "custom-tables",
            title,
            Status::Warn,
            &format!("{error}; a jitter made from a file of samples fails"),
            Some("a scenario with `distribution: table` is refused here; the tools `mktemp` and `yes` are needed for this test, and a `tc` that ignores TC_LIB_DIR cannot load the table".to_owned()),
        ),
        None => kernel("custom-tables", title, Status::Info, not_tested, None),
    }
}

fn seed_check(tc: &Tool, outcome: &Outcome, not_tested: &str) -> Check {
    let title = "seed of netem";
    let too_old = match tc {
        Tool::Found(version) if *version < SEED_SINCE => Some(format!(
            "iproute2 {version} does not know `seed`, it starts with {SEED_SINCE}"
        )),
        Tool::Found(_) => None,
        Tool::Missing => Some("`tc` was not found".to_owned()),
        Tool::Unreadable(printed) => Some(format!(
            "the version of `tc` cannot be read (it printed '{printed}')"
        )),
    };
    match (too_old, outcome) {
        (Some(why), _) => kernel(
            "seed",
            title,
            Status::Warn,
            &format!(
                "{why}; a scenario with `seed` is refused, and one without is not reproducible"
            ),
            Some(format!("install iproute2 {SEED_SINCE} or newer")),
        ),
        (None, Some(Ok(()))) => kernel(
            "seed",
            title,
            Status::Ok,
            "`tc` accepts a seed, so the same seed gives the same lost packets",
            None,
        ),
        (None, Some(Err(error))) => kernel(
            "seed",
            title,
            Status::Warn,
            &format!(
                "`tc` or the kernel refuses a seed: {error}; a scenario with `seed` is refused by the run"
            ),
            Some(format!(
                "use iproute2 {SEED_SINCE} or newer on a kernel that accepts the seed (the answer above says what was refused)"
            )),
        ),
        (None, None) => kernel("seed", title, Status::Info, not_tested, None),
    }
}

/// The lines about the kernel and `tc`, from the first way that works.
fn kernel_checks(findings: &Findings, tc: &Tool, checks: &mut Vec<Check>) {
    let steps = findings.ways.iter().find_map(|w| w.outcome.as_ref().ok());
    let step = |name: &str| -> Outcome {
        steps.and_then(|steps| {
            steps.iter().find(|s| s.name == name).map(|s| {
                s.result
                    .clone()
                    .map_err(|error| answer_of(&error).to_owned())
            })
        })
    };
    let not_tested = if steps.is_none() {
        "not tested: no way to create namespaces works"
    } else {
        "not tested"
    };
    let veth = step("veth");
    // Without an interface the later steps cannot say anything about netem
    let after_veth = |name: &str| -> Outcome {
        if steps.is_some() && !matches!(veth, Some(Ok(()))) {
            Some(Err("not tested: no interface to put it on".to_owned()))
        } else {
            step(name)
        }
    };
    checks.push(veth_check(&veth, not_tested));
    checks.push(netem_check(&after_veth("netem"), not_tested));
    checks.push(tables_check(
        [
            ("normal", &after_veth("tables")),
            ("pareto", &after_veth("pareto")),
            ("paretonormal", &after_veth("paretonormal")),
        ],
        findings.system.tc_lib_dir.as_deref(),
        not_tested,
    ));
    checks.push(custom_tables_check(&after_veth("tablecustom"), not_tested));
    checks.push(seed_check(tc, &after_veth("seed"), not_tested));
    checks.push(models_check(
        &after_veth("burst"),
        &after_veth("chain"),
        &after_veth("reorder"),
        &after_veth("duplicate"),
        &after_veth("corrupt"),
        not_tested,
    ));
    checks.push(tree_check(
        &after_veth("treeroot"),
        &after_veth("tree"),
        not_tested,
    ));
    checks.push(queues_check(
        &after_veth("fabric"),
        &after_veth("queuetree"),
        not_tested,
    ));
    checks.push(disciplines_check(
        &after_veth("queuetree"),
        &[
            ("fifo", after_veth("pfifo")),
            ("codel", after_veth("codel")),
            ("fq-codel", after_veth("fqcodel")),
            ("pie", after_veth("pie")),
        ],
        not_tested,
    ));
    checks.push(network_check(
        &after_veth("forwarding"),
        &after_veth("route"),
        not_tested,
    ));
    checks.push(ipv6_check(&after_veth("ipv6"), not_tested));
    checks.push(segments_check(&after_veth("segments"), not_tested));
    checks.push(raw_socket_check(&step("rawsocket"), not_tested));
}

/// The tree of a link with a queue (D-079): the redirect of the fabric, and the `netem`, `htb` and class of the node.
fn queues_check(fabric: &Outcome, tree: &Outcome, not_tested: &str) -> Check {
    let title = "queues of a link";
    match (fabric, tree) {
        (Some(Ok(())), Some(Ok(()))) => kernel(
            "queues",
            title,
            Status::Ok,
            "the fabric (a redirect between two ports) and the tree of a queue (`netem`, `htb`, a class) can be built, so a direction can have a `queue`",
            None,
        ),
        (Some(Err(error)), _) | (_, Some(Err(error))) => kernel(
            "queues",
            title,
            Status::Warn,
            error,
            Some("scenarios with a `queue` are refused here; the kernel needs the modules `sch_htb`, `cls_u32`, `act_mirred` and `sch_netem`".to_owned()),
        ),
        _ => kernel("queues", title, Status::Info, not_tested, None),
    }
}

/// The disciplines of a queue (D-082): each is tried as the leaf of the class. A discipline the kernel refuses is refused by the
/// scenario that asks for it.
fn disciplines_check(tree: &Outcome, found: &[(&str, Outcome)], not_tested: &str) -> Check {
    let title = "queue disciplines";
    // A discipline is tried as the leaf of the tree, so without the tree it cannot be said (and is not the fault of the discipline)
    if !matches!(tree, Some(Ok(()))) {
        return kernel(
            "disciplines",
            title,
            Status::Info,
            "not tested: the tree of a queue cannot be built (see above)",
            None,
        );
    }
    if found.iter().all(|(_, outcome)| outcome.is_none()) {
        return kernel("disciplines", title, Status::Info, not_tested, None);
    }
    let refused: Vec<String> = found
        .iter()
        .filter_map(|(name, outcome)| match outcome {
            Some(Err(error)) => Some(format!("{name}: {error}")),
            _ => None,
        })
        .collect();
    if refused.is_empty() {
        kernel(
            "disciplines",
            title,
            Status::Ok,
            "fifo, CoDel, FQ-CoDel and PIE are accepted",
            None,
        )
    } else {
        kernel(
            "disciplines",
            title,
            Status::Warn,
            &refused.join("; "),
            Some("a scenario with a discipline that is refused here is refused before the network is built; the kernel needs the modules `sch_codel`, `sch_fq_codel` and `sch_pie`".to_owned()),
        )
    }
}

/// Forwarding and a route with a next hop in a node: what a network of several nodes needs (D-078).
fn network_check(forwarding: &Outcome, route: &Outcome, not_tested: &str) -> Check {
    let title = "forwarding and routes in a node";
    match (forwarding, route) {
        (Some(Ok(())), Some(Ok(()))) => kernel(
            "network",
            title,
            Status::Ok,
            "a node can forward packets and have a route through a next hop, so networks of several nodes can be built",
            None,
        ),
        (Some(Err(error)), _) | (_, Some(Err(error))) => kernel(
            "network",
            title,
            Status::Fail,
            error,
            Some("a node needs `net.ipv4.ip_forward` and `ip route add ... via ...`; scenarios with several nodes or routers are refused here, point to point still runs".to_owned()),
        ),
        _ => kernel("network", title, Status::Info, not_tested, None),
    }
}

/// The limit on the size of the segments of a link (D-086). Every run sets it on every interface, so a machine that refuses it
/// cannot run any scenario.
fn segments_check(outcome: &Outcome, not_tested: &str) -> Check {
    let title = "segment size of a link";
    match outcome {
        Some(Ok(())) => kernel(
            "segments",
            title,
            Status::Ok,
            "`ip` sets the largest segment of an interface, so that TCP is not handled by `netem` as super-packets",
            None,
        ),
        Some(Err(error)) => kernel(
            "segments",
            title,
            Status::Fail,
            error,
            Some("`ip link set <if> gso_max_size <n> gso_max_segs 1` is refused; the iproute2 of the machine is too old (4.12 or newer is needed)".to_owned()),
        ),
        None => kernel("segments", title, Status::Info, not_tested, None),
    }
}

/// IPv6 in a node (D-081). A machine without it still runs every scenario that does not ask for `ip: v6` or `ip: dual`, so it
/// is a warning and not a failure.
fn ipv6_check(outcome: &Outcome, not_tested: &str) -> Check {
    let title = "IPv6 in a node";
    match outcome {
        Some(Ok(())) => kernel("ipv6", title, Status::Ok, "a node can have IPv6 addresses", None),
        Some(Err(error)) => kernel(
            "ipv6",
            title,
            Status::Warn,
            error,
            Some("scenarios with `ip: v6` or `ip: dual` are refused here; the kernel needs IPv6 (not `ipv6.disable=1`)".to_owned()),
        ),
        None => kernel("ipv6", title, Status::Info, not_tested, None),
    }
}

/// The capability that a stream without the checksum of UDP needs (`udp-checksum: false`): the sender writes
/// the header of UDP itself, on a raw socket. The receiver that names damaged packets reads the interface with a
/// packet socket, which needs the same capability.
fn raw_socket_check(outcome: &Outcome, not_tested: &str) -> Check {
    let title = "raw socket in a node";
    match outcome {
        Some(Ok(())) => kernel(
            "raw-socket",
            title,
            Status::Ok,
            "a node has CAP_NET_RAW, so a stream can be sent without the checksum of UDP, and the damaged packets of a link with `corrupt` can be named by sequence number",
            None,
        ),
        Some(Err(error)) => kernel(
            "raw-socket",
            title,
            Status::Warn,
            &format!(
                "{error}; a workload with `udp-checksum: false` is refused by the run, and the damaged packets of a link with `corrupt` are counted but not named by sequence number"
            ),
            Some("run netscenario as root, or with CAP_NET_RAW among the ambient capabilities (`setpriv --ambient-caps +net_raw`, `AmbientCapabilities=` in systemd), or in a user namespace, which has it".to_owned()),
        ),
        None => kernel("raw-socket", title, Status::Info, not_tested, None),
    }
}

/// Whether a node of this machine has no `CAP_NET_RAW`, from the steps of the probe. `Some(answer)` if it has not;
/// `None` if it has, or if the question cannot be answered (the probe could not run): then the sender finds out for
/// itself and its error names the capability.
pub(crate) fn raw_socket_answer(steps: &[Step]) -> Option<String> {
    match steps.iter().find(|step| step.name == "rawsocket")?.result {
        Err(ref error) => Some(answer_of(error).to_owned()),
        Ok(()) => None,
    }
}

/// Runs the probe for the way that a run takes (`own_rights`, or a user namespace), in a throwaway namespace. `None` if
/// it cannot be run (no `sh`, or no way to get namespaces): then the backend declares what the version of `tc` says,
/// and the run finds out for itself (D-068).
pub(crate) fn probe_for_run(tools: &Toolbox, own_rights: bool) -> Option<Vec<Step>> {
    let sh = find("sh")?;
    let way = if own_rights {
        Way::OwnRights
    } else {
        Way::UserNamespace
    };
    probe(way, &tools.unshare, &sh, &tools.ip, &tools.tc).ok()
}

/// The loss models in bursts, the reordering, the duplication and the corruption: one qdisc each.
fn models_check(
    burst: &Outcome,
    chain: &Outcome,
    reorder: &Outcome,
    duplicate: &Outcome,
    corrupt: &Outcome,
    not_tested: &str,
) -> Check {
    let title = "burst loss, reordering, duplication, corruption";
    let steps = [
        ("the Gilbert-Elliott loss", burst),
        ("the four-state loss", chain),
        ("reordering", reorder),
        ("duplication", duplicate),
        ("corruption", corrupt),
    ];
    if steps.iter().all(|(_, outcome)| outcome.is_none()) {
        return kernel("models", title, Status::Info, not_tested, None);
    }
    let refused: Vec<String> = steps
        .iter()
        .filter_map(|(what, outcome)| match outcome {
            Some(Err(error)) => Some(format!("{what}: {error}")),
            _ => None,
        })
        .collect();
    if refused.is_empty() {
        kernel(
            "models",
            title,
            Status::Ok,
            "`tc` and the kernel accept the loss in bursts (two models), reordering, duplication and corruption",
            None,
        )
    } else {
        kernel(
            "models",
            title,
            Status::Warn,
            &format!(
                "refused: {}; a scenario that uses them is refused by the run",
                refused.join("; ")
            ),
            Some("use a current iproute2 on a kernel with the `netem` models (the answers above say what was refused)".to_owned()),
        )
    }
}

/// A duplicating `netem` next to another `netem` in one tree (D-052): the tree of a direction with a rate and a
/// jitter or a reordering. The kernels that refuse it say "cannot mix duplicating netems with other netems in tree".
fn tree_check(root: &Outcome, tree: &Outcome, not_tested: &str) -> Check {
    let title = "duplication with a jitter or reordering and a rate";
    match (root, tree) {
        (Some(Ok(())), Some(Ok(()))) => kernel(
            "duplicate-tree",
            title,
            Status::Ok,
            "the kernel accepts a duplicating `netem` next to another `netem` in one tree",
            None,
        ),
        (Some(Ok(())), Some(Err(error))) => kernel(
            "duplicate-tree",
            title,
            Status::Warn,
            &format!(
                "this kernel refuses a duplicating `netem` next to another one: {error}; a direction with `duplicate` and a `rate` and a `jitter` or `reorder` is refused by the run"
            ),
            Some("leave out the jitter or the reordering, or the rate, or the duplication in that direction, or use a kernel that accepts it (the check was added in 2025 and removed again later)".to_owned()),
        ),
        (Some(Err(_)), _) => kernel(
            "duplicate-tree",
            title,
            Status::Info,
            "not tested: the first queue of the tree is refused already (see above)",
            None,
        ),
        _ => kernel("duplicate-tree", title, Status::Info, not_tested, None),
    }
}

/// Whether this kernel refuses the tree of a duplicating `netem` with another one (D-052), from the steps of the probe.
/// `Some(answer)` is the answer of `tc`; `None` if the kernel accepts it, or if the
/// question cannot be answered (the probe cannot run, or the first queue of the tree is refused already): then the run
/// finds out for itself and reports what `tc` says.
pub(crate) fn duplicate_tree_answer(steps: &[Step]) -> Option<String> {
    let result = |name: &str| {
        steps
            .iter()
            .find(|step| step.name == name)
            .map(|s| s.result.clone())
    };
    match (result("treeroot"), result("tree")) {
        (Some(Ok(())), Some(Err(error))) => Some(answer_of(&error).to_owned()),
        _ => None,
    }
}

/// The features, as the backend declares them for this machine.
fn feature_checks(tc: Tool, probed: &Probed, checks: &mut Vec<Check>) {
    let declared = capabilities_probed(&Environment { tc }, probed);
    let complete = capabilities(&Environment::complete());
    for feature in Feature::ALL {
        let (status, detail, fix) = match declared.support(*feature) {
            Support::Exact => (Status::Ok, "exact".to_owned(), None),
            // A restriction of the backend itself is information on every machine; one that this machine adds is a warning
            Support::Limited { restriction } => {
                let by_design = complete.support(*feature) == declared.support(*feature);
                let status = if by_design {
                    Status::Info
                } else {
                    Status::Warn
                };
                (status, format!("only in part: {restriction}"), None)
            }
            // Not on a complete machine either: the backend does not have it yet, which is no fault of this
            // machine and nothing to fix on it
            Support::Unsupported { reason, fix } => {
                let missing_here = complete.support(*feature) != declared.support(*feature);
                if missing_here {
                    (Status::Warn, reason, Some(fix))
                } else {
                    (Status::Info, reason, None)
                }
            }
        };
        checks.push(check(
            "features",
            feature.id(),
            status,
            feature.label(),
            detail,
            fix,
        ));
    }
}

/// Judges the findings. No I/O, so it can be tested with findings of machines that are broken in every way.
#[must_use]
pub fn assess(findings: &Findings) -> Report {
    let mut checks = Vec::new();
    let tc = tool_checks(findings, &mut checks);
    rights_checks(findings, &mut checks);
    kernel_checks(findings, &tc, &mut checks);
    // The features as a run would declare them: by the version of `tc` and by what the probe of the first way that
    // works found out (D-068)
    let probed = findings
        .ways
        .iter()
        .find_map(|way| way.outcome.as_ref().ok())
        .map_or_else(Probed::default, |steps| Probed::new(steps.clone()));
    feature_checks(tc, &probed, &mut checks);
    Report {
        version: 1,
        ok: checks.iter().all(|c| c.status != Status::Fail),
        checks,
    }
}

impl Report {
    /// The report as text for people.
    #[must_use]
    pub fn render(&self) -> String {
        let mut text = String::from("netscenario doctor\n");
        for (section, heading) in [
            ("tools", "Tools"),
            ("rights", "Rights to create namespaces"),
            ("kernel", "Kernel and tc"),
            ("features", "Features of the Kernel backend on this machine"),
        ] {
            let _ = writeln!(text, "\n{heading}");
            for check in self.checks.iter().filter(|c| c.section == section) {
                let label = match check.status {
                    Status::Ok => "ok",
                    Status::Warn => "warn",
                    Status::Fail => "FAIL",
                    Status::Info => "info",
                };
                let _ = writeln!(text, "  {label:<5} {:<30} {}", check.title, check.detail);
                if let Some(fix) = &check.fix {
                    let _ = writeln!(text, "        fix: {fix}");
                }
            }
        }
        let count = |status| self.checks.iter().filter(|c| c.status == status).count();
        let (fails, warns) = (count(Status::Fail), count(Status::Warn));
        let _ = write!(
            text,
            "\n{}\n",
            if self.ok {
                format!("Result: scenarios can run here ({warns} warnings).")
            } else {
                format!(
                    "Result: scenarios cannot run here ({fails} problems to fix, {warns} warnings)."
                )
            }
        );
        text
    }

    /// What the backend reproduces on this machine, as text for people: one line for each feature (`netscenario capabilities`).
    #[must_use]
    pub fn render_features(&self) -> String {
        let mut text =
            String::from("netscenario capabilities: the Kernel backend on this machine\n\n");
        for check in self.checks.iter().filter(|c| c.section == "features") {
            let _ = writeln!(text, "  {:<34} {}", check.title, check.detail);
            if let Some(fix) = &check.fix {
                let _ = writeln!(text, "  {:<34} fix: {fix}", "");
            }
        }
        let _ = write!(
            text,
            "\n{}\n",
            if self.ok {
                "Scenarios can run here."
            } else {
                "Scenarios cannot run here yet: `netscenario doctor` says what to fix."
            }
        );
        text
    }

    /// What the backend reproduces on this machine, as JSON (`netscenario capabilities --json`).
    #[must_use]
    pub fn features_json(&self) -> String {
        let features: Vec<_> = self
            .checks
            .iter()
            .filter(|c| c.section == "features")
            .map(|c| {
                serde_json::json!({
                    "id": c.id,
                    "title": c.title,
                    "status": c.status,
                    "detail": c.detail,
                    "fix": c.fix,
                })
            })
            .collect();
        serde_json::to_string_pretty(&serde_json::json!({
            "version": 1,
            "backend": "kernel",
            "ok": self.ok,
            "features": features,
        }))
        .unwrap_or_else(|error| format!("{{\"error\": \"{error}\"}}"))
    }

    /// The report as JSON, for machines.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self)
            .unwrap_or_else(|error| format!("{{\"error\": \"{error}\"}}"))
    }

    /// The check with the id `id`.
    #[must_use]
    pub fn check(&self, id: &str) -> Option<&Check> {
        self.checks.iter().find(|c| c.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &'static str, printed: &str) -> ToolFinding {
        ToolFinding {
            name,
            required: name != "kill",
            path: Some(PathBuf::from(format!("/usr/sbin/{name}"))),
            printed: printed.to_owned(),
        }
    }

    fn tools(tc: &str) -> Vec<ToolFinding> {
        vec![
            tool("ip", "ip utility, iproute2-6.15.0"),
            tool("tc", tc),
            tool("unshare", "unshare from util-linux 2.41"),
            tool("nsenter", "nsenter from util-linux 2.41"),
            tool("kill", ""),
        ]
    }

    fn steps(failing: &[(&str, &str)]) -> Vec<Step> {
        [
            "veth",
            "netem",
            "tables",
            "pareto",
            "paretonormal",
            "tablecustom",
            "seed",
            "burst",
            "chain",
            "reorder",
            "duplicate",
            "corrupt",
            "treeroot",
            "tree",
            "fabric",
            "queuetree",
            "pfifo",
            "codel",
            "fqcodel",
            "pie",
            "forwarding",
            "route",
            "ipv6",
            "segments",
            "rawsocket",
        ]
        .into_iter()
        .map(|name| Step {
            name: name.to_owned(),
            result: failing
                .iter()
                .find(|(n, _)| *n == name)
                .map_or(Ok(()), |(_, why)| Err((*why).to_owned())),
        })
        .collect()
    }

    fn good() -> Findings {
        Findings {
            tools: tools("tc utility, iproute2-6.15.0, libbpf 1.5.0"),
            ways: vec![
                WayFinding {
                    way: Way::UserNamespace,
                    outcome: Ok(steps(&[])),
                },
                WayFinding {
                    way: Way::OwnRights,
                    outcome: Err("`unshare --net` failed (exit status: 1): unshare: unshare failed: Operation not permitted".to_owned()),
                },
            ],
            system: System::default(),
        }
    }

    fn status(report: &Report, id: &str) -> Status {
        report
            .check(id)
            .map(|c| c.status)
            .expect("the check exists")
    }

    #[test]
    fn a_good_machine_passes_with_no_warning() {
        let report = assess(&good());
        assert!(report.ok);
        assert!(
            report
                .checks
                .iter()
                .all(|c| matches!(c.status, Status::Ok | Status::Info)),
            "{}",
            report.render()
        );
        // The way that is not available is information, not a problem
        assert_eq!(status(&report, "way-own-rights"), Status::Info);
        assert_eq!(status(&report, "way-user-namespace"), Status::Ok);
    }

    /// A feature that this machine lacks (here: `tc` is too old for the seed) is a warning, and one that it has is
    /// fine. (A feature that the backend does not have yet would be information on every machine; there is none now.)
    #[test]
    fn a_feature_the_machine_lacks_is_a_warning_and_the_others_are_fine() {
        let report = assess(&good());
        assert_eq!(status(&report, "program"), Status::Ok);
        assert_eq!(status(&report, "burst-loss"), Status::Ok);
        let mut old = good();
        old.tools[1].printed = "tc utility, iproute2-5.15.0".to_owned();
        let report = assess(&old);
        assert_eq!(status(&report, "seeded-loss"), Status::Warn);
        assert_eq!(status(&report, "program"), Status::Ok);
    }

    /// The features follow the probe (D-068): a kernel that refuses reordering is a warning on the line of the feature,
    /// with the answer of the kernel, and a good machine has no such line.
    #[test]
    fn a_feature_that_the_kernel_refuses_is_a_warning_with_its_answer() {
        let mut findings = good();
        assert_eq!(status(&assess(&findings), "reordering"), Status::Ok);
        findings.ways[0].outcome = Ok(steps(&[("reorder", "Error: netem: reorder is refused.")]));
        let report = assess(&findings);
        assert_eq!(status(&report, "reordering"), Status::Warn);
        assert!(
            report
                .check("reordering")
                .unwrap()
                .detail
                .contains("reorder is refused"),
            "{}",
            report.render()
        );
        // The others are as they were
        assert_eq!(status(&report, "duplication"), Status::Ok);
    }

    /// D-052: a kernel that refuses a duplicating `netem` next to another is a warning with the answer of the
    /// kernel, and it does not make the plain duplication or the other models look broken.
    #[test]
    fn a_kernel_that_refuses_a_duplicating_tree_is_a_warning_with_its_answer() {
        let mut findings = good();
        findings.ways[0].outcome = Ok(steps(&[(
            "tree",
            "Error: netem: cannot mix duplicating netems with other netems in tree.",
        )]));
        let report = assess(&findings);
        assert_eq!(status(&report, "duplicate-tree"), Status::Warn);
        let line = report
            .checks
            .iter()
            .find(|c| c.id == "duplicate-tree")
            .unwrap();
        assert!(
            line.detail.contains("cannot mix duplicating netems"),
            "{}",
            line.detail
        );
        assert_eq!(status(&report, "models"), Status::Ok);
        // and the machine that accepts it says so
        assert_eq!(status(&assess(&good()), "duplicate-tree"), Status::Ok);
    }

    /// A node without `CAP_NET_RAW` is a warning that says what is lost, and the rest of the machine is fine.
    #[test]
    fn a_node_without_the_capability_for_a_raw_socket_is_a_warning_with_the_way_out() {
        let mut findings = good();
        findings.ways[0].outcome = Ok(steps(&[(
            "rawsocket",
            "CAP_NET_RAW is not among the effective capabilities of a node",
        )]));
        let report = assess(&findings);
        let line = report.checks.iter().find(|c| c.id == "raw-socket").unwrap();
        assert_eq!(line.status, Status::Warn);
        assert!(
            line.detail.contains("udp-checksum: false"),
            "{}",
            line.detail
        );
        assert!(
            line.fix
                .as_deref()
                .is_some_and(|fix| fix.contains("net_raw")),
            "{line:?}"
        );
        assert_eq!(status(&report, "models"), Status::Ok);
        // and the machine that has it says so
        assert_eq!(status(&assess(&good()), "raw-socket"), Status::Ok);
    }

    /// Canary: a model that the kernel refuses is named, and the others stay fine.
    #[test]
    fn a_refused_model_is_named_in_the_line_about_the_models() {
        let mut findings = good();
        findings.ways[0].outcome = Ok(steps(&[("chain", "Illegal \"state\"")]));
        let report = assess(&findings);
        let line = report.checks.iter().find(|c| c.id == "models").unwrap();
        assert_eq!(line.status, Status::Warn);
        assert!(
            line.detail.contains("the four-state loss"),
            "{}",
            line.detail
        );
        assert!(!line.detail.contains("reordering"), "{}", line.detail);
    }

    /// Canary: the judging can fail. A machine without a tool is not ok, and the line says where it looked.
    #[test]
    fn a_missing_tool_fails_and_says_where_it_looked() {
        let mut findings = good();
        findings.tools[1] = ToolFinding {
            name: "tc",
            required: true,
            path: None,
            printed: String::new(),
        };
        findings.ways.clear();
        let report = assess(&findings);
        assert!(!report.ok);
        let tc = report.check("tool-tc").expect("a line for tc");
        assert_eq!(tc.status, Status::Fail);
        assert!(tc.detail.contains("looked in PATH") && tc.detail.contains("/usr/sbin"));
        assert_eq!(tc.fix.as_deref(), Some("install the package `iproute2`"));
        // The ways are not tested, and that is no second problem
        assert_eq!(status(&report, "way-user-namespace"), Status::Info);
        assert!(report.check("can-create-namespaces").is_none());
    }

    #[test]
    fn kill_is_only_a_warning() {
        let mut findings = good();
        findings.tools.retain(|t| t.name != "kill");
        findings.tools.push(ToolFinding {
            name: "kill",
            required: false,
            path: None,
            printed: String::new(),
        });
        let report = assess(&findings);
        assert!(report.ok);
        assert_eq!(status(&report, "tool-kill"), Status::Warn);
    }

    #[test]
    fn apparmor_on_ubuntu_24_04_is_named_with_the_setting_to_change() {
        let mut findings = good();
        findings.ways = [Way::UserNamespace, Way::OwnRights]
            .into_iter()
            .map(|way| WayFinding {
                way,
                outcome: Err("`unshare` failed (exit status: 1): unshare: write failed /proc/self/uid_map: Operation not permitted".to_owned()),
            })
            .collect();
        findings.system.apparmor_restricts_userns = true;
        let report = assess(&findings);
        assert!(!report.ok);
        let line = report.check("can-create-namespaces").expect("the line");
        assert_eq!(line.status, Status::Fail);
        let fix = line.fix.as_deref().expect("a fix");
        assert!(
            fix.contains("kernel.apparmor_restrict_unprivileged_userns=0"),
            "{fix}"
        );
        assert!(
            fix.contains("/etc/sysctl.d/") && fix.contains("run netscenario as root"),
            "{fix}"
        );
        // The answer of the tool is shown for each way
        assert!(
            report
                .check("way-user-namespace")
                .expect("a line")
                .detail
                .contains("Operation not permitted")
        );
        // Nothing of the kernel is claimed
        assert_eq!(status(&report, "netem"), Status::Info);
    }

    #[test]
    fn a_container_is_told_what_it_needs() {
        let mut findings = good();
        findings.ways = [Way::UserNamespace, Way::OwnRights]
            .into_iter()
            .map(|way| WayFinding {
                way,
                outcome: Err("`unshare` failed (exit status: 1): unshare: unshare failed: Operation not permitted".to_owned()),
            })
            .collect();
        findings.system.container = true;
        let fix = assess(&findings)
            .check("can-create-namespaces")
            .and_then(|c| c.fix.clone())
            .expect("a fix");
        assert!(
            fix.contains("SYS_ADMIN") && fix.contains("seccomp"),
            "{fix}"
        );
    }

    #[test]
    fn root_that_is_refused_does_not_get_told_to_become_root() {
        let mut findings = good();
        findings.system.own_rights = true;
        findings.ways = [Way::OwnRights, Way::UserNamespace]
            .into_iter()
            .map(|way| WayFinding {
                way,
                outcome: Err("refused".to_owned()),
            })
            .collect();
        let fix = assess(&findings)
            .check("can-create-namespaces")
            .and_then(|c| c.fix.clone())
            .expect("a fix");
        assert!(!fix.contains("run netscenario as root"), "{fix}");
        assert!(fix.contains("security module"), "{fix}");
    }

    #[test]
    fn a_process_without_rights_is_told_about_root_and_about_ambient_capabilities() {
        let mut findings = good();
        findings.ways = [Way::UserNamespace, Way::OwnRights]
            .into_iter()
            .map(|way| WayFinding {
                way,
                outcome: Err("refused".to_owned()),
            })
            .collect();
        let fix = assess(&findings)
            .check("can-create-namespaces")
            .and_then(|c| c.fix.clone())
            .expect("a fix");
        assert!(fix.contains("run netscenario as root"), "{fix}");
        assert!(
            fix.contains("ambient capabilities") && fix.contains("setpriv --ambient-caps"),
            "{fix}"
        );
    }

    #[test]
    fn an_old_tc_warns_about_the_seed_and_names_the_version() {
        let mut findings = good();
        findings.tools = tools("tc utility, iproute2-6.1.0");
        findings.ways[0].outcome = Ok(steps(&[("seed", "What is \"seed\"? Usage: ... netem")]));
        let report = assess(&findings);
        assert!(report.ok, "an old tc is a limit, not a failure");
        let seed = report.check("seed").expect("the line");
        assert_eq!(seed.status, Status::Warn);
        assert!(seed.detail.contains("6.1.0") && seed.detail.contains("6.6.0"));
        assert_eq!(seed.fix.as_deref(), Some("install iproute2 6.6.0 or newer"));
        // The same fact appears in the features, as the backend declares it
        assert_eq!(status(&report, "seeded-loss"), Status::Warn);
    }

    #[test]
    fn a_new_tc_that_the_kernel_refuses_is_a_warning_with_the_answer() {
        let mut findings = good();
        findings.ways[0].outcome = Ok(steps(&[("seed", "RTNETLINK answers: Invalid argument")]));
        let seed = assess(&findings).check("seed").cloned().expect("the line");
        assert_eq!(seed.status, Status::Warn);
        assert!(seed.detail.contains("Invalid argument"), "{}", seed.detail);
    }

    /// The finding of the first CI run: a `tc` that is built from source does not find `normal.dist`.
    #[test]
    fn missing_distribution_tables_warn_and_name_tc_lib_dir() {
        let mut findings = good();
        findings.ways[0].outcome = Ok(steps(&[(
            "tables",
            "No distribution data for normal (/usr/lib/tc//normal.dist: No such file or directory)",
        )]));
        let report = assess(&findings);
        assert!(report.ok);
        let tables = report.check("tables").expect("the line");
        assert_eq!(tables.status, Status::Warn);
        assert!(tables.detail.contains("normal.dist"), "{}", tables.detail);
        assert!(
            tables
                .fix
                .as_deref()
                .is_some_and(|f| f.contains("TC_LIB_DIR"))
        );
        findings.system.tc_lib_dir = Some("/opt/tc".to_owned());
        let fix = assess(&findings)
            .check("tables")
            .and_then(|c| c.fix.clone())
            .expect("a fix");
        assert!(fix.contains("/opt/tc"), "{fix}");
    }

    #[test]
    fn a_kernel_without_netem_fails_and_the_later_steps_are_not_blamed() {
        let mut findings = good();
        findings.ways[0].outcome = Ok(steps(&[
            ("netem", "Error: Specified qdisc kind is unknown."),
            ("tables", "Error: Specified qdisc kind is unknown."),
            ("seed", "Error: Specified qdisc kind is unknown."),
        ]));
        let report = assess(&findings);
        assert!(!report.ok);
        let netem = report.check("netem").expect("the line");
        assert_eq!(netem.status, Status::Fail);
        assert!(
            netem
                .fix
                .as_deref()
                .is_some_and(|f| f.contains("sch_netem"))
        );
    }

    #[test]
    fn without_a_virtual_pair_nothing_else_is_tested() {
        let mut findings = good();
        findings.ways[0].outcome = Ok(steps(&[(
            "veth",
            "RTNETLINK answers: Operation not supported",
        )]));
        let report = assess(&findings);
        assert!(!report.ok);
        assert_eq!(status(&report, "veth"), Status::Fail);
        assert_eq!(status(&report, "netem"), Status::Fail);
        assert!(
            report
                .check("netem")
                .expect("the line")
                .detail
                .contains("no interface")
        );
    }

    #[test]
    fn the_probe_output_is_read_line_by_line() {
        let parsed = parse_probe(
            "ok veth\nfail netem Error: Specified qdisc kind is unknown. \nnoise\nok seed\n",
        );
        assert_eq!(
            parsed,
            [
                Step {
                    name: "veth".to_owned(),
                    result: Ok(())
                },
                Step {
                    name: "netem".to_owned(),
                    result: Err("Error: Specified qdisc kind is unknown.".to_owned())
                },
                Step {
                    name: "seed".to_owned(),
                    result: Ok(())
                },
            ]
        );
    }

    #[test]
    fn the_answer_of_a_tool_is_cut_from_the_command_line() {
        assert_eq!(
            answer_of(
                "`unshare --user x` failed (exit status: 1): unshare: failed: Operation not permitted"
            ),
            "unshare: failed: Operation not permitted"
        );
        assert_eq!(answer_of("refused"), "refused");
    }

    #[test]
    fn the_text_has_every_section_and_the_json_the_same_checks() {
        let report = assess(&good());
        let text = report.render();
        for heading in [
            "Tools",
            "Rights to create namespaces",
            "Kernel and tc",
            "Features of the Kernel backend",
        ] {
            assert!(text.contains(heading), "{heading}");
        }
        assert!(text.contains("Result: scenarios can run here"), "{text}");
        let json: serde_json::Value = serde_json::from_str(&report.to_json()).expect("json");
        assert_eq!(json["ok"], true);
        assert_eq!(json["version"], 1);
        assert_eq!(
            json["checks"].as_array().map(Vec::len),
            Some(report.checks.len())
        );
        assert_eq!(json["checks"][0]["status"], "ok");
    }

    #[test]
    fn a_failing_machine_says_so_in_the_last_line() {
        let mut findings = good();
        findings.ways[0].outcome = Err("refused".to_owned());
        let text = assess(&findings).render();
        assert!(text.contains("FAIL") && text.contains("fix:"), "{text}");
        assert!(text.contains("Result: scenarios cannot run here"), "{text}");
    }
}
