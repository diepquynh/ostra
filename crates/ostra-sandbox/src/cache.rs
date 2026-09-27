//! The sandbox's own tool caches, apart from the user's.

use ostra_core::ids::SessionId;
use ostra_core::paths;
use std::path::{Path, PathBuf};

/// The sandbox's own tool caches under `~/.cache/ostra/sandbox/` (or `$OSTRA_SANDBOX_CACHE`): one
/// set per session for agents and one per project for project programs. The user's `~/.cargo`,
/// `~/.npm`, and `~/.cache` stay read-only, because builds on the host run what those caches hold (extracted crate sources, npm and pnpm stores, Go build output). macOS
/// tools ignore `XDG_CACHE_HOME` and write under `~/Library/Caches`, which stays read-only for the
/// same reason, so they get their own variables.
pub(crate) const CACHE_ENV: &[(&str, &str)] = &[
    ("CARGO_HOME", "cargo"),
    ("npm_config_cache", "npm"),
    ("npm_config_store_dir", "pnpm-store"),
    ("XDG_CACHE_HOME", "xdg"),
    ("GOMODCACHE", "go-mod"),
    ("GOCACHE", "go-build"),
    ("PIP_CACHE_DIR", "pip"),
    ("YARN_CACHE_FOLDER", "yarn"),
    ("UV_CACHE_DIR", "uv"),
    ("CLANG_MODULE_CACHE_PATH", "clang-modules"),
];

/// The user's cargo settings, linked into the sandbox's `CARGO_HOME` so builds keep the same
/// linker and registry settings. Credentials are not linked.
pub(crate) const CARGO_SETTINGS: &[&str] = &["config.toml", "config"];

fn cache_base(home: &Path) -> PathBuf {
    std::env::var_os("OSTRA_SANDBOX_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".cache/ostra/sandbox"))
}

/// The tool caches of every agent execution in one session.
pub fn session_cache(home: &Path, session: &SessionId) -> PathBuf {
    cache_base(home).join(format!("session-{session}"))
}

/// Caches keyed by a path. `salt` keeps two users of the same path apart.
pub(crate) fn keyed_cache(home: &Path, salt: &str, key: &Path) -> PathBuf {
    cache_base(home).join(stable_hex(salt, key))
}

/// 16 hex digits of SHA-256 over `salt` and `path`. Names on disk use it instead of
/// `DefaultHasher`, whose algorithm may change between Rust releases and would move them.
pub(crate) fn stable_hex(salt: &str, path: &Path) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(salt.as_bytes());
    h.update([0]);
    h.update(path.as_os_str().as_encoded_bytes());
    h.finalize()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Removes a session's tool caches on a thread of its own, because they can hold gigabytes.
pub fn remove_session_cache(session: &SessionId) {
    let Some(home) = paths::home() else {
        return;
    };
    let dir = session_cache(Path::new(&home), session);
    if dir.is_dir() {
        let _ = std::thread::Builder::new()
            .name("ostra-cache-remove".into())
            .spawn(move || {
                if let Err(e) = std::fs::remove_dir_all(&dir) {
                    tracing::warn!("could not remove the session cache {}: {e}", dir.display());
                }
            });
    }
}
