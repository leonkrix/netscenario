//! `netscenario profiles` and `netscenario capabilities` (CLI-03): what the profile library has, and what the Kernel backend does.

use std::fmt::Write as _;
use std::io::Write;

use netscenario_model::{Profile, library};

use crate::args::{Parsed, Spec};
use crate::{exit, usage_error};

const PROFILES_HELP: &str = "\
Usage: netscenario profiles [<id>] [--json]

Lists the profiles of the library (SCN-06): the named sets of values that a link takes with `profile: <id>`, and
what each one sets. With an id it shows the values, up (forward) and down (reverse), and the sources of the
numbers. Every number has a source in docs/PROFILES.md; what no source states is not set, and the file writes it
next to the profile.

  --json    print the library, or the profile, as JSON

Exit codes: 0 done, 2 there is no such profile.
";

const CAPABILITIES_HELP: &str = "\
Usage: netscenario capabilities [--json]

Prints what the Kernel backend reproduces on this machine, feature by feature: exactly, only in part (a scenario
must accept the restriction with `allow-limited`), or not at all, with the way out. It is the feature list of
`netscenario doctor` without the checks that lead to it. See docs/DOCTOR.md and docs/COVERAGE.md.

  --json    print the features as JSON

Exit codes: 0 done.
";

/// `profiles`: the list, or one profile.
pub(crate) fn profiles(args: &[String], out: &mut impl Write, err: &mut impl Write) -> u8 {
    const SPEC: Spec = Spec {
        values: &[],
        flags: &["json"],
    };
    let parsed = match Parsed::parse(args, &SPEC) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(err, "profiles", &message, PROFILES_HELP),
    };
    if parsed.help {
        let _ = write!(out, "{PROFILES_HELP}");
        return exit::OK;
    }
    if parsed.positionals.len() > 1 {
        let message = format!("unexpected extra argument '{}'", parsed.positionals[1]);
        return usage_error(err, "profiles", &message, PROFILES_HELP);
    }
    let library = library();
    let json = parsed.flag("json");
    let Some(id) = parsed.positionals.first() else {
        let _ = if json {
            writeln!(out, "{}", json_of_library(library))
        } else {
            write!(out, "{}", list(library))
        };
        return exit::OK;
    };
    let Some(profile) = library.get(id) else {
        let hint = match library.closest(id) {
            Some(close) => format!(" Did you mean '{close}'?"),
            None => String::new(),
        };
        let message = format!(
            "there is no profile called '{id}'.{hint} `netscenario profiles` lists the profiles"
        );
        return usage_error(err, "profiles", &message, PROFILES_HELP);
    };
    let _ = if json {
        writeln!(out, "{}", json_of_profile(profile))
    } else {
        write!(out, "{}", show(profile, library.version()))
    };
    exit::OK
}

/// `capabilities`: the features of the Kernel backend on this machine.
pub(crate) fn capabilities(args: &[String], out: &mut impl Write, err: &mut impl Write) -> u8 {
    const SPEC: Spec = Spec {
        values: &[],
        flags: &["json"],
    };
    let parsed = match Parsed::parse(args, &SPEC) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(err, "capabilities", &message, CAPABILITIES_HELP),
    };
    if parsed.help {
        let _ = write!(out, "{CAPABILITIES_HELP}");
        return exit::OK;
    }
    if let Some(extra) = parsed.positionals.first() {
        let message = format!("unexpected extra argument '{extra}'");
        return usage_error(err, "capabilities", &message, CAPABILITIES_HELP);
    }
    let report = netscenario_backend_kernel::doctor::diagnose();
    let _ = if parsed.flag("json") {
        writeln!(out, "{}", report.features_json())
    } else {
        write!(out, "{}", report.render_features())
    };
    exit::OK
}

fn json_of_library(library: &netscenario_model::Library) -> String {
    serde_json::to_string_pretty(library)
        .unwrap_or_else(|error| format!("{{\"error\": \"{error}\"}}"))
}

fn json_of_profile(profile: &Profile) -> String {
    serde_json::to_string_pretty(profile)
        .unwrap_or_else(|error| format!("{{\"error\": \"{error}\"}}"))
}

fn list(library: &netscenario_model::Library) -> String {
    let mut text = format!(
        "Profile library, version {} (the numbers and their sources: docs/PROFILES.md)\n\n",
        library.version()
    );
    let width = library
        .profiles()
        .iter()
        .map(|p| p.id.len())
        .max()
        .unwrap_or(0);
    for profile in library.profiles() {
        let _ = writeln!(
            text,
            "  {:<width$}  {:<12} {}",
            profile.id,
            profile.keys().join(", "),
            profile.summary
        );
    }
    text.push_str(
        "\nUse one on a link with `profile: <id>`; the first node of `between` is the terminal.\n`netscenario profiles <id>` shows the values and the sources.\n",
    );
    text
}

fn show(profile: &Profile, version: u32) -> String {
    let mut text = format!(
        "{} ({}, {}), library version {version}\n{}\n\nSources: {} (docs/PROFILES.md)\n\n",
        profile.id,
        profile.family,
        profile.tier,
        profile.summary,
        profile.sources.join(", ")
    );
    match profile.values_yaml() {
        Ok(yaml) => {
            text.push_str(
                "What a link gets (forward is up, from the terminal; reverse is down):\n\n",
            );
            for line in yaml.lines() {
                let _ = writeln!(text, "  {line}");
            }
        }
        Err(message) => {
            let _ = writeln!(text, "(the values cannot be written: {message})");
        }
    }
    text.push_str("\nKeys that are written next to `profile:` win over these.\n");
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(args: &[&str]) -> (u8, String, String) {
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = profiles(&args, &mut out, &mut err);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn the_list_names_every_profile_and_what_it_sets() {
        let (code, out, _) = call(&[]);
        assert_eq!(code, exit::OK);
        for profile in library().profiles() {
            assert!(out.contains(&profile.id), "{}: {out}", profile.id);
        }
        assert!(out.contains("version 1"), "{out}");
        assert!(
            out.contains("delay, loss, rate"),
            "geo-good sets three keys: {out}"
        );
    }

    #[test]
    fn one_profile_shows_its_values_and_its_sources() {
        let (code, out, _) = call(&["geo-good"]);
        assert_eq!(code, exit::OK);
        assert!(out.contains("298500us"), "{out}");
        assert!(out.contains("144mbit"), "{out}");
        assert!(out.contains("Sources: S6"), "{out}");
        let (_, json, _) = call(&["geo-good", "--json"]);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["id"], "geo-good");
        assert_eq!(value["sources"][0], "S6");
    }

    #[test]
    fn an_unknown_profile_is_a_usage_error_with_the_closest_one() {
        let (code, _, err) = call(&["lte-typcal"]);
        assert_eq!(code, exit::USAGE);
        assert!(err.contains("Did you mean 'lte-typical'"), "{err}");
        let (code, _, err) = call(&["a", "b"]);
        assert_eq!(code, exit::USAGE);
        assert!(err.contains("unexpected extra argument 'b'"), "{err}");
    }

    #[test]
    fn the_list_as_json_has_the_version_and_every_profile() {
        let (_, json, _) = call(&["--json"]);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["version"], 1);
        assert_eq!(
            value["profiles"].as_array().unwrap().len(),
            library().profiles().len()
        );
    }
}
