//! Shared constants: the probe users and the on-disk layout setup creates.

pub const ROOT: &str = r"C:\ostra-probe";
pub const OUT: &str = r"C:\ostra-probe\out";
pub const REPO: &str = r"C:\ostra-probe\repo";
pub const PASS_FILE: &str = r"C:\ostra-probe\pass.txt";

pub const USER_A: &str = "OstraProbeA";
pub const USER_B: &str = "OstraProbeB";

/// BUILTIN\Users. A restricting SID that still lets a restricted token read what a normal user
/// reads (System32, Program Files), while a process's default DACL grants the specific account
/// SID, not this group, so it does not defeat the cross-process test.
pub const USERS_SID: &str = "S-1-5-32-545";

/// Everyone. Needed in a restricting set so the restricted child can reach base objects keyed to
/// Everyone (KnownDlls, BaseNamedObjects); process objects are keyed to the account/logon SID, not
/// Everyone, so this does not let a restricted opener reach a sibling process.
pub const EVERYONE_SID: &str = "S-1-1-0";

/// The exe under test, resolved at runtime.
pub fn exe() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "winprobe.exe".into())
}
