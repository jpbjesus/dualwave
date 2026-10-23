"""The host broker is never started as root; all controls are ordinary files."""

import array
from dataclasses import replace
import importlib.util
import os
from pathlib import Path
import socket
import struct
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch


SOURCE = Path(__file__).resolve().parents[1] / 'linux/usbip-host.py'
SPEC = importlib.util.spec_from_file_location('dualwave_usbip_host', SOURCE)
host = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = host
SPEC.loader.exec_module(host)


def tcp_pair(family=socket.AF_INET):
    listener = socket.socket(family, socket.SOCK_STREAM)
    address = '127.0.0.1' if family == socket.AF_INET else '::1'
    listener.bind((address, 0))
    listener.listen(1)
    client = socket.socket(family, socket.SOCK_STREAM)
    client.connect(listener.getsockname())
    server, _ = listener.accept()
    listener.close()
    return client, server


def rights(fds):
    return [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array('i', fds))]


class FakeVhci(host.Vhci):
    """Use the real parser/write method, emulating kernel status transitions."""
    def __init__(self, path):
        super().__init__(path)
        self.rows = {
            0: host.Port(0, 'hs', host.FREE, 0, 0, 0),
            8: host.Port(8, 'ss', host.FREE, 0, 0, 0),
            16: host.Port(16, 'hs', host.FREE, 0, 0, 0),
        }
        self.commands = []
        self.busy = set()
        for name in ('attach', 'detach'):
            (self.path / name).touch()
        self.status()

    def status(self):
        header = 'hub port sta spd dev sockfd local_busid\n'
        for name, numbers in (('status', (0, 8)), ('status.1', (16,))):
            rows = ''.join(f'{p.hub} {p.number:04d} {p.status:03d} {p.speed:03d} '
                           f'{p.devid:08x} {p.sockfd:06d} 1-1\n'
                           for p in (self.rows[n] for n in numbers))
            (self.path / name).write_text(header + rows)

    def attach(self, port, fd, devid, speed):
        if port in self.busy:
            raise OSError(16, 'port raced with another importer')
        super().attach(port, fd, devid, speed)
        self.commands.append(('attach', port, fd, devid, speed))
        self.rows[port] = replace(self.rows[port], status=host.USED, speed=speed, devid=devid, sockfd=fd)
        self.status()

    def detach(self, port):
        super().detach(port)
        self.commands.append(('detach', port))
        self.rows[port] = replace(self.rows[port], status=host.FREE, speed=0, devid=0, sockfd=0)
        self.status()


class HostTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.vhci = FakeVhci(Path(self.temporary.name))
        self.peer = host.process_identity(os.getpid())[0]
        self.broker = host.Broker(self.vhci, alive=lambda _peer: True)
        self.addCleanup(self.broker.close)
        self.client, self.server = tcp_pair()
        self.addCleanup(self.client.close)
        self.addCleanup(self.server.close)

    def attach(self, peer=None, speed=3, devid=65539):
        return self.broker.request(peer or self.peer, f'ATTACH {devid} {speed}', [self.client.fileno()])

    def exchange(self, message, fds=(), shutdown=True):
        sender, receiver = socket.socketpair()
        self.addCleanup(sender.close)
        self.addCleanup(receiver.close)
        sender.sendmsg([message], rights(fds) if fds else [])
        if shutdown:
            sender.shutdown(socket.SHUT_WR)
        self.broker.serve_connection(receiver, os.getgid())
        return sender.recv(1024)

    def test_exact_client_protocol_transfers_real_tcp_fd_and_keeps_urb_bytes(self):
        self.server.sendall(b'first URB remains unread')
        self.assertEqual(self.exchange(b'ATTACH 65539 3\n', [self.client.fileno()]), b'OK 0\n')
        owned = self.broker.owned[0]
        self.assertNotEqual(owned.fd, self.client.fileno())
        self.assertEqual(os.read(owned.fd, len(b'first URB remains unread')), b'first URB remains unread')
        self.assertEqual((self.vhci.path / 'attach').read_text(), f'0 {owned.fd} 65539 3')
        self.assertEqual(self.exchange(b'DETACH 0\n'), b'OK\n')
        with self.assertRaises(OSError):
            os.fstat(owned.fd)

    def test_speeds_choose_correct_hub_and_status_dot_files_use_raw_ports(self):
        self.assertEqual(self.attach(speed=5), 'OK 8\n')
        self.assertEqual(self.attach(speed=2), 'OK 0\n')
        self.assertEqual(self.attach(speed=3), 'OK 16\n')
        with self.assertRaisesRegex(host.Rejected, 'no free port'):
            self.attach(speed=1)

    def test_wireless_speed_four_uses_hs_per_kernel_abi(self):
        self.assertEqual(self.attach(speed=4), 'OK 0\n')

    def test_kernel_busy_race_retries_another_free_port(self):
        self.vhci.busy.add(0)
        self.assertEqual(self.attach(), 'OK 16\n')

    def test_foreign_uid_pid_and_process_starttime_cannot_detach(self):
        self.attach()
        for peer in (replace(self.peer, uid=self.peer.uid + 1),
                     replace(self.peer, pid=self.peer.pid + 1),
                     replace(self.peer, starttime=self.peer.starttime + 1)):
            with self.subTest(peer=peer), self.assertRaisesRegex(host.Rejected, 'not owned'):
                self.broker.request(peer, 'DETACH 0', [])
        self.assertFalse(any(command[0] == 'detach' for command in self.vhci.commands))

    def test_reused_port_identity_is_dropped_without_detaching(self):
        for field, value in (('devid', 0x20003), ('sockfd', 9999), ('speed', 2)):
            with self.subTest(field=field):
                self.attach()
                self.vhci.rows[0] = replace(self.vhci.rows[0], **{field: value})
                self.vhci.status()
                with self.assertRaisesRegex(host.Rejected, 'not owned'):
                    self.broker.request(self.peer, 'DETACH 0', [])
                self.assertNotIn(0, self.broker.owned)
                self.vhci.rows[0] = host.Port(0, 'hs', host.FREE, 0, 0, 0)
                self.vhci.status()
        self.assertFalse(any(command[0] == 'detach' for command in self.vhci.commands))

    def test_watchdog_detaches_dead_owner_only_when_live_status_matches(self):
        self.attach()
        self.broker.alive = lambda _peer: False
        self.broker.reap()
        self.assertEqual(self.vhci.commands[-1], ('detach', 0))
        self.assertFalse(self.broker.owned)

    def test_watchdog_drops_reused_port_even_with_dead_owner(self):
        self.attach()
        self.broker.alive = lambda _peer: False
        self.vhci.rows[0] = replace(self.vhci.rows[0], devid=0x90009)
        self.vhci.status()
        self.broker.reap()
        self.assertEqual(len(self.vhci.commands), 1)
        self.assertFalse(self.broker.owned)

    def test_unassigned_status_hides_identity_and_never_authorizes_detach(self):
        self.attach()
        self.vhci.rows[0] = replace(self.vhci.rows[0], status=5, devid=0, speed=0, sockfd=0)
        self.vhci.status()
        with self.assertRaisesRegex(host.Rejected, 'not yet available'):
            self.broker.request(self.peer, 'DETACH 0', [])
        self.broker.alive = lambda _peer: False
        self.broker.reap()
        self.assertIn(0, self.broker.owned)
        self.assertEqual(len(self.vhci.commands), 1)

    def test_restart_does_not_adopt_an_existing_busy_port(self):
        self.attach()
        restarted = host.Broker(self.vhci, alive=lambda _peer: True)
        self.addCleanup(restarted.close)
        with self.assertRaisesRegex(host.Rejected, 'not owned'):
            restarted.request(self.peer, 'DETACH 0', [])
        self.assertEqual(restarted.request(self.peer, 'ATTACH 65539 3', [self.client.fileno()]), 'OK 16\n')

    def test_disconnected_request_does_not_hand_port_to_another_process(self):
        sender, receiver = socket.socketpair()
        sender.sendmsg([b'ATTACH 65539 3\n'], rights([self.client.fileno()]))
        sender.close()
        with receiver:
            self.broker.serve_connection(receiver, os.getgid())
        self.assertEqual(self.broker.owned[0].peer, self.peer)
        with self.assertRaisesRegex(host.Rejected, 'not owned'):
            self.broker.request(replace(self.peer, starttime=self.peer.starttime + 1), 'DETACH 0', [])

    def test_malformed_numbers_whitespace_commands_and_descriptor_counts(self):
        for text in ('ATTACH -1 3', 'ATTACH +1 3', 'ATTACH 0x10003 3',
                     'ATTACH 4294967296 3', 'ATTACH 1 7', 'ATTACH 0 3',
                     'ATTACH 1 0', 'ATTACH 1 03 ', 'ATTACH 1\t3',
                     'DETACH -1', 'DETACH 2147483648', 'DETACH 0 ignored',
                     'ATTACH 1111111111111111111111111111 3'):
            with self.subTest(text=text), self.assertRaises(host.Rejected):
                self.broker.request(self.peer, text, [self.client.fileno()])
        with self.assertRaisesRegex(host.Rejected, 'exactly one'):
            self.broker.request(self.peer, 'ATTACH 65539 3', [])
        with self.assertRaisesRegex(host.Rejected, 'does not accept'):
            self.broker.request(self.peer, 'DETACH 0', [self.client.fileno()])
        self.assertFalse(self.vhci.commands)

    def test_request_truncation_overflow_nonascii_and_extra_commands_close_all_rights(self):
        baseline = len(list(Path('/proc/self/fd').iterdir()))
        for message in (b'ATTACH 65539 3', b'X' * 129 + b'\n', b'ATTACH \xff 3\n',
                        b'ATTACH 65539 3\nDETACH 0\n', b'ATTACH 65539 3\r\n'):
            with self.subTest(message=message):
                sender, receiver = socket.socketpair()
                with sender, receiver:
                    sender.sendmsg([message], rights([self.client.fileno()]))
                    sender.shutdown(socket.SHUT_WR)
                    self.broker.serve_connection(receiver, os.getgid())
                    self.assertTrue(sender.recv(1024).startswith(b'ERR '))
        self.assertEqual(len(list(Path('/proc/self/fd').iterdir())), baseline)
        self.assertFalse(self.vhci.commands)

    def test_ancillary_truncation_and_multiple_rights_are_rejected_without_leaks(self):
        baseline = len(list(Path('/proc/self/fd').iterdir()))
        for count in (2, 20):
            sender, receiver = socket.socketpair()
            with sender, receiver:
                sender.sendmsg([b'ATTACH 65539 3\n'], rights([self.client.fileno()] * count))
                with self.assertRaises(host.Rejected):
                    host.receive_request(receiver)
        self.assertEqual(len(list(Path('/proc/self/fd').iterdir())), baseline)

    def test_fragmented_request_captures_rights_on_later_fragment(self):
        sender, receiver = socket.socketpair()
        with sender, receiver:
            sender.sendall(b'ATTACH ')
            thread = threading.Thread(target=lambda: sender.sendmsg([b'65539 3\n'], rights([self.client.fileno()])))
            thread.start()
            request, descriptors = host.receive_request(receiver)
            thread.join()
            try:
                self.assertEqual(request, 'ATTACH 65539 3')
                self.assertEqual(len(descriptors), 1)
            finally:
                host.close_fds(descriptors)

    def test_request_timeout_is_bounded(self):
        sender, receiver = socket.socketpair()
        with sender, receiver, patch.object(host, 'REQUEST_TIMEOUT', 0.01):
            with self.assertRaises(TimeoutError):
                host.receive_request(receiver)

    def test_non_tcp_unconnected_and_regular_descriptors_are_rejected_without_leaks(self):
        unix = socket.socketpair()
        udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        disconnected = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        with unix[0], unix[1], udp, disconnected, (self.vhci.path / 'attach').open('r') as regular:
            baseline = len(list(Path('/proc/self/fd').iterdir()))
            for fd in (unix[0].fileno(), udp.fileno(), disconnected.fileno(), regular.fileno()):
                with self.subTest(fd=fd), self.assertRaises(host.Rejected):
                    host.validate_import(fd)
            self.assertEqual(len(list(Path('/proc/self/fd').iterdir())), baseline)

    def test_import_rejects_nonloopback_local_endpoint_without_network_traffic(self):
        # A loopback destination with a nonloopback source stays on this host.
        probe = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        with probe:
            probe.connect(('192.0.2.1', 9))  # connect(UDP) only selects a route.
            local = probe.getsockname()[0]
        if local.startswith('127.') or local == '0.0.0.0':
            self.skipTest('no nonloopback local address')
        listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        client = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        with listener, client:
            listener.bind(('127.0.0.1', 0))
            listener.listen(1)
            client.bind((local, 0))
            client.connect(listener.getsockname())
            accepted, _ = listener.accept()
            with accepted, self.assertRaisesRegex(host.Rejected, 'loopback'):
                host.validate_import(client.fileno())

    def test_import_rejects_nonblocking_and_kernel_socket_timeouts(self):
        self.client.setblocking(False)
        with self.assertRaisesRegex(host.Rejected, 'blocking'):
            host.validate_import(self.client.fileno())
        self.client.setblocking(True)
        self.client.setsockopt(socket.SOL_SOCKET, socket.SO_RCVTIMEO, struct.pack('@ll', 1, 0))
        with self.assertRaisesRegex(host.Rejected, 'no socket timeout'):
            host.validate_import(self.client.fileno())

    def test_ipv6_loopback_is_supported(self):
        try:
            client, server = tcp_pair(socket.AF_INET6)
        except OSError:
            self.skipTest('IPv6 loopback unavailable')
        with client, server:
            host.validate_import(client.fileno())

    def test_authentication_uses_peer_credentials_and_supplementary_groups(self):
        sender, receiver = socket.socketpair()
        with sender, receiver:
            self.assertEqual(host.authenticate(receiver, os.getgid()), self.peer)
            with self.assertRaisesRegex(host.Rejected, 'not authorized'):
                host.authenticate(receiver, 2147483647)
            with patch.object(host, 'process_identity', return_value=(replace(self.peer, uid=99999), {os.getgid()})):
                with self.assertRaisesRegex(host.Rejected, 'not authorized'):
                    host.authenticate(receiver, os.getgid())

    def test_dead_owner_cannot_attach(self):
        self.broker.alive = lambda _peer: False
        with self.assertRaisesRegex(host.Rejected, 'exited'):
            self.attach()
        self.assertFalse(self.vhci.commands)

    def test_symlink_control_files_and_malformed_status_are_rejected(self):
        (self.vhci.path / 'attach').unlink()
        (self.vhci.path / 'attach').symlink_to(self.vhci.path / 'detach')
        with self.assertRaises(OSError):
            self.attach()
        (self.vhci.path / 'status').write_text('hub port sta spd dev sockfd local_busid\nhs 0000 004 000 NOTHEX 000000 0-0\n')
        with self.assertRaises(host.Rejected):
            self.vhci.ports()


