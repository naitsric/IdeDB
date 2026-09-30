//! Opening a session on a saved data source: look it up, resolve its
//! password, connect.

use idedb_core::Engine;
use idedb_store::{DataSource, SecretStore, Store};

use crate::AnySession;

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("data source not found")]
    NotFound,
    /// The data source does not save its password and none was given, or the
    /// saved one is missing. The UI asks for it.
    #[error("{0}")]
    PasswordRequired(String),
    #[error(transparent)]
    Store(#[from] idedb_store::Error),
    #[error(transparent)]
    Driver(#[from] idedb_core::Error),
}

/// Connects to the saved data source `id` with `password`, or the stored
/// one when `None` (see [`resolve_password`]).
pub async fn open_data_source(
    store: &Store,
    secrets: &dyn SecretStore,
    id: &str,
    password: Option<String>,
) -> Result<(DataSource, AnySession), OpenError> {
    let source = store.get(id)?.ok_or(OpenError::NotFound)?;
    let password = resolve_password(&source, password, secrets)?;
    let session = AnySession::connect(&source.params, password.as_deref()).await?;
    Ok((source, session))
}

/// The password to connect with: the one given, else the stored one. When
/// neither exists the UI is told to ask, rather than connecting without
/// one and failing authentication. A passwordless server has an empty
/// password stored.
pub fn resolve_password(
    source: &DataSource,
    given: Option<String>,
    secrets: &dyn SecretStore,
) -> Result<Option<String>, OpenError> {
    if given.is_some() || source.params.engine == Engine::Sqlite {
        return Ok(given);
    }
    let stored = if source.save_password { secrets.get(&source.id)? } else { None };
    match stored {
        Some(password) => Ok(Some(password)),
        None => Err(OpenError::PasswordRequired(if source.save_password {
            format!("Password for {} is not in the Keychain", source.name)
        } else {
            format!("Password for {} is not saved", source.name)
        })),
    }
}

#[cfg(test)]
mod tests {
    use idedb_core::{ConnectionParams, SslMode};
    use idedb_store::MemorySecrets;

    use super::*;

    fn source(engine: Engine, save_password: bool) -> DataSource {
        DataSource {
            id: "ds".into(),
            name: "db".into(),
            params: ConnectionParams {
                engine,
                host: "localhost".into(),
                port: None,
                user: "u".into(),
                database: String::new(),
                ssl_mode: SslMode::Prefer,
                path: String::new(),
            },
            color: None,
            save_password,
        }
    }

    fn required(result: Result<Option<String>, OpenError>) -> bool {
        matches!(result, Err(OpenError::PasswordRequired(_)))
    }

    #[test]
    fn asks_when_a_saved_password_is_missing_from_the_keychain() {
        let secrets = MemorySecrets::default();
        assert!(required(resolve_password(&source(Engine::Postgres, true), None, &secrets)));
    }

    #[test]
    fn uses_a_stored_password_even_when_empty() {
        let secrets = MemorySecrets::default();
        secrets.set("ds", "").unwrap();
        assert_eq!(resolve_password(&source(Engine::Mysql, true), None, &secrets).unwrap(), Some(String::new()));
        secrets.set("ds", "s3cret").unwrap();
        assert_eq!(resolve_password(&source(Engine::Mysql, true), None, &secrets).unwrap(), Some("s3cret".into()));
    }

    #[test]
    fn asks_for_a_password_that_is_not_saved_and_prefers_the_given_one() {
        let secrets = MemorySecrets::default();
        secrets.set("ds", "stale").unwrap();
        assert!(required(resolve_password(&source(Engine::Postgres, false), None, &secrets)));
        let given = resolve_password(&source(Engine::Postgres, true), Some("typed".into()), &secrets).unwrap();
        assert_eq!(given, Some("typed".into()));
    }

    #[test]
    fn sqlite_never_asks() {
        let secrets = MemorySecrets::default();
        assert_eq!(resolve_password(&source(Engine::Sqlite, true), None, &secrets).unwrap(), None);
    }

    #[tokio::test]
    async fn open_fails_for_a_missing_data_source() {
        let store = Store::in_memory().unwrap();
        let opened = open_data_source(&store, &MemorySecrets::default(), "missing", None).await;
        assert!(matches!(opened, Err(OpenError::NotFound)));
    }

    #[tokio::test]
    async fn open_asks_for_a_password_that_is_not_saved() {
        let store = Store::in_memory().unwrap();
        let saved = store.save(DataSource { id: String::new(), ..source(Engine::Postgres, false) }).unwrap();
        let opened = open_data_source(&store, &MemorySecrets::default(), &saved.id, None).await;
        match opened {
            Err(OpenError::PasswordRequired(message)) => assert_eq!(message, "Password for db is not saved"),
            Err(e) => panic!("expected PasswordRequired, got {e:?}"),
            Ok(_) => panic!("expected PasswordRequired, got a session"),
        }
    }

    #[tokio::test]
    async fn opens_a_saved_sqlite_data_source() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let mut sqlite = source(Engine::Sqlite, false);
        sqlite.id = String::new();
        sqlite.params.path = path.to_str().unwrap().to_owned();
        let store = Store::in_memory().unwrap();
        let saved = store.save(sqlite).unwrap();

        let (opened, session) = open_data_source(&store, &MemorySecrets::default(), &saved.id, None).await.unwrap();
        assert_eq!(opened, saved);
        assert!(matches!(session, AnySession::Sqlite(_)));
        assert_eq!(session.server_info().engine, Engine::Sqlite);
        assert!(path.exists());
    }
}
