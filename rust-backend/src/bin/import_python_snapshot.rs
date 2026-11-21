use matterdraft_backend::{vault::Credential, vault_store::VaultStore};
use std::{
    io::{self, BufRead},
    path::Path,
    process::ExitCode,
};
use zeroize::Zeroizing;

fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let (snapshot, destination, recovery) = match args.as_slice() {
        [snapshot, destination] => (snapshot, destination, false),
        [snapshot, destination, flag] if flag == "--recovery" => (snapshot, destination, true),
        _ => return Err("Usage: import_python_snapshot SNAPSHOT NEW_VAULT [--recovery]. Supply the password or recovery key on stdin.".into()),
    };
    let mut credential = Zeroizing::new(String::new());
    io::stdin()
        .lock()
        .read_line(&mut credential)
        .map_err(|error| error.to_string())?;
    let credential = credential.trim_end_matches(['\r', '\n']);
    let credential = if recovery {
        Credential::Recovery(credential)
    } else {
        Credential::Password(credential)
    };
    VaultStore::import_python_snapshot(Path::new(snapshot), Path::new(destination), credential)
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => {
            println!("Converted the detached vault snapshot.");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
