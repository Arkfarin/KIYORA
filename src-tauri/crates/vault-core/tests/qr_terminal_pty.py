"""Account-free Linux PTY acceptance for the Rust test-only terminal child."""
import errno
import fcntl
import os
import pty
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time


class Child:
    def __init__(self, root, term="xterm-256color", phone=False):
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.environ["KIYORA_TEST_AUTH_TERMINAL"] = root
            os.environ["KIYORA_TEST_AUTH_PHONE"] = "1" if phone else "0"
            os.environ["TERM"] = term
            os.execv(sys.argv[1], [sys.argv[1], "--exact",
                "auth_flow::tests::native::terminal_fixture_child", "--nocapture"])
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
        self.all = b""
        self.pending = b""
        self.root = root
        self.reaped = False

    def until(self, text):
        token = text.encode()
        deadline = time.monotonic() + 20
        while token not in self.pending:
            assert time.monotonic() < deadline, "timed out waiting for " + text
            if not select.select([self.fd], [], [], 0.1)[0]:
                continue
            try:
                chunk = os.read(self.fd, 65536)
            except OSError as error:
                if error.errno == errno.EIO:
                    chunk = b""
                else:
                    raise
            assert chunk, "terminal child exited before " + text
            self.all += chunk
            self.pending += chunk
        _, self.pending = self.pending.split(token, 1)

    def send(self, text):
        os.write(self.fd, text)

    def control(self, command):
        profiles = os.path.join(self.root, "data", "profiles")
        profile = os.listdir(profiles)[0]
        path = os.path.join(profiles, profile, "tdlib", "fixture-control")
        with open(path + ".tmp", "w", encoding="utf-8") as file:
            file.write(command)
        os.replace(path + ".tmp", path)

    def finish(self):
        self.until("PTY_CLOSED")
        flags = termios.tcgetattr(self.fd)[3]
        assert flags & termios.ECHO and flags & termios.ICANON, "terminal mode not restored"
        _, status = os.waitpid(self.pid, 0)
        self.reaped = True
        assert os.waitstatus_to_exitcode(status) == 0, "terminal fixture failed"
        assert b"tg://" not in self.all and b"PRIVATE" not in self.all, "credential echoed"

    def cleanup(self):
        if not self.reaped:
            os.kill(self.pid, signal.SIGKILL)
            os.waitpid(self.pid, 0)
        os.close(self.fd)


def run(root, case, term="xterm-256color", phone=False):
    child = Child(root, term, phone)
    try:
        case(child)
        child.finish()
        return child.all
    finally:
        child.cleanup()


def cancel_qr(child):
    child.until("Waiting…")
    competing_env = dict(os.environ, KIYORA_TEST_AUTH_TERMINAL=child.root,
                         KIYORA_TEST_AUTH_PHONE="0")
    competing = subprocess.run([sys.argv[1], "--exact",
        "auth_flow::tests::native::terminal_fixture_child", "--nocapture"],
        env=competing_env, capture_output=True, timeout=15, check=False)
    assert competing.returncode != 0
    assert b"ProfileBusy" in competing.stdout + competing.stderr
    child.control("rotate")
    child.until("Waiting…")
    child.send(b"\x03")
    child.until("Authorization cancelled")


def signal_qr(child):
    child.until("Waiting…")
    os.kill(child.pid, signal.SIGINT)
    child.until("Authorization cancelled")


def two_factor(child):
    child.until("Waiting…")
    child.control("WaitPassword")
    child.until("2FA password (or /cancel):")
    child.send(b"WRONG_PASSWORD_PRIVATE\r")
    child.until("InvalidPassword")
    child.until("2FA password (or /cancel):")
    child.send(b"PASSWORD_PRIVATE\r")
    child.until("Telegram authorization successful!")


def pending_code(child):
    child.until("Telegram phone number")
    child.send(b"PRIVATE_PHONE\r")
    child.until("Telegram code (or")
    child.send(b"INTERRUPTED_CODE_PRIVATE\r")
    # The next challenge update arrives while the native query's reply is withheld.
    # Ctrl+C must still be read while Backend::authenticate is pending.
    child.send(b"\x03")
    child.until("Authorization cancelled")


with tempfile.TemporaryDirectory() as root:
    ansi = run(root, cancel_qr)
    assert ansi.count(b"\x1b[?1049h") >= 2
    assert ansi.count(b"\x1b[2J") >= 2
    run(root, signal_qr)
    run(root, two_factor)
    run(root, lambda child: child.until("Telegram is already authorized."))
with tempfile.TemporaryDirectory() as root:
    plain = run(root, cancel_qr, term="dumb")
    assert b"\x1b[" not in plain
    assert b"Previous QR is no longer current" in plain
with tempfile.TemporaryDirectory() as root:
    run(root, pending_code, phone=True)
print("PTY: rotation, Ctrl+C, SIGINT, hidden 2FA, restart, dumb fallback and pending request passed")
