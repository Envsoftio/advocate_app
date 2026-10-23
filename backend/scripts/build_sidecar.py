"""Build a native PyInstaller sidecar for the current host only.

Usage: python backend/scripts/build_sidecar.py
Requires pyinstaller, rustc, and English and Hindi tessdata files. Run on the target OS.
"""
from __future__ import annotations
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
BACKEND = ROOT / 'backend'
TAURI_BINARIES = ROOT / 'frontend' / 'src-tauri' / 'binaries'


def detect_triple():
    output = subprocess.check_output(['rustc','-vV'], text=True)
    return next(line.partition(': ')[2].strip() for line in output.splitlines() if line.startswith('host:'))


def locate_tessdata(language: str):
    custom = os.getenv('TESSDATA_SOURCE')
    possibilities = [Path(custom)] if custom else []
    possibilities += [
        Path('/usr/share/tesseract-ocr/5/tessdata'),
        Path('/opt/homebrew/share/tessdata'),
        Path('/usr/local/share/tessdata'),
        Path(r'C:\Program Files\Tesseract-OCR\tessdata'),
    ]
    for path in possibilities:
        if path.is_dir():
            path = path / f'{language}.traineddata'
        if path.is_file() and path.name == f'{language}.traineddata':
            return path
    raise SystemExit(f'Missing {language} OCR model. Set TESSDATA_SOURCE to a tessdata directory containing eng.traineddata and hin.traineddata.')


def main():
    triple = detect_triple()
    try:
        import pypdfium2
        import pypdfium2.raw
        with pypdfium2.PdfDocument.new() as probe:
            probe.new_page(72, 72)
    except (ImportError, OSError) as exc:
        raise SystemExit('PDFium runtime is missing or unusable in this Python environment. '
                         'Install backend/requirements-dev.txt for this platform before building.') from exc
    tessdata = [locate_tessdata(language) for language in ('eng', 'hin')]
    addition = os.pathsep
    out = ROOT / '.build-sidecar'
    build_env = os.environ.copy()
    build_env['PYINSTALLER_CONFIG_DIR'] = str(out / 'config')
    cmd = [sys.executable, '-m', 'PyInstaller', '--onefile', '--noconfirm', '--clean',
           '--name', 'filing-api', '--distpath', str(out / 'dist'), '--workpath', str(out / 'work'),
           '--specpath', str(out), '--paths', str(BACKEND), '--collect-all', 'pymupdf',
           '--collect-all', 'pypdfium2', '--collect-all', 'pypdfium2_raw',
           '--add-data', f'{BACKEND / "app" / "rules" / "profiles.json"}{addition}app/rules',
           '--add-data', f'{BACKEND / "app" / "assets" / "editable-draft-template.pptx"}{addition}app/assets',
           *(item for path in tessdata for item in ('--add-data', f'{path}{addition}tessdata')),
           str(BACKEND / 'launcher.py')]
    print('Building native sidecar for', triple)
    subprocess.check_call(cmd, cwd=ROOT, env=build_env)
    extension = '.exe' if sys.platform == 'win32' else ''
    TAURI_BINARIES.mkdir(parents=True, exist_ok=True)
    built = out / 'dist' / ('filing-api' + extension)
    target = TAURI_BINARIES / ('filing-api-' + triple + extension)
    shutil.copy2(built, target)
    if sys.platform != 'win32':
        target.chmod(0o755)
    print('Created', target)

if __name__ == '__main__':
    main()
