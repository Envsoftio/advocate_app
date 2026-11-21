//! Python-compatible vault key envelopes and authenticated CBENC1 streams.
use base64::{engine::general_purpose::STANDARD, Engine};
use openssl::{
    pkcs5::scrypt,
    rand::rand_bytes,
    symm::{encrypt_aead, Cipher, Crypter, Mode},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
    sync::Arc,
};
use tempfile::NamedTempFile;
use zeroize::{Zeroize, Zeroizing};

pub const MAGIC: &[u8; 7] = b"CBENC1\n";
pub const PROTECTED_PACKAGE_MAGIC: &[u8; 7] = b"CBMAT4\n";
pub const CHUNK_BYTES: usize = 1024 * 1024;
const PACKAGE_HEADER_LIMIT: usize = 16 * 1024;
const TAG_BYTES: usize = 16;
const NONCE_BYTES: usize = 12;
const KDF_N: u64 = 1 << 15;
const KDF_R: u64 = 8;
const KDF_P: u64 = 1;
const KDF_MAX_MEMORY: u64 = 128 * 1024 * 1024;
const KEY_FORMAT: &str = "courtbundle-key-envelope";
const HEX: &[u8; 16] = b"0123456789abcdef";

#[derive(Debug)]
pub enum VaultError {
    Format(&'static str),
    Crypto,
    Authentication,
    Io(io::Error),
}

impl fmt::Display for VaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Format(message) => f.write_str(message),
            Self::Crypto => f.write_str("Vault cryptographic operation failed"),
            Self::Authentication => f.write_str("Invalid credential or damaged encrypted data"),
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for VaultError {}

impl From<io::Error> for VaultError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone)]
pub struct DataKey(Arc<Zeroizing<[u8; 32]>>);

impl DataKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(Arc::new(Zeroizing::new(bytes)))
    }

    pub(crate) fn expose(&self) -> &[u8; 32] {
        &self.0
    }
}

