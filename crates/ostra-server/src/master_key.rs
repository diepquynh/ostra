//! The master key that seals credentials in the registry. It lives in the OS keychain when one
//! is reachable, otherwise in an owner-only file. `OSTRA_MASTER_KEY_FILE` names a file to use
//! instead, for example a mounted Docker secret.

use anyhow::Context;
use ostra_core::paths;
use ostra_store::RegistryDb;
use ostra_store::secrets::{Sealer, generate_key};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const BACKEND_KEY: &str = "secrets:backend";
const CHECK_KEY: &str = "secrets:check";
const CHECK_PLAIN: &[u8] = b"ostra";
const SERVICE: &str = "ostra";
const KEYCHAIN_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    Keychain,
    File,
}

impl Backend {
    fn as_str(self) -> &'static str {
        match self {
            Backend::Keychain => "keychain",
            Backend::File => "file",
        }
    }
}

enum KeychainError {
    NoEntry,
    Unavailable(String),
}

/// The registry, able to read and save credentials, with every plaintext credential sealed.
pub fn unlock(registry: RegistryDb) -> anyhow::Result<RegistryDb> {
    let key = load_key(
        &registry,
        std::env::var_os("OSTRA_MASTER_KEY_FILE").map(PathBuf::from),
        &paths::data_dir(),
        keychain_get,
        keychain_set,
    )?;
    let sealer = Arc::new(Sealer::new(&key));
    check_key(&registry, &sealer)?;
    let registry = registry.with_sealer(sealer);
    let sealed = registry.seal_plaintext_secrets()?;
    if sealed > 0 {
        tracing::info!("encrypted {sealed} saved credentials that were stored as plaintext");
    }
    Ok(registry)
}

fn load_key(
    registry: &RegistryDb,
    key_file: Option<PathBuf>,
    data_dir: &Path,
    get: impl Fn(&str) -> Result<[u8; 32], KeychainError>,
    set: impl Fn(&str, &[u8; 32]) -> Result<(), KeychainError>,
) -> anyhow::Result<[u8; 32]> {
    if let Some(path) = key_file {
        let key = file_key(&path)?;
        tracing::info!(
            "credentials are encrypted with the key in {}",
            path.display()
        );
        return Ok(key);
    }
    let recorded = registry
        .kv_get(BACKEND_KEY)?
        .map(|v| String::from_utf8_lossy(&v).into_owned());
    let account = keychain_account(data_dir);
    if recorded.as_deref() != Some(Backend::File.as_str()) {
        match get(&account) {
            Ok(key) => {
                record(registry, Backend::Keychain)?;
                tracing::info!("credentials are encrypted with a key in the system keychain");
                return Ok(key);
            }
            Err(KeychainError::NoEntry) => {
                if recorded.as_deref() == Some(Backend::Keychain.as_str()) {
                    tracing::warn!(
                        "the encryption key is missing from the system keychain, so a new one is created and saved credentials must be entered again"
                    );
                }
                let key = generate_key();
                match set(&account, &key).and_then(|()| get(&account)) {
                    Ok(stored) if stored == key => {
                        record(registry, Backend::Keychain)?;
                        tracing::info!(
                            "credentials are encrypted with a new key in the system keychain"
                        );
                        return Ok(key);
                    }
                    Ok(_) | Err(KeychainError::NoEntry) => {
                        tracing::info!("the system keychain did not keep the key")
                    }
                    Err(KeychainError::Unavailable(e)) => {
                        tracing::info!("the system keychain cannot store the key: {e}")
                    }
                }
            }
            Err(KeychainError::Unavailable(e)) => {
                if recorded.as_deref() == Some(Backend::Keychain.as_str()) {
                    anyhow::bail!(
                        "Unlock the system keychain and start Ostra again, because the key that encrypts saved credentials is kept there and the keychain cannot be reached ({e}). To move to a key file instead, set OSTRA_MASTER_KEY_FILE."
                    );
                }
                tracing::info!("the system keychain cannot be reached: {e}");
            }
        }
    }
    let path = data_dir.join("master.key");
    let key = file_key(&path)?;
    record(registry, Backend::File)?;
    tracing::info!(
        "credentials are encrypted with the key in {}, readable by this user only",
        path.display()
    );
    Ok(key)
}

