//! JSON-lines driver and on-disk format checks for disposable parity fixtures.
use matterdraft_backend::{
    matters::PageOutput,
    secure_db,
    vault::{self, Credential, DataKey, EncryptedReader},
    vault_store::{ImportOptions, VaultStore},
};
use serde_json::{json, Value};
use std::{
    fs::{self, File},
    io::{self, BufRead, Read, Seek, SeekFrom, Write},
    path::Path,
};

fn check_formats(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let config: Value = serde_json::from_reader(File::open(root.join("python.json"))?)?;
    let password = config["password"].as_str().unwrap();
    let recovery = config["recovery"].as_str().unwrap();
    let key = vault::unlock_key(&config["envelope"], Credential::Password(password))?;
    let recovery_key = vault::unlock_key(&config["envelope"], Credential::Recovery(recovery))?;
    let plain = fs::read(root.join("plain.bin"))?;
    let mut outer = EncryptedReader::at(File::open(root.join("nested.bin"))?, 0, &key)?;
    let (metadata, offset) = vault::protected_package_header(&mut outer)?;
    let mut inner = EncryptedReader::at(outer, offset, &recovery_key)?;
    assert_eq!(metadata, config["envelope"]);
    inner.seek(SeekFrom::Start(vault::CHUNK_BYTES as u64 - 7))?;
    let mut across_frames = [0; 48];
    inner.read_exact(&mut across_frames)?;
    assert_eq!(
        &across_frames,
        &plain[vault::CHUNK_BYTES - 7..vault::CHUNK_BYTES + 41]
    );
    inner.seek(SeekFrom::End(-13))?;
    let mut tail = Vec::new();
    inner.read_to_end(&mut tail)?;
    assert_eq!(tail, &plain[plain.len() - 13..]);

    let created = vault::create_key_envelope(password)?;
    let output = root.join("rust.bin");
    vault::publish_encrypted(&output, &created.data_key, |writer| {
        writer.write_all(&plain)?;
        Ok(())
    })?;
    let before = fs::read(&output)?;
    let failed = vault::publish_encrypted(&output, &created.data_key, |writer| {
        writer.write_all(&plain)?;
        Err::<(), _>(vault::VaultError::Format("Injected interruption"))
    });
    assert!(failed.is_err());
    assert_eq!(fs::read(output)?, before);
    let mut package = vault::protected_package_writer(
        File::create(root.join("rust.cbmatter"))?,
        &created.envelope,
        &created.data_key,
    )?;
    io::copy(&mut File::open(root.join("plain.zip"))?, &mut package)?;
    package.finish()?;

    let python_db = root.join("python.sqlite3");
    let database = secure_db::open_protected(&python_db, &key)?;
    let name: String = database.query_row("SELECT name FROM sample", [], |row| row.get(0))?;
    assert_eq!(name, "Python synthetic row");
    assert!(secure_db::open_protected(&python_db, &DataKey::from_bytes([0; 32])).is_err());
    let from_rust = root.join("rust.sqlite3");
    secure_db::create_protected(&from_rust, &created.data_key)?;
    secure_db::open_protected(&from_rust, &created.data_key)?.execute_batch(
        "CREATE TABLE sample (name TEXT); INSERT INTO sample VALUES ('Rust synthetic row')",
    )?;
    let legacy = root.join("legacy.sqlite3");
    let migrated = root.join("migrated.sqlite3");
    assert!(secure_db::open_protected(&legacy, &created.data_key).is_err());
    secure_db::migrate_plain_database(&legacy, &migrated, &created.data_key)?;
    assert!(secure_db::migrate_plain_database(&legacy, &migrated, &created.data_key).is_err());
    println!(
        "{}",
        json!({"envelope":created.envelope,"recovery":&*created.recovery_key})
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    let operation = arguments.next().expect("fixture operation");
    let root = arguments.next().expect("disposable fixture root");
    if operation == "formats" {
        return check_formats(Path::new(&root));
    }
    if let Some(resources) = std::env::var_os("MATTERDRAFT_TEST_OCR_RESOURCE_ROOT") {
        matterdraft_backend::matters::set_ocr_resource_root(resources.into(), true);
    }
    let mut vault = VaultStore::open(Path::new(&root))?;
    for line in io::stdin().lock().lines() {
        let input: Value = serde_json::from_str(&line?)?;
        let method = input["method"].as_str().unwrap();
        let path = input["path"].as_str().unwrap();
        let reply = if method == "IMPORT" {
            let options = ImportOptions {
                password: input["password"].as_str(),
                acknowledge_signature_risk: input["acknowledge_signature_risk"] == true,
                included: input["included"] == true,
                source_file_mtime: input["source_file_mtime"].as_str().unwrap_or_default(),
            };
            let file = Path::new(input["source"].as_str().unwrap());
            vault.import_pdf(
                path,
                file.file_name().unwrap().to_str().unwrap(),
                &fs::read(file)?,
                options,
            )
        } else if method == "REPLACE_RAW" {
            use matterdraft_backend::matters::RawReplacementUpload;
            (|| {
                let raw = RawReplacementUpload::parse(input["metadata"].as_str().unwrap())?;
                let work = vault.prepare_working_replacement(path, &raw.fields)?;
                let source = Path::new(input["source"].as_str().unwrap());
                let bytes = fs::read(source).expect("synthetic replacement fixture");
                vault.complete_working_replacement(work.compute(&raw.filename, &bytes)?)
            })()
            .unwrap_or_else(|refusal| refusal)
        } else if method == "RUN_PENDING" {
            if let Some(work) = vault.begin_work()? {
                vault.finish_work(work.compute())?;
            }
            matterdraft_backend::Reply {
                status: 200,
                body: json!(null),
            }
        } else if method == "SAVE_BACKUP" {
            let destination = Path::new(input["destination"].as_str().unwrap());
            match vault.write_portable_backup(path, File::create(destination)?) {
                Ok(file) => {
                    file.sync_all()?;
                    matterdraft_backend::Reply {
                        status: 200,
                        body: json!(null),
                    }
                }
                Err(refusal) => refusal,
            }
        } else if method == "RESTORE_BACKUP" {
            let source = Path::new(input["source"].as_str().unwrap());
            vault.restore_portable(File::open(source)?, input["password"].as_str(), false)
        } else if method == "PAGE_OUTPUT" {
            let operation = match input["operation"].as_str().unwrap() {
                "extract" => PageOutput::Extract,
                "split" => PageOutput::Split,
                _ => unreachable!(),
            };
            match vault.page_output(
                path,
                operation,
                input["selection"].as_str().unwrap(),
                input["acknowledge_signature_risk"] == true,
            ) {
                Ok(bytes) => matterdraft_backend::Reply {
                    status: 200,
                    body: json!({"bytes":bytes}),
                },
                Err(refusal) => refusal,
            }
        } else if method == "SNAPSHOT" {
            let source = serde_json::from_value(input["body"].clone()).unwrap();
            let id = path.split('/').nth(2).unwrap();
            match vault.open_pdf(id, &source) {
                Ok(mut pdf) => {
                    let mut bytes = Vec::new();
                    pdf.copy_to(&mut bytes).unwrap();
                    matterdraft_backend::Reply {
                        status: 200,
                        body: serde_json::json!({"length":pdf.bytes,"bytes":bytes}),
                    }
                }
                Err(refusal) => refusal,
            }
        } else {
            let order = matterdraft_backend::RequestOrder::from_json(input["key_source"].as_str());
            vault.request_with_order(method, path, input["body"].clone(), &order)
        };
        let mut output = io::stdout().lock();
        serde_json::to_writer(&mut output, &reply)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
    Ok(())
}
