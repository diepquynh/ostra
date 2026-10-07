//! The closing stage's answers to the engine's `Pipeline` hooks: the request file of a `TEST`
//! session.

use ostra_core::pipeline::Category;
use ostra_engine::plan::SpawnRequest;
use ostra_engine::state::SessionState;

/// A `TEST` session has no implementer, so the request stands in for the implementer report.
pub(crate) fn spawn_files(st: &SessionState, req: &SpawnRequest) {
    if st.category == Some(Category::Test)
        && let Some(p) = &req.inputs.implementer_report
        && !p.exists()
    {
        let _ = std::fs::write(
            p,
            format!(
                "# Test request\n\nNo implementer ran in this session. The user asked for tests directly.\n\n## Request\n\n{}\n\n## Changed files\n\nNone, because no implementer ran. Take the code under test from the request: the files or symbols it names, or the earlier change it refers to, from the git history or the staged changes.\n",
                st.full_request()
            ),
        );
    }
}