fn record(registry: &RegistryDb, backend: Backend) -> anyhow::Result<()> {
    registry.kv_set(BACKEND_KEY, backend.as_str().as_bytes())?;
    Ok(())
}

/// A key that opens nothing sealed so far means credentials were saved under another key: they
/// read as not set, and the check value moves to the current key.
fn check_key(registry: &RegistryDb, sealer: &Sealer) -> anyhow::Result<()> {
    if let Some(stored) = registry.kv_get(CHECK_KEY)?
        && sealer.open(CHECK_KEY, &stored).as_deref() == Ok(CHECK_PLAIN)
    {
        return Ok(());
    }
    if registry.kv_get(CHECK_KEY)?.is_some() {
        tracing::warn!(
            "saved credentials were encrypted with a different key and cannot be read; enter them again in settings"
        );
    }
    registry.kv_set(CHECK_KEY, &sealer.seal(CHECK_KEY, CHECK_PLAIN))?;
    Ok(())
}

/// One keychain entry per data dir, so separate Ostra installs keep separate keys.
fn keychain_account(dir: &Path) -> String {
    let dir = ostra_core::paths::canonical(dir).unwrap_or_else(|_| dir.to_path_buf());
    format!("master-key {}", dir.display())
}

/// A keychain call can block on an unlock prompt or a dead D-Bus, so it runs on a thread that is
/// abandoned after a timeout.
fn with_timeout<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, KeychainError> + Send + 'static,
) -> Result<T, KeychainError> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(KEYCHAIN_TIMEOUT)
        .unwrap_or_else(|_| Err(KeychainError::Unavailable("timed out".into())))
}

fn map_err(e: keyring::Error) -> KeychainError {
    match e {
        keyring::Error::NoEntry => KeychainError::NoEntry,
        other => KeychainError::Unavailable(other.to_string()),
    }
}

fn keychain_get(account: &str) -> Result<[u8; 32], KeychainError> {
    let account = account.to_string();
    with_timeout(move || {
        let entry = keyring::Entry::new(SERVICE, &account).map_err(map_err)?;
        let text = entry.get_password().map_err(map_err)?;
        parse_key(&text)
            .ok_or_else(|| KeychainError::Unavailable("the stored key is malformed".into()))
    })
}

fn keychain_set(account: &str, key: &[u8; 32]) -> Result<(), KeychainError> {
    let account = account.to_string();
    let text = hex::encode(key);
    with_timeout(move || {
        let entry = keyring::Entry::new(SERVICE, &account).map_err(map_err)?;
        entry.set_password(&text).map_err(map_err)
    })
}

fn parse_key(text: &str) -> Option<[u8; 32]> {
    hex::decode(text.trim()).ok()?.try_into().ok()
}