pub enum Credential<'a> {
    Password(&'a str),
    Recovery(&'a str),
}

#[derive(Deserialize, Serialize)]
struct Slot {
    salt: String,
    nonce: String,
    wrapped: String,
}

pub struct NewVault {
    pub envelope: Value,
    pub recovery_key: Zeroizing<String>,
    pub data_key: DataKey,
}

fn kdf_descriptor() -> Value {
    json!({"name":"scrypt","n":KDF_N,"r":KDF_R,"p":KDF_P})
}

fn derive_key(secret: &[u8], salt: &[u8; 16]) -> Result<Zeroizing<[u8; 32]>, VaultError> {
    let mut key = Zeroizing::new([0; 32]);
    scrypt(
        secret,
        salt,
        KDF_N,
        KDF_R,
        KDF_P,
        KDF_MAX_MEMORY,
        key.as_mut(),
    )
    .map_err(|_| VaultError::Crypto)?;
    Ok(key)
}

fn wrap_slot(data_key: &DataKey, secret: &[u8], aad: &[u8]) -> Result<Slot, VaultError> {
    let mut salt = [0; 16];
    rand_bytes(&mut salt).map_err(|_| VaultError::Crypto)?;
    let key = derive_key(secret, &salt)?;
    let (nonce, encrypted, tag) = seal(&key, aad, data_key.expose())?;
    let mut wrapped = encrypted;
    wrapped.extend_from_slice(&tag);
    Ok(Slot {
        salt: STANDARD.encode(salt),
        nonce: STANDARD.encode(nonce),
        wrapped: STANDARD.encode(wrapped),
    })
}

const PASSWORD_AAD: &[u8] = b"CourtBundle vault key v1:password";
const RECOVERY_AAD: &[u8] = b"CourtBundle vault key v1:recovery";

pub fn create_key_envelope(password: &str) -> Result<NewVault, VaultError> {
    let mut bytes = Zeroizing::new([0; 32]);
    rand_bytes(bytes.as_mut()).map_err(|_| VaultError::Crypto)?;
    let data_key = DataKey(Arc::new(bytes));
    let (envelope, recovery_key) = rotate_key_envelope(&data_key, password)?;
    Ok(NewVault {
        envelope,
        recovery_key,
        data_key,
    })
}

pub fn rotate_key_envelope(
    data_key: &DataKey,
    password: &str,
) -> Result<(Value, Zeroizing<String>), VaultError> {
    if password.chars().count() < 6 {
        return Err(VaultError::Format(
            "Password must contain at least 6 characters",
        ));
    }
    let mut recovery = Zeroizing::new([0; 32]);
    rand_bytes(recovery.as_mut()).map_err(|_| VaultError::Crypto)?;
    let password_slot = wrap_slot(data_key, password.as_bytes(), PASSWORD_AAD)?;
    let recovery_slot = wrap_slot(data_key, recovery.as_ref(), RECOVERY_AAD)?;
    let envelope = json!({"format":KEY_FORMAT,"version":1,"kdf":kdf_descriptor(),
        "password":password_slot,"recovery":recovery_slot});
    Ok((envelope, hex_bytes(recovery.as_ref())))
}

pub(crate) fn hex_bytes(bytes: &[u8]) -> Zeroizing<String> {
    let mut text = Zeroizing::new(String::with_capacity(bytes.len() * 2));
    for byte in bytes {
        text.push(char::from(HEX[(byte >> 4) as usize]));
        text.push(char::from(HEX[(byte & 15) as usize]));
    }
    text
}

pub(crate) fn publication_tag(key: &DataKey, id: &str) -> Result<String, VaultError> {
    let key = openssl::pkey::PKey::hmac(key.expose()).map_err(|_| VaultError::Crypto)?;
    let mut signer = openssl::sign::Signer::new(openssl::hash::MessageDigest::sha256(), &key)
        .map_err(|_| VaultError::Crypto)?;
    signer
        .update(b"MatterDraft Rust document publication v1:")
        .map_err(|_| VaultError::Crypto)?;
    signer
        .update(id.as_bytes())
        .map_err(|_| VaultError::Crypto)?;
    let tag = signer.sign_to_vec().map_err(|_| VaultError::Crypto)?;
    Ok(hex_bytes(&tag).to_string())
}

fn recovery_bytes(value: &str) -> Result<Zeroizing<[u8; 32]>, VaultError> {
    let mut input = value.bytes();
    let mut output = Zeroizing::new([0; 32]);
    for byte in output.iter_mut() {
        let high = input.find(|byte| !byte.is_ascii_whitespace());
        let low = input.next();
        let digit = |digit: Option<u8>| digit.and_then(|digit| char::from(digit).to_digit(16));
        let (Some(high), Some(low)) = (digit(high), digit(low)) else {
            return Err(VaultError::Authentication);
        };
        *byte = ((high << 4) | low) as u8;
    }
    if !input.all(|byte| byte.is_ascii_whitespace()) {
        return Err(VaultError::Authentication);
    }
    Ok(output)
}

fn decode_fixed<const N: usize>(text: &str) -> Result<[u8; N], VaultError> {
    // Check the wire length before allocating or passing input to the KDF.
    if text.len() != N.div_ceil(3) * 4 {
        return Err(VaultError::Authentication);
    }
    STANDARD
        .decode(text)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(VaultError::Authentication)
}

pub fn unlock_key(envelope: &Value, credential: Credential<'_>) -> Result<DataKey, VaultError> {
    if envelope["format"] != KEY_FORMAT
        || envelope["version"] != 1
        || envelope["kdf"] != kdf_descriptor()
    {
        return Err(VaultError::Format("Unsupported vault key format"));
    }
    let recovery;
    let (name, secret, aad) = match credential {
        Credential::Password(value) => ("password", value.as_bytes(), PASSWORD_AAD),
        Credential::Recovery(value) => {
            recovery = recovery_bytes(value)?;
            ("recovery", recovery.as_ref(), RECOVERY_AAD)
        }
    };
    let slot = Slot::deserialize(&envelope[name]).map_err(|_| VaultError::Authentication)?;
    let salt = decode_fixed::<16>(&slot.salt)?;
    let nonce = decode_fixed::<NONCE_BYTES>(&slot.nonce)?;
    let wrapped = decode_fixed::<48>(&slot.wrapped)?;
    let key = derive_key(secret, &salt)?;
    let (encrypted, tag) = wrapped.split_at(32);
    let plaintext = open(&key, aad, &nonce, encrypted, tag)?;
    let mut bytes = Zeroizing::new([0; 32]);
    bytes.copy_from_slice(&plaintext);
    let data_key = DataKey(Arc::new(bytes));
    Ok(data_key)
}

type Frame = ([u8; NONCE_BYTES], Vec<u8>, [u8; TAG_BYTES]);

fn seal(key: &[u8; 32], aad: &[u8], plaintext: &[u8]) -> Result<Frame, VaultError> {
    let mut nonce = [0; NONCE_BYTES];
    let mut tag = [0; TAG_BYTES];
    rand_bytes(&mut nonce).map_err(|_| VaultError::Crypto)?;
    let ciphertext = encrypt_aead(
        Cipher::aes_256_gcm(),
        key,
        Some(&nonce),
        aad,
        plaintext,
        &mut tag,
    )
    .map_err(|_| VaultError::Crypto)?;
    Ok((nonce, ciphertext, tag))
}

fn open(
    key: &[u8; 32],
    aad: &[u8],
    nonce: &[u8],
    ciphertext: &[u8],
    tag: &[u8],
) -> Result<Zeroizing<Vec<u8>>, VaultError> {
    let cipher = Cipher::aes_256_gcm();
    let mut context =
        Crypter::new(cipher, Mode::Decrypt, key, Some(nonce)).map_err(|_| VaultError::Crypto)?;
    context.aad_update(aad).map_err(|_| VaultError::Crypto)?;
    context.set_tag(tag).map_err(|_| VaultError::Crypto)?;
    // Zeroize the buffer even if finalization rejects modified ciphertext.
    let mut buffer = Zeroizing::new(vec![0; ciphertext.len() + cipher.block_size()]);
    let size = context
        .update(ciphertext, &mut buffer)
        .map_err(|_| VaultError::Authentication)?;
    let tail = context
        .finalize(&mut buffer[size..])
        .map_err(|_| VaultError::Authentication)?;
    buffer.truncate(size + tail);
    Ok(buffer)
}

fn stream_aad(index: u64, final_frame: bool) -> [u8; 16] {
    let mut aad = [0; 16];
    aad[..7].copy_from_slice(MAGIC);
    aad[7..15].copy_from_slice(&index.to_be_bytes());
    aad[15] = u8::from(final_frame);
    aad
}

fn io_vault(error: VaultError) -> io::Error {
    match error {
        VaultError::Io(error) => error,
        _ => io::Error::new(io::ErrorKind::InvalidData, error),
    }
}

/// A bounded encrypted sink. `finish` authenticates the end and flushes it.
pub struct EncryptedWriter<'a, W> {
    output: W,
    key: &'a DataKey,
    pending: Zeroizing<Vec<u8>>,
    index: u64,
    failed: bool,
}

