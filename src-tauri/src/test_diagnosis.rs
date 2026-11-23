//! Explanations of failed tests on the Tests tab, written by a model: for a
//! failing top-level test (its subtests included) or a package that did not
//! compile, why it failed, whether the code, the test, chance or the place
//! it ran is at fault, the line to change and the fix.
//!
//! A call sends the failure as the last run read it, how the tests ran (the
//! sandbox has no network), the code it points at and the end of the
//! output (`inputs`), and reads back one answer of a strict schema. Each
//! explanation is kept per review and failure (package and top-level test),
//! with when the run it explained started and fingerprints of the failure
//! and of the code sent, so a later look tells when either changed since
//! (outdated), and the Translate flow can add a version in another
//! language.
//!
//! The calls being written are kept by the core (`calls`), so a review left
//! and opened again finds them going on and never asks twice for one
//! failure; the calls prepared for a run are kept per run and sides
//! (`InputsMemo`), so reading the explanations again is quick.
//!
//! Nothing here asks a model on its own: only a click on Explain the
//! failure (or Explain again), or Translate, does.

mod calls;
mod comment;
mod inputs;
mod service;

pub use calls::{CallKey, DiagnosisCalls};
pub use comment::write as write_review_comment;
#[cfg(test)]
pub use comment::GeneralReason;
pub use comment::{fence_language, placement, sentence, where_section, Placement};
pub use inputs::{bare_code, Subject};
pub use service::{
    diagnose, diagnoses, plan_translation, translate, Facts, InputsMemo, KeptInputs,
};

/// What the explanations of failures keep between commands: the calls being
/// written and the calls prepared for the last runs.
#[derive(Default)]
pub struct TestDiagnoses {
    pub calls: DiagnosisCalls,
    pub inputs: InputsMemo,
}

impl TestDiagnoses {
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests;