/// Reads the key file, creating it owner-only when missing and tightening a looser mode.
fn file_key(path: &Path) -> anyhow::Result<[u8; 32]> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            restrict(path)?;
            parse_key(&text).with_context(|| {
                format!(
                    "{} does not hold a 64-character hex key; restore it, or delete it and enter saved credentials again",
                    path.display()
                )
            })
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let key = generate_key();
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            use std::io::Write;
            let mut f = opts
                .open(path)
                .with_context(|| format!("creating {}", path.display()))?;
            f.write_all(hex::encode(key).as_bytes())?;
            f.sync_all()?;
            Ok(key)
        }
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn restrict(path: &Path) -> anyhow::Result<()> {
    // On Windows the file keeps the ACL it inherits from the data dir under the user's profile.
    #[cfg(not(unix))]
    let _ = path;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path)?.permissions().mode();
        // A read-only secret mount cannot be changed and is already the operator's choice.
        if mode & 0o077 != 0
            && let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        {
            tracing::warn!(
                "{} is readable by other users and could not be restricted: {e}",
                path.display()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn unreachable_keychain(_: &str) -> Result<[u8; 32], KeychainError> {
        Err(KeychainError::Unavailable("no D-Bus".into()))
    }

    fn unreachable_set(_: &str, _: &[u8; 32]) -> Result<(), KeychainError> {
        Err(KeychainError::Unavailable("no D-Bus".into()))
    }

    fn registry(dir: &Path) -> RegistryDb {
        RegistryDb::open(&dir.join("registry.db")).unwrap()
    }

    #[test]
    fn falls_back_to_an_owner_only_key_file() {
        let dir = tempfile::tempdir().unwrap();
        let reg = registry(dir.path());
        let key = load_key(
            &reg,
            None,
            dir.path(),
            unreachable_keychain,
            unreachable_set,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.path().join("master.key");
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let again = load_key(
            &reg,
            None,
            dir.path(),
            unreachable_keychain,
            unreachable_set,
        )
        .unwrap();
        assert_eq!(key, again, "the file key is reused");
    }

    #[test]
    fn a_new_key_goes_to_the_keychain_when_it_works() {
        let dir = tempfile::tempdir().unwrap();
        let reg = registry(dir.path());
        let stored: RefCell<Option<[u8; 32]>> = RefCell::new(None);
        let get = |_: &str| stored.borrow().ok_or(KeychainError::NoEntry);
        let set = |_: &str, k: &[u8; 32]| {
            *stored.borrow_mut() = Some(*k);
            Ok(())
        };
        let key = load_key(&reg, None, dir.path(), get, set).unwrap();
        assert_eq!(Some(key), *stored.borrow());
        assert!(!dir.path().join("master.key").exists());
        assert_eq!(load_key(&reg, None, dir.path(), get, set).unwrap(), key);
    }

    #[test]
    fn an_unreachable_keychain_that_holds_the_key_stops_startup() {
        let dir = tempfile::tempdir().unwrap();
        let reg = registry(dir.path());
        record(&reg, Backend::Keychain).unwrap();
        let err = load_key(
            &reg,
            None,
            dir.path(),
            unreachable_keychain,
            unreachable_set,
        )
        .unwrap_err()
        .to_string();
        assert!(err.starts_with("Unlock the system keychain"), "{err}");
        assert!(
            !dir.path().join("master.key").exists(),
            "no new key replaces it"
        );
    }

    #[test]
    fn plaintext_credentials_are_sealed_at_unlock_and_a_lost_key_reads_as_not_set() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.db");
        let reg = RegistryDb::open(&path).unwrap();
        reg.kv_set("git_credential:a", b"{\"secret\":\"ghp_plain\"}")
            .unwrap();
        reg.kv_set("auth:token:x", b"123").unwrap();
        let key_file = dir.path().join("k");
        let key = load_key(
            &reg,
            Some(key_file.clone()),
            dir.path(),
            unreachable_keychain,
            unreachable_set,
        )
        .unwrap();
        let sealer = Arc::new(Sealer::new(&key));
        check_key(&reg, &sealer).unwrap();
        let reg = reg.with_sealer(sealer);
        assert_eq!(reg.seal_plaintext_secrets().unwrap(), 1);
        let raw = reg.kv_get("git_credential:a").unwrap().unwrap();
        assert!(ostra_store::secrets::is_sealed(&raw));
        assert_eq!(
            reg.kv_get("auth:token:x").unwrap().unwrap(),
            b"123",
            "non-secret rows stay"
        );
        assert_eq!(
            reg.secret_get("git_credential:a").unwrap().unwrap(),
            b"{\"secret\":\"ghp_plain\"}"
        );
        for f in ["registry.db", "registry.db-wal"] {
            let bytes = std::fs::read(dir.path().join(f)).unwrap_or_default();
            assert!(
                !bytes.windows(9).any(|w| w == b"ghp_plain"),
                "{f} still holds plaintext"
            );
        }

        let other = RegistryDb::open(&path)
            .unwrap()
            .with_sealer(Arc::new(Sealer::ephemeral()));
        assert_eq!(other.secret_get("git_credential:a").unwrap(), None);
        assert!(other.secret_scan("git_credential:").unwrap().is_empty());
    }

    #[test]
    fn a_registry_without_a_key_refuses_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let reg = registry(dir.path());
        assert!(matches!(
            reg.secret_set("vapid_private", b"k"),
            Err(ostra_store::StoreError::Locked)
        ));
    }
}
