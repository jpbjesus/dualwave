#!/usr/bin/python3 -I
"""DualWave's socket-activated, root-only USB/IP host broker.

The unprivileged app completes IMPORT before passing one connected TCP socket.
No USB/IP packets are read here: the next byte belongs to the kernel URB reader.
ABI and status-field sources (Linux, GPL-2.0+):
https://github.com/torvalds/linux/blob/v6.17/drivers/usb/usbip/vhci_sysfs.c
https://github.com/torvalds/linux/blob/v6.17/tools/usb/usbip/libsrc/vhci_driver.c
https://docs.kernel.org/usb/usbip_protocol.html
SCM_RIGHTS / SO_PEERCRED and activation:
https://man7.org/linux/man-pages/man7/unix.7.html
https://www.freedesktop.org/software/systemd/man/latest/sd_listen_fds.html

Ownership is deliberately NOT recovered after a broker restart: sysfs exposes
no generation ID, so persisted records could authorize detaching a reused port.
Existing busy ports are never adopted. Restart the app to create fresh imports;
the kernel releases old imports when their USB/IP server closes the connection.
"""

import array
import dataclasses
import errno
import fcntl
import grp
import ipaddress
import logging
import os
from pathlib import Path
import re
import signal
import socket
import stat
import struct
import time


SOCKET_PATH = Path('/run/dualwave/usbip.sock')
VHCI_PATH = Path('/sys/devices/platform/vhci_hcd.0')
MAX_REQUEST = 128
REQUEST_TIMEOUT = 3.0
FREE = 4
USED = 6


class Rejected(Exception):
    """An untrusted request or a port whose ownership cannot be established."""


def decimal(value, maximum):
    if not re.fullmatch(r'[0-9]{1,10}', value):
        raise Rejected('expected an unsigned decimal integer')
    result = int(value, 10)
    if result > maximum:
        raise Rejected('integer out of range')
    return result


@dataclasses.dataclass(frozen=True)
class Peer:
    pid: int
    uid: int
    starttime: int


def process_identity(pid, proc=Path('/proc')):
    """Read twice around credentials so an exited/reused PID fails closed."""
    def start():
        # comm may contain spaces and ')'; field 22 follows the last ')'.
        fields = (proc / str(pid) / 'stat').read_text().rsplit(')', 1)[1].split()
        if fields[0] in ('Z', 'X', 'x'):
            raise Rejected('peer process has exited')
        return int(fields[19])

    try:
        before = start()
        fields = {}
        for line in (proc / str(pid) / 'status').read_text().splitlines():
            if ':' in line:
                key, value = line.split(':', 1)
                fields[key] = value.split()
        uids = tuple(int(value) for value in fields['Uid'])
        gids = {int(value) for value in fields['Gid'] + fields['Groups']}
        after = start()
        if before != after or len(set(uids)) != 1:
            raise Rejected('peer credentials changed')
        return Peer(pid, uids[0], before), gids
    except (OSError, ValueError, IndexError, KeyError) as exc:
        raise Rejected('cannot verify peer process') from exc


def authenticate(connection, allowed_gid):
    pid, uid, gid = struct.unpack('3i', connection.getsockopt(
        socket.SOL_SOCKET, socket.SO_PEERCRED, struct.calcsize('3i')))
    peer, groups = process_identity(pid)
    if peer.uid != uid or (uid != 0 and allowed_gid not in groups | {gid}):
        raise Rejected('peer is not authorized for the dualwave group')
    return peer


def peer_alive(peer):
    try:
        return process_identity(peer.pid)[0] == peer
    except Rejected:
        return False


def close_fds(descriptors):
    for fd in descriptors:
        os.close(fd)


