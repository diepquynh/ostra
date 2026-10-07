//! The spec stage's answers to the engine's `Pipeline` hooks.

#[allow(unused_imports)]
use crate::prelude::*;
use ostra_engine::state::SessionState;
use std::path::PathBuf;

pub(crate) fn spec_file(s: &SessionState) -> Option<PathBuf> {
    s.ext
        .os()
        .spec
        .current
        .as_ref()
        .map(|c| PathBuf::from(&c.spec_path))
}
