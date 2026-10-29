# Security policy

## Supported versions

The latest released version gets security fixes. While the version is `0.x`, older minor versions do not.

## Reporting a vulnerability

Please do not open a public issue for a security problem.

Use GitHub's private vulnerability reporting: on the repository page choose **Security**, then **Report a vulnerability**. You get an answer within a few days.

## What this project does that matters for security

netscenario creates network namespaces, virtual network devices and traffic-control rules, and it starts programs inside them. That needs privileges (`CAP_NET_ADMIN` in a user namespace, or root). The design rules that follow from this:

- **It never touches the network of the host.** All devices and rules live in namespaces that netscenario creates and removes. A rule that would apply to a host interface is refused.
- **It asks for the least privilege that works.** The preferred way is an unprivileged user namespace. Where that is not possible, it says what it needs and why.
- **Scenario files are data, not code.** A scenario cannot run commands on its own. A workload that runs a command says so explicitly, and that command is yours.
- **It sends nothing anywhere.** No telemetry, no network access of its own beyond the emulated network in the namespaces.

Reports about escaping a namespace, about a scenario file that makes the tool act outside what it was told, or about unsafe handling of the files and sockets it creates are especially welcome.
