//! Where and when a result was made (ENV-02): the facts that a reader needs to judge it.
//!
//! The host name, the user name and the paths are not recorded: a result may be shared.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::backend::BackendInfo;

/// The provenance block of a result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Provenance {
    /// The version of netscenario that made the result.
    pub netscenario: String,
    /// When the run started, UTC, as `2026-10-30T12:00:00Z`.
    pub started_at: String,
    /// The backend that ran it.
    pub backend: String,
    /// What the backend says about itself (the version of `tc`, how the rights were obtained).
    pub backend_details: BTreeMap<String, String>,
    /// The release of the kernel (`6.12.113+deb13-amd64`), if it could be read.
    pub kernel: Option<String>,
    /// The model of the processor, if it could be read.
    pub cpu_model: Option<String>,
    /// How many processors the run could use.
    pub cpus: Option<usize>,
}

/// Collects the provenance of a run that starts now.
#[must_use]
pub fn collect(backend: &BackendInfo, now: SystemTime) -> Provenance {
    let seconds = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    Provenance {
        netscenario: env!("CARGO_PKG_VERSION").to_owned(),
        started_at: rfc3339_utc(seconds),
        backend: backend.name.clone(),
        backend_details: backend.details.clone(),
        kernel: std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .ok()
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty()),
        cpu_model: std::fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|text| cpu_model(&text)),
        cpus: std::thread::available_parallelism().ok().map(usize::from),
    }
}

/// The model of the first processor in the text of `/proc/cpuinfo`.
#[must_use]
pub(crate) fn cpu_model(cpuinfo: &str) -> Option<String> {
    cpuinfo.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        // x86 says `model name`, some ARM kernels say `Model` or `Hardware`
        matches!(key.trim(), "model name" | "Model" | "Hardware")
            .then(|| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    })
}

/// Seconds since 1970 as `YYYY-MM-DDTHH:MM:SSZ`, by the civil-from-days algorithm of the proleptic Gregorian
/// calendar.
#[must_use]
pub fn rfc3339_utc(seconds: u64) -> String {
    let days = i64::try_from(seconds / 86_400).unwrap_or(0);
    let rest = seconds % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_part + 2) / 5 + 1;
    let month = if month_part < 10 {
        month_part + 3
    } else {
        month_part - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_right_at_known_moments() {
        for (seconds, expected) in [
            (0, "1970-01-01T00:00:00Z"),
            (951_782_400, "2000-02-29T00:00:00Z"),
            (1_000_000_000, "2001-09-09T01:46:40Z"),
            (1_709_164_800, "2024-02-29T00:00:00Z"),
            (1_709_251_199, "2024-02-29T23:59:59Z"),
            (1_709_251_200, "2024-03-01T00:00:00Z"),
            (1_767_225_600, "2026-01-01T00:00:00Z"),
            (4_102_444_799, "2099-12-31T23:59:59Z"),
        ] {
            assert_eq!(rfc3339_utc(seconds), expected);
        }
    }

    #[test]
    fn the_model_of_the_processor_is_read_from_cpuinfo() {
        let x86 = "processor\t: 0\nvendor_id\t: AuthenticAMD\nmodel name\t: AMD Ryzen 7 5800X 8-Core Processor\ncpu MHz\t\t: 3800\n";
        assert_eq!(
            cpu_model(x86).as_deref(),
            Some("AMD Ryzen 7 5800X 8-Core Processor")
        );
        assert_eq!(
            cpu_model("Hardware\t: BCM2711\n").as_deref(),
            Some("BCM2711")
        );
        assert_eq!(cpu_model("processor\t: 0\n"), None);
        assert_eq!(cpu_model(""), None);
    }

    #[test]
    fn the_provenance_names_the_backend_and_the_tool() {
        let mut details = BTreeMap::new();
        details.insert("tc".to_owned(), "iproute2 6.15.0".to_owned());
        let provenance = collect(
            &BackendInfo {
                name: "kernel".to_owned(),
                details,
            },
            UNIX_EPOCH + std::time::Duration::from_secs(1_767_225_600),
        );
        assert_eq!(provenance.backend, "kernel");
        assert_eq!(provenance.started_at, "2026-01-01T00:00:00Z");
        assert_eq!(provenance.backend_details["tc"], "iproute2 6.15.0");
        assert_eq!(provenance.netscenario, env!("CARGO_PKG_VERSION"));
    }
}
