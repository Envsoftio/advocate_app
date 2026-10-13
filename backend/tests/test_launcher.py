import os
from threading import Event

import launcher


def test_parent_liveness_for_current_process():
    assert launcher.parent_alive(os.getpid())


def test_watchdog_requests_shutdown_when_desktop_exits(monkeypatch):
    class Server:
        should_exit = False

    server = Server()
    checked = Event()

    def missing_parent(pid):
        assert pid == 12345
        checked.set()
        return False

    monkeypatch.setattr(launcher, 'parent_alive', missing_parent)
    launcher.stop_when_parent_exits(server, 12345)
    assert checked.is_set()
    assert server.should_exit
