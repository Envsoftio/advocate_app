//! SQLCipher 4 raw-key connections compatible with the Python vault database.
use crate::vault::{hex_bytes, DataKey};
use rusqlite::{ffi, params, Connection, OpenFlags};
use std::{
    fmt,
    fs::{self, File},
    io::{self, Read},
    path::Path,
    time::Duration,
};
use tempfile::NamedTempFile;
use zeroize::Zeroizing;

const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

#[derive(Debug)]
pub enum DatabaseError {
    Contract(&'static str),
    Sql(rusqlite::Error),
    Io(io::Error),
}

impl fmt::Display for DatabaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(message) => f.write_str(message),
            Self::Sql(_) => f.write_str("Protected database operation failed"),
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for DatabaseError {}

impl From<rusqlite::Error> for DatabaseError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sql(error)
    }
}

impl From<io::Error> for DatabaseError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

fn database_header(path: &Path) -> Result<[u8; SQLITE_HEADER.len()], DatabaseError> {
    let file_type = fs::symlink_metadata(path)?.file_type();
    if !file_type.is_file() {
        return Err(DatabaseError::Contract("Expected a regular database file"));
    }
    let mut header = [0; SQLITE_HEADER.len()];
    File::open(path)?.read_exact(&mut header)?;
    Ok(header)
}

fn raw_sqlcipher_key(data_key: &DataKey) -> Zeroizing<String> {
    let hex = hex_bytes(data_key.expose());
    let mut key = Zeroizing::new(String::with_capacity(hex.len() + 3));
    key.push_str("x'");
    key.push_str(&hex);
    key.push('\'');
    key
}

fn key_connection(con: &Connection, data_key: &DataKey) -> Result<(), DatabaseError> {
    let key = raw_sqlcipher_key(data_key);
    // SQLCipher copies this raw-key representation before returning. The key
    // stays out of SQL text, rusqlite errors, and SQL trace callbacks.
    let status = unsafe { ffi::sqlite3_key(con.handle(), key.as_ptr().cast(), key.len() as i32) };
    if status != ffi::SQLITE_OK {
        return Err(DatabaseError::Contract(
            "SQLCipher could not set the database key",
        ));
    }
    let version: String = con.query_row("PRAGMA cipher_version", [], |row| row.get(0))?;
    if !version.starts_with("4.") {
        return Err(DatabaseError::Contract("SQLCipher 4 is required"));
    }
    con.busy_timeout(Duration::from_secs(30))?;
    con.pragma_update(None, "temp_store", "MEMORY")?;
    Ok(())
}

/// Open an existing keyed database and force the key check before schema operations.
pub fn open_protected(path: &Path, data_key: &DataKey) -> Result<Connection, DatabaseError> {
    open_keyed(path, data_key, OpenFlags::SQLITE_OPEN_READ_WRITE)
}

pub(crate) fn open_protected_read_only(
    path: &Path,
    data_key: &DataKey,
) -> Result<Connection, DatabaseError> {
    open_keyed(path, data_key, OpenFlags::SQLITE_OPEN_READ_ONLY)
}

pub(crate) fn open_protected_snapshot(
    path: &Path,
    data_key: &DataKey,
) -> Result<Connection, DatabaseError> {
    if &database_header(path)? == SQLITE_HEADER {
        return Err(DatabaseError::Contract(
            "Expected a protected database snapshot",
        ));
    }
    let name = path
        .to_str()
        .ok_or(DatabaseError::Contract("Invalid snapshot filename"))?;
    let encoded: String = form_urlencoded::byte_serialize(name.as_bytes()).collect();
    let uri = format!("file:{}?immutable=1", encoded.replace('+', "%20"));
    let connection = connect_keyed(
        Path::new(&uri),
        data_key,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;
    connection.pragma_update(None, "trusted_schema", "OFF")?;
    Ok(connection)
}

fn open_keyed(
    path: &Path,
    data_key: &DataKey,
    flags: OpenFlags,
) -> Result<Connection, DatabaseError> {
    if &database_header(path)? == SQLITE_HEADER {
        return Err(DatabaseError::Contract(
            "Plaintext database requires migration before protected opening",
        ));
    }
    connect_keyed(path, data_key, flags)
}

fn connect_keyed(
    path: &Path,
    data_key: &DataKey,
    flags: OpenFlags,
) -> Result<Connection, DatabaseError> {
    let connection = Connection::open_with_flags(path, flags | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    key_connection(&connection, data_key)?;
    connection
        .query_row("SELECT count(*) FROM sqlite_master", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|_| DatabaseError::Contract("Invalid key or damaged protected database"))?;
    Ok(connection)
}

pub fn verify_integrity(con: &Connection) -> Result<(), DatabaseError> {
    let result: String = con.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if result != "ok" {
        return Err(DatabaseError::Contract(
            "Protected database failed integrity check",
        ));
    }
    Ok(())
}

fn unused_destination(destination: &Path) -> Result<&Path, DatabaseError> {
    match fs::symlink_metadata(destination) {
        Ok(_) => {
            return Err(DatabaseError::Contract(
                "Expected an unused database destination",
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    Ok(parent)
}

fn publish_candidate(
    candidate: NamedTempFile,
    destination: &Path,
    data_key: &DataKey,
) -> Result<(), DatabaseError> {
    let check = open_protected(candidate.path(), data_key)?;
    verify_integrity(&check)?;
    drop(check);
    candidate.as_file().sync_all()?;
    candidate
        .persist_noclobber(destination)
        .map_err(|error| error.error)?;
    #[cfg(unix)]
    File::open(
        destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?
    .sync_all()?;
    Ok(())
}

/// Create a fresh encrypted database using an exclusive destination publication.
pub fn create_protected(destination: &Path, data_key: &DataKey) -> Result<(), DatabaseError> {
    let parent = unused_destination(destination)?;
    let candidate = NamedTempFile::new_in(parent)?;
    let con = Connection::open_with_flags(
        candidate.path(),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    key_connection(&con, data_key)?;
    con.pragma_update(None, "user_version", 0)?;
    drop(con);
    publish_candidate(candidate, destination, data_key)
}

/// Copy a legacy database to a new encrypted file. The migration owner must
/// hold exclusive access to the source root for the duration of this call.
pub fn migrate_plain_database(
    source: &Path,
    destination: &Path,
    data_key: &DataKey,
) -> Result<(), DatabaseError> {
    if &database_header(source)? != SQLITE_HEADER {
        return Err(DatabaseError::Contract(
            "Expected a plaintext SQLite source",
        ));
    }
    let parent = unused_destination(destination)?;
    let candidate = NamedTempFile::new_in(parent)?;
    let filename = candidate
        .path()
        .to_str()
        .ok_or(DatabaseError::Contract("Invalid database filename"))?;
    {
        let con = Connection::open_with_flags(
            source,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        con.busy_timeout(Duration::from_secs(30))?;
        let key = raw_sqlcipher_key(data_key);
        // Bound parameters keep key material out of the SQL statement.
        con.execute(
            "ATTACH DATABASE ?1 AS protected KEY ?2",
            params![filename, &*key],
        )?;
        con.query_row("SELECT sqlcipher_export('protected')", [], |_| Ok(()))?;
        let version: i64 = con.pragma_query_value(None, "user_version", |row| row.get(0))?;
        con.pragma_update(
            Some(rusqlite::DatabaseName::Attached("protected")),
            "user_version",
            version,
        )?;
        con.execute_batch("DETACH DATABASE protected")?;
    }
    publish_candidate(candidate, destination, data_key)
}