def receive_request(connection):
    """Capture rights on EVERY fragment and close them on every error path."""
    data = bytearray()
    descriptors = []
    deadline = time.monotonic() + REQUEST_TIMEOUT
    try:
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise Rejected('request timed out')
            connection.settimeout(remaining)
            chunk, ancillary, flags, _ = connection.recvmsg(
                MAX_REQUEST + 1 - len(data), socket.CMSG_SPACE(4 * array.array('i').itemsize),
                socket.MSG_CMSG_CLOEXEC)
            bad_control = False
            for level, kind, payload in ancillary:
                if level == socket.SOL_SOCKET and kind == socket.SCM_RIGHTS:
                    fds = array.array('i')
                    complete = len(payload) - len(payload) % fds.itemsize
                    fds.frombytes(payload[:complete])
                    descriptors.extend(fds)
                    bad_control |= complete != len(payload)
                else:
                    bad_control = True
            if flags & (socket.MSG_CTRUNC | socket.MSG_TRUNC) or bad_control:
                raise Rejected('truncated or unexpected ancillary data')
            if len(descriptors) > 1:
                raise Rejected('expected at most one file descriptor')
            if not chunk:
                raise Rejected('request ended before newline')
            data.extend(chunk)
            if len(data) > MAX_REQUEST:
                raise Rejected('request too long')
            if b'\n' in data:
                if data.count(b'\n') != 1 or not data.endswith(b'\n'):
                    raise Rejected('expected exactly one request')
                try:
                    request = data[:-1].decode('ascii')
                except UnicodeDecodeError as exc:
                    raise Rejected('request must be ASCII') from exc
                return request, descriptors
    except BaseException:
        close_fds(descriptors)
        raise


def validate_import(fd):
    """Check the shared description without changing flags or consuming bytes."""
    # socket(fileno=...) owns the descriptor, so use a temporary duplicate.
    duplicate = os.dup(fd)
    try:
        imported = socket.socket(fileno=duplicate)
    except OSError as exc:
        os.close(duplicate)
        raise Rejected('import is not a connected TCP socket') from exc
    try:
        with imported:
            family = imported.getsockopt(socket.SOL_SOCKET, socket.SO_DOMAIN)
            if family not in (socket.AF_INET, socket.AF_INET6):
                raise Rejected('import must be an IPv4 or IPv6 TCP socket')
            if imported.getsockopt(socket.SOL_SOCKET, socket.SO_TYPE) != socket.SOCK_STREAM:
                raise Rejected('import must be a TCP stream')
            if imported.getsockopt(socket.SOL_SOCKET, socket.SO_PROTOCOL) != socket.IPPROTO_TCP:
                raise Rejected('import must use TCP')
            for endpoint in (imported.getsockname(), imported.getpeername()):
                address = ipaddress.ip_address(endpoint[0])
                if isinstance(address, ipaddress.IPv6Address) and address.ipv4_mapped:
                    address = address.ipv4_mapped
                if not address.is_loopback:
                    raise Rejected('import endpoints must be loopback')
            if fcntl.fcntl(fd, fcntl.F_GETFL) & os.O_NONBLOCK:
                raise Rejected('import must be a blocking TCP socket')
            for option in (socket.SO_RCVTIMEO, socket.SO_SNDTIMEO):
                if any(imported.getsockopt(socket.SOL_SOCKET, option, struct.calcsize('@ll'))):
                    raise Rejected('import must have no socket timeout')
            # TCP_ESTABLISHED = 1. A socket with FIN queued is no longer usable.
            if imported.getsockopt(socket.IPPROTO_TCP, socket.TCP_INFO, 1) != b'\x01':
                raise Rejected('import TCP connection is not established')
    except OSError as exc:
        raise Rejected('import is not a connected TCP socket') from exc


@dataclasses.dataclass(frozen=True)
class Port:
    number: int
    hub: str
    status: int
    speed: int
    devid: int
    sockfd: int