impl<'a, W: Write> EncryptedWriter<'a, W> {
    pub fn new(mut output: W, key: &'a DataKey) -> Result<Self, VaultError> {
        output.write_all(MAGIC)?;
        Ok(Self {
            output,
            key,
            pending: Zeroizing::new(Vec::with_capacity(CHUNK_BYTES)),
            index: 0,
            failed: false,
        })
    }

    fn emit(&mut self, final_frame: bool) -> Result<(), VaultError> {
        if self.failed {
            return Err(VaultError::Format("Encrypted writer is no longer usable"));
        }
        self.failed = true;
        let (nonce, ciphertext, tag) = seal(
            self.key.expose(),
            &stream_aad(self.index, final_frame),
            &self.pending,
        )?;
        let length = (ciphertext.len() + tag.len()) as u32;
        self.output.write_all(&length.to_be_bytes())?;
        self.output.write_all(&nonce)?;
        self.output.write_all(&ciphertext)?;
        self.output.write_all(&tag)?;
        self.pending.as_mut_slice().zeroize();
        self.pending.clear();
        self.index = self
            .index
            .checked_add(1)
            .ok_or(VaultError::Format("Encrypted stream is too long"))?;
        self.failed = false;
        Ok(())
    }

    pub fn finish(mut self) -> Result<W, VaultError> {
        if !self.pending.is_empty() {
            self.emit(false)?;
        }
        self.emit(true)?;
        self.output.flush()?;
        Ok(self.output)
    }
}

impl<W: Write> Write for EncryptedWriter<'_, W> {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        if self.failed {
            return Err(io::Error::other("Encrypted writer is no longer usable"));
        }
        let take = (CHUNK_BYTES - self.pending.len()).min(input.len());
        self.pending.extend_from_slice(&input[..take]);
        if self.pending.len() == CHUNK_BYTES {
            self.emit(false).map_err(io_vault)?;
        }
        Ok(take)
    }

    fn flush(&mut self) -> io::Result<()> {
        // Emit any buffered input, allowing callers to stream through layers.
        if !self.pending.is_empty() {
            self.emit(false).map_err(io_vault)?;
        }
        self.output.flush()
    }
}

fn read_magic(input: &mut impl Read) -> Result<(), VaultError> {
    let mut magic = [0; MAGIC.len()];
    input.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(VaultError::Format("Unsupported encrypted file format"));
    }
    Ok(())
}

