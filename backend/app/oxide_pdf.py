"""Explicit opt-in Rust extraction boundary; failures never silently switch engines."""
from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import threading
import sys
from contextlib import contextmanager

from .file_io import read_bytes, size, iter_plain_chunks

WORKER_TIMEOUT = 60
MAX_RESPONSE_BYTES = 64 * 1024 * 1024
MAX_INPUT_BYTES = 256 * 1024 * 1024


@contextmanager
def stream_pages(path: Path):
    """Versioned page stream with backpressure and deterministic early-close cleanup."""
    worker = configured_worker()
    if not worker or not Path(worker).is_absolute():
        raise ValueError('PDF Oxide requires an absolute worker executable path')
    process = None
    feeder = timer = None
    failed = threading.Event()
    timed_out = threading.Event()
    try:
        process = subprocess.Popen([worker, '--stream'], stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)

        def terminate():
            if process.poll() is None:
                try:
                    process.kill()
                except ProcessLookupError:
                    pass

        def expire():
            timed_out.set()
            terminate()

        def feed():
            total = 0
            try:
                for chunk in iter_plain_chunks(path):
                    total += len(chunk)
                    if total > MAX_INPUT_BYTES:
                        raise ValueError('Input limit')
                    process.stdin.write(chunk)
            except Exception:
                failed.set()
                terminate()
            finally:
                try:
                    process.stdin.close()
                except OSError:
                    pass

        timer = threading.Timer(WORKER_TIMEOUT, expire)
        feeder = threading.Thread(target=feed)
        timer.start()
        feeder.start()

        def record():
            line = process.stdout.readline(MAX_RESPONSE_BYTES + 1)
            if not line.endswith(b'\n') or len(line) > MAX_RESPONSE_BYTES:
                raise ValueError('Invalid stream frame')
            result = json.loads(line)
            if not isinstance(result, dict):
                raise ValueError('Invalid stream record')
            return result

        header = record()
        count = header.get('page_count')
        if (header.get('protocol') != 2 or header.get('kind') != 'header'
                or header.get('engine') != 'pdf_oxide' or header.get('version') != '0.3.78'
                or type(count) is not int or count < 0):
            raise ValueError('Invalid stream header')

        def pages():
            try:
                for index in range(count):
                    item = record()
                    if (item.get('kind') != 'page' or type(item.get('page')) is not int
                            or item['page'] != index or not isinstance(item.get('text'), str)
                            or len(item['text'].encode('utf-8')) > 10_000_000):
                        raise ValueError('Invalid stream page')
                    if failed.is_set() or timed_out.is_set():
                        raise ValueError('Worker failed')
                    yield item['text']
                if record() != {'kind': 'end', 'page_count': count}:
                    raise ValueError('Missing stream completion')
                if process.stdout.read(1):
                    raise ValueError('Trailing stream data')
                process.wait()
                if process.returncode or failed.is_set() or timed_out.is_set():
                    raise ValueError('Worker failed')
            except (OSError, ValueError, KeyError, TypeError):
                raise ValueError('PDF Oxide search extraction failed') from None

        yield pages()
    except (OSError, ValueError, KeyError, TypeError):
        raise ValueError('PDF Oxide search extraction failed') from None
    finally:
        if timer is not None:
            timer.cancel()
            timer.join()
        if process is not None:
            if process.poll() is None:
                process.kill()
            process.wait()
            if feeder is not None:
                feeder.join()
            process.stdin.close()
            process.stdout.close()


def configured_worker() -> str | None:
    explicit = os.environ.get('MATTERDRAFT_PDF_OXIDE_WORKER')
    if explicit:
        return explicit
    if os.environ.get('MATTERDRAFT_PDF_OXIDE_BUNDLED') == '1':
        if not getattr(sys, 'frozen', False) or not hasattr(sys, '_MEIPASS'):
            raise ValueError('Bundled PDF Oxide requires a packaged backend')
        name = 'matterdraft-pdf-oxide' + ('.exe' if sys.platform == 'win32' else '')
        return str(Path(sys._MEIPASS) / 'pdf-tools' / name)
    return None


def _run(worker: str, data: bytes, page: int | None) -> bytes:
    """Bound captured output; never spool decrypted text to disk."""
    command = [worker] + ([] if page is None else ['--page', str(page)])
    with subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                          stderr=subprocess.DEVNULL) as process:
        output = bytearray()
        failed = threading.Event()

        def feed():
            try:
                process.stdin.write(data)
            except (BrokenPipeError, OSError):
                pass
            finally:
                try:
                    process.stdin.close()
                except OSError:
                    pass

        def drain():
            try:
                while chunk := process.stdout.read(65536):
                    if len(output) + len(chunk) > MAX_RESPONSE_BYTES:
                        failed.set()
                        process.kill()
                        return
                    output.extend(chunk)
            except OSError:
                failed.set()

        threads = [threading.Thread(target=feed), threading.Thread(target=drain)]
        for thread in threads:
            thread.start()
        try:
            process.wait(timeout=WORKER_TIMEOUT)
        finally:
            if process.poll() is None:
                process.kill()
            process.wait()
            for thread in threads:
                thread.join()
        if process.returncode or failed.is_set():
            raise ValueError('Worker failed')
        return bytes(output)


def extract_pages(path: Path, *, page: int | None = None) -> list[str]:
    if page is not None and (type(page) is not int or page < 0):
        raise ValueError('Choose a non-negative PDF page index')
    worker = configured_worker()
    if not worker or not Path(worker).is_absolute():
        raise ValueError('PDF Oxide requires an absolute worker executable path')
    if size(path) > 256 * 1024 * 1024:
        raise ValueError('PDF Oxide input exceeds the 256 MiB limit')
    try:
        payload = json.loads(_run(worker, read_bytes(path), page))
        if not isinstance(payload, dict):
            raise ValueError('Invalid worker response')
        pages = payload['pages']
        if (payload.get('protocol') != 1 or payload.get('engine') != 'pdf_oxide'
                or payload.get('version') != '0.3.78'
                or not isinstance(pages, list) or len(pages) > 2000
                or any(not isinstance(page, str) for page in pages)
                or sum(map(len, pages)) > 10_000_000):
            raise ValueError('Invalid worker response')
        count, start = payload.get('page_count'), payload.get('start_page')
        if (type(count) is not int or count < 0 or type(start) is not int
                or start != (page or 0)
                or (page is None and len(pages) != count)
                or (page is not None and (len(pages) != 1 or page >= count))):
            raise ValueError('Invalid page response')
        return pages
    except (OSError, subprocess.SubprocessError, ValueError, KeyError, TypeError):
        raise ValueError('PDF Oxide extraction failed; no review draft was created') from None