class Vhci:
    def __init__(self, path=VHCI_PATH):
        self.path = Path(path)

    def ports(self):
        ports = {}
        files = sorted(p for p in self.path.iterdir()
                       if re.fullmatch(r'status(?:\.[0-9]+)?', p.name))
        if not files:
            raise Rejected('vhci_hcd is unavailable; install virtual-controller support and reboot')
        for path in files:
            with path.open('r', encoding='ascii') as stream:
                text = stream.read(1024 * 1024 + 1)
            if len(text) > 1024 * 1024:
                raise Rejected('vhci status too long')
            lines = text.splitlines()
            if not lines or lines[0].split() != ['hub', 'port', 'sta', 'spd', 'dev', 'sockfd', 'local_busid']:
                raise Rejected('unsupported vhci status header')
            for line in lines[1:]:
                cols = line.split()
                if len(cols) != 7 or cols[0] not in ('hs', 'ss') or not re.fullmatch(r'[0-9a-fA-F]{8}', cols[4]):
                    raise Rejected('invalid vhci status row')
                port = Port(decimal(cols[1], 0x7fffffff), cols[0], decimal(cols[2], 0xffffffff),
                            decimal(cols[3], 6), int(cols[4], 16), decimal(cols[5], 0x7fffffff))
                if port.number in ports:
                    raise Rejected('duplicate vhci port')
                ports[port.number] = port
        if not ports:
            raise Rejected('vhci_hcd reports no ports')
        return ports

    def _write(self, name, command):
        # A sysfs transaction is one write, without truncation or file creation.
        fd = os.open(self.path / name, os.O_WRONLY | os.O_CLOEXEC | os.O_NOFOLLOW)
        try:
            if os.write(fd, command.encode('ascii')) != len(command):
                raise OSError(errno.EIO, 'short vhci control write')
        finally:
            os.close(fd)

    def attach(self, port, fd, devid, speed):
        self._write('attach', f'{port} {fd} {devid} {speed}')

    def detach(self, port):
        self._write('detach', str(port))


@dataclasses.dataclass
class Owned:
    peer: Peer
    port: int
    devid: int
    speed: int
    fd: int

    def matches(self, status):
        return (status is not None and status.status == USED and
                (status.devid, status.speed, status.sockfd) == (self.devid, self.speed, self.fd))


class Broker:
    """Single event-loop owner: allocation, detach and watchdog are serialized."""
    def __init__(self, vhci=None, alive=peer_alive):
        self.vhci = vhci if vhci is not None else Vhci()
        self.alive = alive
        self.owned = {}

    def drop(self, port):
        owner = self.owned.pop(port, None)
        if owner is not None:
            os.close(owner.fd)

    def reap(self):
        if not self.owned:
            return
        ports = self.vhci.ports()
        for number, owner in list(self.owned.items()):
            current = ports.get(number)
            if current is not None and current.status == 5:
                # The kernel hides identity until USB enumeration completes.
                continue
            if not owner.matches(current):
                self.drop(number)
            elif not self.alive(owner.peer):
                self.vhci.detach(number)
                self.drop(number)

    def request(self, peer, request, descriptors):
        self.reap()
        if not self.alive(peer):
            raise Rejected('peer process has exited or changed identity')
        attach = re.fullmatch(r'ATTACH ([0-9]+) ([0-9]+)', request)
        detach = re.fullmatch(r'DETACH ([0-9]+)', request)
        if attach:
            devid = decimal(attach[1], 0xffffffff)
            speed = decimal(attach[2], 6)
            if speed not in (1, 2, 3, 4, 5, 6) or devid == 0:
                raise Rejected('invalid USB device identity or speed')
            if len(descriptors) != 1:
                raise Rejected('ATTACH requires exactly one imported descriptor')
            validate_import(descriptors[0])
            for port in sorted(self.vhci.ports().values(), key=lambda p: p.number):
                if port.status != FREE or port.number in self.owned or (port.hub == 'ss') != (speed >= 5):
                    continue
                # Keep this fd allocated for the entire ownership lifetime. The
                # kernel reports its original numeric value in status.sockfd.
                fd = os.dup(descriptors[0])
                try:
                    self.vhci.attach(port.number, fd, devid, speed)
                except OSError as exc:
                    os.close(fd)
                    if exc.errno == errno.EBUSY:
                        continue
                    raise
                self.owned[port.number] = Owned(peer, port.number, devid, speed, fd)
                return f'OK {port.number}\n'
            raise Rejected('vhci_hcd has no free port for this USB speed')
        if detach:
            port = decimal(detach[1], 0x7fffffff)
            if descriptors:
                raise Rejected('DETACH does not accept descriptors')
            owner = self.owned.get(port)
            if owner is None or owner.peer != peer:
                raise Rejected('port is not owned by this process')
            if not owner.matches(self.vhci.ports().get(port)):
                raise Rejected('port identity changed or is not yet available')
            self.vhci.detach(port)
            self.drop(port)
            return 'OK\n'
        raise Rejected('expected ATTACH <devid> <speed> or DETACH <port>')

    def serve_connection(self, connection, allowed_gid):
        descriptors = []
        try:
            peer = authenticate(connection, allowed_gid)
            request, descriptors = receive_request(connection)
            reply = self.request(peer, request, descriptors)
        except (Rejected, OSError, UnicodeError) as exc:
            # Do not return paths, fd numbers, or arbitrary exception text.
            reason = str(exc) if isinstance(exc, Rejected) else 'host operation failed; check the service journal'
            reply = f'ERR {reason}\n'
            logging.warning('USB/IP request rejected: %s', exc)
        finally:
            close_fds(descriptors)
        try:
            connection.settimeout(1)
            connection.sendall(reply.encode('ascii'))
        except OSError:
            # A successful attach whose response was lost remains owned by the
            # authenticated process; only that owner or the watchdog can detach.
            pass

    def close(self):
        # No blind sysfs cleanup on shutdown: a reused port must stay untouched.
        for port in list(self.owned):
            self.drop(port)


