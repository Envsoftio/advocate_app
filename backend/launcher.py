"""Executable entrypoint for uvicorn and PyInstaller sidecar."""
import argparse
import os
from pathlib import Path
import sys
import threading
import time


def parent_alive(pid: int) -> bool:
    if sys.platform == 'win32':
        import ctypes
        kernel32 = ctypes.windll.kernel32
        kernel32.OpenProcess.argtypes = (ctypes.c_uint32, ctypes.c_int, ctypes.c_uint32)
        kernel32.OpenProcess.restype = ctypes.c_void_p
        kernel32.WaitForSingleObject.argtypes = (ctypes.c_void_p, ctypes.c_uint32)
        kernel32.WaitForSingleObject.restype = ctypes.c_uint32
        kernel32.CloseHandle.argtypes = (ctypes.c_void_p,)
        handle = kernel32.OpenProcess(0x00100000, False, pid)  # SYNCHRONIZE
        if not handle:
            return False
        try:
            return kernel32.WaitForSingleObject(handle, 0) == 0x00000102  # WAIT_TIMEOUT
        finally:
            kernel32.CloseHandle(handle)
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def stop_when_parent_exits(server, parent_pid: int) -> None:
    while not server.should_exit:
        if not parent_alive(parent_pid):
            server.should_exit = True
            return
        time.sleep(0.5)


def main():
    parser = argparse.ArgumentParser(description='CourtBundle local processing service')
    parser.add_argument('--port', type=int, default=8765)
    parser.add_argument('--data-dir', type=str, default=None)
    parser.add_argument('--parent-pid', type=int, default=None)
    args = parser.parse_args()
    if not 1024 <= args.port <= 65535:
        parser.error('Port must be between 1024 and 65535')
    if not os.getenv('COURTBUNDLE_API_TOKEN'):
        print('COURTBUNDLE_API_TOKEN environment variable is required.', file=sys.stderr)
        sys.exit(2)
    if args.parent_pid is not None and args.parent_pid <= 0:
        parser.error('Parent PID must be positive')
    # PyInstaller one-file mode extracts bundled OCR traineddata under _MEIPASS.
    if getattr(sys, 'frozen', False):
        tessdata = Path(sys._MEIPASS) / 'tessdata'
        if tessdata.is_dir():
            os.environ['TESSDATA_PREFIX'] = str(tessdata)
    import uvicorn
    from app.main import create_app
    config = uvicorn.Config(create_app(data_dir=args.data_dir), host='127.0.0.1', port=args.port,
                            access_log=False, log_level='info')
    server = uvicorn.Server(config)
    if args.parent_pid is not None:
        threading.Thread(target=stop_when_parent_exits, args=(server, args.parent_pid),
                         daemon=True).start()
    server.run()


if __name__ == '__main__':
    main()