fn read_frame(
    input: &mut impl Read,
    key: &DataKey,
    index: u64,
) -> Result<Zeroizing<Vec<u8>>, VaultError> {
    let mut length = [0; 4];
    input.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if !(TAG_BYTES..=CHUNK_BYTES + TAG_BYTES).contains(&length) {
        return Err(VaultError::Format("Invalid encrypted chunk length"));
    }
    let mut nonce = [0; NONCE_BYTES];
    let mut encrypted = vec![0; length];
    input.read_exact(&mut nonce)?;
    input.read_exact(&mut encrypted)?;
    let (ciphertext, tag) = encrypted.split_at(length - TAG_BYTES);
    open(
        key.expose(),
        &stream_aad(index, ciphertext.is_empty()),
        &nonce,
        ciphertext,
        tag,
    )
}

fn verify_end(input: &mut impl Read) -> Result<(), VaultError> {
    match input.read_exact(&mut [0]) {
        Ok(()) => Err(VaultError::Format("Trailing encrypted data")),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Authenticate a complete file while visiting each bounded plaintext chunk once.
pub fn visit_encrypted(
    input: impl Read,
    key: &DataKey,
    mut visit: impl FnMut(&[u8]),
) -> Result<(), VaultError> {
    try_visit_encrypted(input, key, |contents| {
        visit(contents);
        Ok(())
    })
}

pub(crate) fn try_visit_encrypted(
    mut input: impl Read,
    key: &DataKey,
    mut visit: impl FnMut(&[u8]) -> Result<(), VaultError>,
) -> Result<(), VaultError> {
    read_magic(&mut input)?;
    for index in 0..u64::MAX {
        let contents = read_frame(&mut input, key, index)?;
        if contents.is_empty() {
            return verify_end(&mut input);
        }
        visit(&contents)?;
    }
    Err(VaultError::Format("Encrypted stream is too long"))
}

struct Chunk {
    plaintext_offset: u64,
    encoded_offset: u64,
    size: usize,
    hash: [u8; 32],
}

/// Seekable plaintext view. Construction checks all frames, including the end
/// marker. Subsequent seeks authenticate the loaded frame again.
pub struct EncryptedReader<R> {
    input: R,
    key: DataKey,
    chunks: Vec<Chunk>,
    length: u64,
    position: u64,
    cache: Option<(usize, Zeroizing<Vec<u8>>)>,
    hash: [u8; 32],
}

pub(crate) fn check_encrypted_start(
    mut source: impl Read + Seek,
    offset: u64,
    key: &DataKey,
) -> Result<(), VaultError> {
    source.seek(SeekFrom::Start(offset))?;
    read_magic(&mut source)?;
    read_frame(&mut source, key, 0)?;
    Ok(())
}

impl<R: Read + Seek> EncryptedReader<R> {
    pub fn at(mut input: R, offset: u64, key: &DataKey) -> Result<Self, VaultError> {
        input.seek(SeekFrom::Start(offset))?;
        read_magic(&mut input)?;
        let mut chunks = Vec::new();
        let mut length = 0u64;
        let mut hash = Sha256::new();
        loop {
            let encoded_offset = input.stream_position()?;
            let contents = read_frame(&mut input, key, chunks.len() as u64)?;
            if contents.is_empty() {
                verify_end(&mut input)?;
                break;
            }
            chunks.push(Chunk {
                plaintext_offset: length,
                encoded_offset,
                size: contents.len(),
                hash: Sha256::digest(&*contents).into(),
            });
            hash.update(&*contents);
            length = length
                .checked_add(contents.len() as u64)
                .ok_or(VaultError::Format("Encrypted stream is too long"))?;
        }
        Ok(Self {
            input,
            key: key.clone(),
            chunks,
            length,
            position: 0,
            cache: None,
            hash: hash.finalize().into(),
        })
    }

    pub fn len(&self) -> u64 {
        self.length
    }

    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    pub(crate) fn digest(&self) -> &[u8; 32] {
        &self.hash
    }
}

impl<R: Read + Seek> Read for EncryptedReader<R> {
    fn read(&mut self, destination: &mut [u8]) -> io::Result<usize> {
        if self.position >= self.length || destination.is_empty() {
            return Ok(0);
        }
        let index = self
            .chunks
            .partition_point(|chunk| chunk.plaintext_offset <= self.position)
            - 1;
        let chunk = &self.chunks[index];
        if self
            .cache
            .as_ref()
            .is_none_or(|(cached, _)| *cached != index)
        {
            self.input.seek(SeekFrom::Start(chunk.encoded_offset))?;
            let contents =
                read_frame(&mut self.input, &self.key, index as u64).map_err(io_vault)?;
            if contents.len() != chunk.size || Sha256::digest(&*contents).as_slice() != chunk.hash {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Encrypted chunk changed",
                ));
            }
            self.cache = Some((index, contents));
        }
        let (_, content) = self.cache.as_ref().expect("the selected chunk is loaded");
        let start = (self.position - chunk.plaintext_offset) as usize;
        let size = destination.len().min(content.len() - start);
        destination[..size].copy_from_slice(&content[start..start + size]);
        self.position += size as u64;
        Ok(size)
    }
}