class SocketOwnershipTest(unittest.TestCase):
    def test_listener_node_owner_group_mode_and_secure_ancestry(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'usbip.sock'
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
                listener.bind(str(path))
                listener.listen(1)
                path.chmod(0o660)
                # Fake only parent ownership: tests do not chown or touch /run.
                real_lstat = Path.lstat

                def metadata(node):
                    original = real_lstat(node)
                    if node == path:
                        return original
                    values = list(original)
                    values[0] = 0o40755
                    values[4] = os.getuid()
                    return os.stat_result(values)

                with patch.object(Path, 'lstat', metadata):
                    host.validate_listener(listener, os.getgid(), path, owner_uid=os.getuid())
                    for mode in (0o666, 0o600, 0o777):
                        path.chmod(mode)
                        with self.assertRaisesRegex(host.Rejected, 'ownership or mode'):
                            host.validate_listener(listener, os.getgid(), path, owner_uid=os.getuid())
                    path.chmod(0o660)
                    with self.assertRaisesRegex(host.Rejected, 'ownership or mode'):
                        host.validate_listener(listener, os.getgid() + 1, path, owner_uid=os.getuid())
                    with self.assertRaisesRegex(host.Rejected, 'ownership or mode'):
                        host.validate_listener(listener, os.getgid(), path, owner_uid=os.getuid() + 1)
                with self.assertRaisesRegex(host.Rejected, 'parent directory'):
                    host.validate_listener(listener, os.getgid(), path, owner_uid=os.getuid())
                path.unlink()
                path.touch(mode=0o660)
                with self.assertRaisesRegex(host.Rejected, 'ownership or mode'):
                    host.validate_listener(listener, os.getgid(), path, owner_uid=os.getuid())

    def test_unactivated_start_is_rejected_before_fd_access(self):
        with patch.dict(os.environ, {}, clear=True), self.assertRaisesRegex(host.Rejected, 'through dualwave-usbip.socket'):
            host.activated_listener(os.getgid())


if __name__ == '__main__':
    unittest.main()
