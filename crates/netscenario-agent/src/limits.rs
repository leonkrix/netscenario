//! The limit on the open files of the agent.

use nix::sys::resource::{Resource, getrlimit, setrlimit};

/// Raises the soft limit on open files to the hard limit and returns the soft limit that is in force afterwards (`None` if the
/// system refused to tell or to change it).
///
/// A connection of a TCP workload is a file for the client and one for the server, and the usual soft limit is 1024 (the CI
/// runners of Ubuntu 22.04 have it, as a root shell): with `connections: 1000` the client has no room for the files of the
/// agent next to them, and the connects fail with "Too many open files". The hard limit is the one the system gives the user.
#[must_use]
pub fn raise_open_files() -> Option<u64> {
    let (soft, hard) = getrlimit(Resource::RLIMIT_NOFILE).ok()?;
    if soft >= hard {
        return Some(soft);
    }
    match setrlimit(Resource::RLIMIT_NOFILE, hard, hard) {
        Ok(()) => Some(hard),
        Err(_) => Some(soft),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The soft limit goes up to the hard one (the test lowers it first, in a process of its own under `nextest`; the hard limit
    /// stays as it is).
    #[test]
    fn the_soft_limit_is_raised_to_the_hard_limit() {
        let (_, hard) = getrlimit(Resource::RLIMIT_NOFILE).unwrap();
        let low = 256.min(hard);
        setrlimit(Resource::RLIMIT_NOFILE, low, hard).unwrap();
        assert_eq!(getrlimit(Resource::RLIMIT_NOFILE).unwrap().0, low);
        assert_eq!(raise_open_files(), Some(hard));
        assert_eq!(getrlimit(Resource::RLIMIT_NOFILE).unwrap(), (hard, hard));
    }
}