def validate_listener(listener, allowed_gid, path=SOCKET_PATH, owner_uid=0):
    """Reject a replaced node or a socket reachable through a writable parent."""
    path = Path(path)
    if (listener.family != socket.AF_UNIX or listener.type != socket.SOCK_STREAM or
            not listener.getsockopt(socket.SOL_SOCKET, socket.SO_ACCEPTCONN) or
            listener.getsockname() != str(path)):
        raise Rejected('expected the configured listening Unix stream socket')
    node = path.lstat()
    if (not stat.S_ISSOCK(node.st_mode) or node.st_uid != owner_uid or
            node.st_gid != allowed_gid or stat.S_IMODE(node.st_mode) != 0o660):
        raise Rejected('unsafe USB/IP socket ownership or mode')
    for parent in path.parents:
        metadata = parent.lstat()
        if (not stat.S_ISDIR(metadata.st_mode) or metadata.st_uid != owner_uid or
                stat.S_IMODE(metadata.st_mode) & 0o022):
            raise Rejected('unsafe USB/IP socket parent directory')


def activated_listener(allowed_gid):
    if os.environ.get('LISTEN_PID') != str(os.getpid()) or os.environ.get('LISTEN_FDS') != '1':
        raise Rejected('start this broker through dualwave-usbip.socket')
    listener = socket.socket(fileno=3)
    os.set_inheritable(3, False)
    validate_listener(listener, allowed_gid)
    return listener


def main():
    logging.basicConfig(level=logging.INFO, format='%(levelname)s: %(message)s')
    if os.geteuid() != 0:
        raise Rejected('the USB/IP host broker requires its root system service')
    allowed_gid = grp.getgrnam('dualwave').gr_gid
    listener = activated_listener(allowed_gid)
    # systemd owns this directory; flock also prevents manual duplicate brokers.
    lock = os.open('/run/dualwave/usbip.lock', os.O_WRONLY | os.O_CREAT | os.O_CLOEXEC | os.O_NOFOLLOW, 0o600)
    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    broker = Broker()
    running = True

    def stop(_signal, _frame):
        nonlocal running
        running = False

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    listener.settimeout(1)
    try:
        while running:
            try:
                connection, _ = listener.accept()
            except socket.timeout:
                pass
            else:
                with connection:
                    broker.serve_connection(connection, allowed_gid)
            try:
                broker.reap()
            except (Rejected, OSError, UnicodeError) as exc:
                logging.warning('USB/IP ownership check deferred: %s', exc)
    finally:
        broker.close()
        listener.close()
        os.close(lock)


if __name__ == '__main__':
    try:
        main()
    except (Rejected, OSError, KeyError) as error:
        logging.error('%s', error)
        raise SystemExit(1) from None
