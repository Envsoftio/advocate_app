"""Stage and hash the redistributable OCR runtime for a MatterDraft Rust bundle."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import re
import shutil
import struct
import subprocess
import tempfile
from urllib.request import urlopen

ROOT = Path(__file__).resolve().parents[2]
STAGING = ROOT / "frontend/src-tauri/ocr-runtime"
WINDOWS_MACHINES = {"x86_64": 0x8664, "arm64": 0xAA64}
SYSTEM_MACHO = ("/usr/lib/", "/System/Library/")
MODEL_RELEASE = "tessdata_fast 4.1.0"
MODEL_URL = "https://raw.githubusercontent.com/tesseract-ocr/tessdata_fast/4.1.0"
MODEL_HASHES = {
    "eng.traineddata": "7d4322bd2a7749724879683fc3912cb542f19906c83bcc1a52132556427170b2",
    "hin.traineddata": "4c73ffc59d497c186b19d1e90f5d721d678ea6b2e277b719bee4e2af12271825",
    "LICENSE": "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30",
}


def command(*args):
    return subprocess.check_output(args, text=True)


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def download_models(destination):
    destination.mkdir()
    for filename, expected in MODEL_HASHES.items():
        path = destination / filename
        with urlopen(f"{MODEL_URL}/{filename}", timeout=40) as source:
            content = source.read(16 * 1024 * 1024 + 1)
        if hashlib.sha256(content).hexdigest() != expected:
            raise ValueError(f"Upstream OCR model checksum mismatch for {filename}")
        path.write_bytes(content)
    return destination


def mac_minimum(path, arch):
    output = command("vtool", "-arch", arch, "-show-build", str(path))
    match = re.search(r"^\s*(?:minos|version) (\d+\.\d+(?:\.\d+)?)\s*$", output, re.MULTILINE)
    if not match:
        raise ValueError(f"{path} has no detectable macOS deployment target")
    return match.group(1)


def version_tuple(value):
    return tuple(int(part) for part in value.split(".")) + (0,) * (3 - len(value.split(".")))


def mac_libraries(entry, arch, minimum):
    pending = [entry]
    libraries = {}
    while pending:
        source = pending.pop().resolve(strict=True)
        if source.name in libraries:
            if libraries[source.name][0] != source:
                raise ValueError(f"Conflicting library basename {source.name}")
            continue
        arches = command("lipo", "-archs", str(source)).split()
        if arch not in arches:
            raise ValueError(f"{source} lacks the {arch} slice")
        deployed = mac_minimum(source, arch)
        if minimum and version_tuple(deployed) > version_tuple(minimum):
            raise ValueError(f"{source} requires macOS {deployed}, above the requested {minimum}")
        # Exclude LC_ID_DYLIB. The first dependency line is the ID of this dylib.
        links = command("otool", "-arch", arch, "-L", str(source)).splitlines()[2:]
        changes = {}
        for link in links:
            install_name = link.strip().partition(" (compatibility version")[0]
            if install_name.startswith(SYSTEM_MACHO):
                continue
            if install_name.startswith("@loader_path/"):
                dependency = source.parent / install_name.removeprefix("@loader_path/")
            elif install_name.startswith("@rpath/"):
                dependency = source.parent / install_name.removeprefix("@rpath/")
            elif install_name.startswith("/"):
                dependency = Path(install_name)
            else:
                raise ValueError(f"Unresolved install name {install_name} in {source}")
            dependency = dependency.resolve(strict=True)
            changes[install_name] = dependency.name
            pending.append(dependency)
        libraries[source.name] = (source, changes, arches, deployed)
    return libraries


def check_dll_architecture(path, machine):
    with path.open("rb") as dll:
        if dll.read(2) != b"MZ":
            raise ValueError(f"{path} is not a Windows DLL")
        dll.seek(0x3C)
        dll.seek(struct.unpack("<I", dll.read(4))[0])
        signature, actual = struct.unpack("<4sH", dll.read(6))
    if signature != b"PE\0\0" or actual != machine:
        raise ValueError(f"{path} does not match the target Windows architecture")


def windows_libraries(entry, arch, objdump):
    dumpbin = shutil.which("dumpbin") if not objdump else None
    if not (dumpbin or objdump):
        raise ValueError("Provide llvm-objdump or use a Visual Studio developer shell")
    system_names = {"kernel32.dll", "user32.dll", "gdi32.dll", "advapi32.dll",
                    "msvcrt.dll", "ucrtbase.dll", "ntdll.dll", "bcrypt.dll"}
    available = {path.name.lower(): path for path in entry.parent.iterdir() if path.is_file()}
    pending = [entry]
    libraries = {}
    while pending:
        library = pending.pop()
        name = library.name.lower()
        if name in libraries:
            continue
        check_dll_architecture(library, WINDOWS_MACHINES[arch])
        if objdump:
            listed = command(objdump, "-p", str(library))
            dependencies = re.findall(r"DLL Name:\s*([\w.+-]+\.dll)", listed, re.IGNORECASE)
        else:
            listed = command(dumpbin, "/DEPENDENTS", str(library))
            dependencies = re.findall(r"^\s+([\w.+-]+\.dll)\s*$", listed,
                                      flags=re.MULTILINE | re.IGNORECASE)
        if not dependencies:
            raise ValueError(f"No import table detected in {library}")
        for dependency in dependencies:
            dependency = dependency.lower()
            if dependency in available:
                pending.append(available[dependency])
            elif dependency.startswith(("api-ms-win-", "ext-ms-win-")):
                continue
            elif dependency not in system_names:
                raise ValueError(f"Missing redistributable dependency {dependency} for {library}")
        libraries[name] = library
    return libraries


def copy_licenses(source, destination):
    for name in ("LICENSE", "LICENSE.md", "LICENSE.txt", "COPYING", "NOTICE", "COPYRIGHT"):
        license_file = source / name
        if license_file.is_file():
            destination.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(license_file, destination / name)


def stage(args):
    host = platform.system()
    os_name = "Windows" if args.windows_objdump else host
    arch = args.arch or ("x86_64" if args.windows_objdump else platform.machine().lower().replace("amd64", "x86_64"))
    if arch == "aarch64":
        arch = "arm64"
    if os_name not in ("Darwin", "Windows") or arch not in WINDOWS_MACHINES:
        raise ValueError("Stage on the supported macOS or Windows build host")
    if os_name == "Windows" and not args.windows_objdump and arch != platform.machine().lower().replace("amd64", "x86_64"):
        raise ValueError("Stage on the target Windows architecture")
    entry = args.library.resolve(strict=True)
    if bool(args.tessdata) != bool(args.models_source):
        raise ValueError("Supply both --tessdata and --models-source to use local models")
    staging = args.output.absolute()
    staging.parent.mkdir(parents=True, exist_ok=True)
    if staging.exists() and (staging.is_symlink() or not (staging / "manifest.json").is_file()):
        raise ValueError(f"Refusing to replace an unmanaged directory: {staging}")
    with tempfile.TemporaryDirectory(prefix=".ocr-stage-", dir=staging.parent) as temporary:
        tessdata = (args.tessdata.resolve(strict=True) if args.tessdata
                    else download_models(Path(temporary) / "upstream-models"))
        candidate = Path(temporary) / "runtime"
        lib = candidate / "lib"
        lib.mkdir(parents=True)
        models = candidate / "tessdata"
        models.mkdir()
        for language in ("eng", "hin"):
            model = f"{language}.traineddata"
            shutil.copyfile(tessdata / model, models / model)
            if (models / model).stat().st_size == 0:
                raise ValueError(f"Empty OCR model {model}")
        copy_licenses(tessdata, candidate / "licenses" / "tessdata")
        if args.licenses:
            shutil.copytree(args.licenses, candidate / "licenses" / "redistribution")
        deployment_targets = {}
        if os_name == "Darwin":
            if not re.fullmatch(r"libtesseract\.5(?:\.\d+)*\.dylib", entry.name):
                raise ValueError("Use the Tesseract 5 dynamic library")
            entry_name = "libtesseract.5.dylib"
            libraries = mac_libraries(entry, arch, args.max_deployment_target)
            for name, (source, changes, arches, deployed) in libraries.items():
                staged_name = entry_name if source == entry else name
                target = lib / staged_name
                deployment_targets[staged_name] = deployed
                if len(arches) > 1:
                    command("lipo", str(source), "-thin", arch, "-output", str(target))
                else:
                    shutil.copyfile(source, target)
                target.chmod(0o755)
                edits = ["-id", f"@loader_path/{staged_name}"]
                for old, new in changes.items():
                    filename = entry_name if new == entry.name else new
                    edits.extend(("-change", old, f"@loader_path/{filename}"))
                command("install_name_tool", *edits, str(target))
                command("codesign", "--force", "--sign", args.signing_identity, str(target))
                copy_licenses(source.parent.parent, candidate / "licenses" / name)
        else:
            entry_name = entry.name.lower()
            for name, source in windows_libraries(entry, arch, args.windows_objdump).items():
                shutil.copyfile(source, lib / name)
            copy_licenses(entry.parent, candidate / "licenses" / "tesseract")
        manifest = {
            "target": f"{arch}-{os_name.lower()}",
            "entry": entry_name,
            "macos_deployment_targets": deployment_targets,
            "library_source": args.library_source,
            "models_source": args.models_source or MODEL_RELEASE,
            "sha256": {
                path.relative_to(candidate).as_posix(): digest(path)
                for path in sorted(candidate.rglob("*"))
                if path.is_file()
            },
        }
        (candidate / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n",
                                                encoding="utf-8")
        old = Path(temporary) / "previous"
        if staging.exists():
            staging.rename(old)
        try:
            candidate.rename(staging)
        except OSError:
            if old.exists():
                old.rename(staging)
            raise
    print(f"Staged {manifest['target']} OCR runtime in {staging}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--library", type=Path, required=True)
    parser.add_argument("--tessdata", type=Path, help="Use local models instead of the pinned upstream release")
    parser.add_argument("--library-source", required=True, help="Release or package provenance")
    parser.add_argument("--models-source", help="Source identification for local models")
    parser.add_argument("--licenses", type=Path, help="Additional redistribution notices")
    parser.add_argument("--arch", choices=WINDOWS_MACHINES)
    parser.add_argument("--output", type=Path, default=STAGING)
    parser.add_argument("--max-deployment-target", default="12.0",
                        help="Highest permissible native macOS minimum")
    parser.add_argument("--windows-objdump", help="llvm-objdump for validating a Windows DLL cross-build")
    parser.add_argument("--signing-identity", default="-", help="macOS code signing identity")
    args = parser.parse_args()
    try:
        stage(args)
    except (OSError, ValueError, subprocess.CalledProcessError, struct.error) as error:
        parser.exit(1, f"OCR runtime staging failed: {error}\n")


if __name__ == "__main__":
    main()