impl<R: Read + Seek> Seek for EncryptedReader<R> {
    fn seek(&mut self, target: SeekFrom) -> io::Result<u64> {
        let position = match target {
            SeekFrom::Start(position) => Some(position),
            SeekFrom::Current(delta) => self.position.checked_add_signed(delta),
            SeekFrom::End(delta) => self.length.checked_add_signed(delta),
        }
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Invalid encrypted file seek")
        })?;
        self.position = position;
        Ok(position)
    }
}

/// Publish a complete encrypted sibling atomically after the caller finishes writing.
pub fn publish_encrypted<T>(
    path: &Path,
    key: &DataKey,
    write: impl FnOnce(&mut EncryptedWriter<'_, &mut File>) -> Result<T, VaultError>,
) -> Result<T, VaultError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut staged = NamedTempFile::new_in(parent)?;
    let mut encrypted = EncryptedWriter::new(staged.as_file_mut(), key)?;
    let result = write(&mut encrypted)?;
    encrypted.finish()?.sync_all()?;
    staged.persist(path).map_err(|error| error.error)?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(result)
}

/// Read the compatible CBMAT4 header and locate its encrypted ZIP stream.
pub fn protected_package_header(
    input: &mut (impl Read + Seek),
) -> Result<(Value, u64), VaultError> {
    input.seek(SeekFrom::Start(0))?;
    let mut magic = [0; PROTECTED_PACKAGE_MAGIC.len()];
    input.read_exact(&mut magic)?;
    if &magic != PROTECTED_PACKAGE_MAGIC {
        return Err(VaultError::Format("Invalid protected package"));
    }
    let mut bytes = [0; 4];
    input.read_exact(&mut bytes)?;
    let length = u32::from_be_bytes(bytes) as usize;
    if !(1..=PACKAGE_HEADER_LIMIT).contains(&length) {
        return Err(VaultError::Format("Invalid protected package header"));
    }
    let mut document = vec![0; length];
    input.read_exact(&mut document)?;
    let envelope = serde_json::from_slice(&document)
        .map_err(|_| VaultError::Format("Invalid protected package header"))?;
    Ok((envelope, input.stream_position()?))
}

pub fn protected_package_writer<'a, W: Write>(
    mut output: W,
    envelope: &Value,
    key: &'a DataKey,
) -> Result<EncryptedWriter<'a, W>, VaultError> {
    let metadata = serde_json::to_vec(envelope)
        .map_err(|_| VaultError::Format("Invalid protected package header"))?;
    if metadata.len() > PACKAGE_HEADER_LIMIT {
        return Err(VaultError::Format("Invalid protected package header"));
    }
    output.write_all(PROTECTED_PACKAGE_MAGIC)?;
    output.write_all(&(metadata.len() as u32).to_be_bytes())?;
    output.write_all(&metadata)?;
    EncryptedWriter::new(output, key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opened_reader_rejects_valid_same_key_frame_substitution() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("artifact.bin");
        let key = DataKey::from_bytes([5; 32]);
        let initial = b"authentic original";
        let replacement = b"different payload!";
        assert_eq!(initial.len(), replacement.len());

        let encrypted = |bytes: &[u8]| {
            let mut writer = EncryptedWriter::new(Vec::new(), &key).unwrap();
            writer.write_all(bytes).unwrap();
            writer.finish().unwrap()
        };
        fs::write(&path, encrypted(initial)).unwrap();
        let mut snapshot = EncryptedReader::at(File::open(&path).unwrap(), 0, &key).unwrap();
        fs::write(&path, encrypted(replacement)).unwrap();
        assert!(snapshot.read_exact(&mut [0; 3]).is_err());
        let mut reopened = EncryptedReader::at(File::open(&path).unwrap(), 0, &key).unwrap();
        let mut new_payload = Vec::new();
        reopened.read_to_end(&mut new_payload).unwrap();
        assert_eq!(new_payload, replacement);
    }
}
