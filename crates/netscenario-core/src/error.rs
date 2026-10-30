//! The error of the run lifecycle.

use netscenario_model::Problem;

use crate::capability::CapabilityError;

/// What can go wrong between a scenario and a running network.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The scenario is not valid. This happens for a scenario built in code; one read from a file has been
    /// checked already.
    #[error("the scenario is not valid:\n{}", list(.0))]
    InvalidScenario(Vec<Problem>),
    /// The backend cannot reproduce the scenario exactly.
    #[error("{0}")]
    Capability(#[from] CapabilityError),
    /// The backend failed to do something it said it can do.
    #[error("the {backend} backend failed to {action}: {message}")]
    Backend {
        /// The name of the backend.
        backend: String,
        /// What it was doing, as a verb phrase (`create the node 'client'`).
        action: String,
        /// What went wrong, including what the tools reported.
        message: String,
    },
}

fn list(problems: &[Problem]) -> String {
    problems
        .iter()
        .map(|problem| match &problem.path {
            Some(path) => format!("  {}: {}", path, problem.message),
            None => format!("  {}", problem.message),
        })
        .collect::<Vec<_>>()
        .join("\n")
}
