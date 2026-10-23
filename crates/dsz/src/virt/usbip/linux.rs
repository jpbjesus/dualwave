//! Native Linux USB/IP import. The device server and every URB stay shared.
//!
//! ABI references: Linux tools/usb/usbip/libsrc/vhci_driver.c and
//! drivers/usb/usbip/vhci_sysfs.c (the kernel's public sysfs interface).
//! A helper receives an already imported loopback TCP socket via SCM_RIGHTS;
//! passing an fd number as text would address the helper's unrelated fd table.

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::wire;

const VHCI: &str = "/sys/devices/platform/vhci_hcd.0";
const HELPER: &str = "/run/dualwave/usbip.sock";
const DEVICE_BYTES: usize = 312;

#[derive(Debug, PartialEq)]
struct Port {
    number: u32,
    status: u32,
    super_speed: bool,
}

fn parse_status(text: &str) -> io::Result<Vec<Port>> {
    let mut ports = Vec::new();
    for line in text.lines().skip(1).filter(|l| !l.trim().is_empty()) {
        let cols: Vec<_> = line.split_whitespace().collect();
        if cols.len() < 5 || !matches!(cols[0], "hs" | "ss") {
            return Err(invalid("invalid vhci_hcd status row"));
        }
        ports.push(Port {
            number: cols[1].parse().map_err(|_| invalid("invalid vhci port"))?,
            status: cols[2].parse().map_err(|_| invalid("invalid vhci status"))?,
            super_speed: cols[0] == "ss",
        });
    }
    if ports.is_empty() { return Err(invalid("vhci_hcd reports no ports")); }
    Ok(ports)
}

fn invalid(message: &str) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, message) }

fn writable_controls() -> bool {
    ["attach", "detach"].iter().all(|name| {
        OpenOptions::new().write(true).open(Path::new(VHCI).join(name)).is_ok()
    })
}

pub(super) fn blocked() -> Option<String> {
    if !Path::new(VHCI).join("status").is_file() {
        return Some("Linux USB/IP host is unavailable: load the vhci_hcd kernel module (sudo modprobe vhci_hcd), then enable DualWave virtual-controller setup.".into());
    }
    if !writable_controls() && !fs::metadata(HELPER).is_ok_and(|m| m.file_type().is_socket()) {
        return Some(format!("Linux USB/IP needs the DualWave privileged attach service at {HELPER}. Run virtual-controller setup; do not run the desktop app as root."));
    }
    None
}

#[derive(Debug, PartialEq)]
pub(super) struct Imported {
    devid: u32,
    speed: u32,
}

fn imported_device(common: &[u8; 8], bytes: &[u8; DEVICE_BYTES], busid: &str) -> io::Result<Imported> {
    if *common != wire::op_common(wire::OP_REP_IMPORT, 0) {
        return Err(invalid("USB/IP import rejected or incompatible version"));
    }
    let busid_end = bytes[256..288].iter().position(|&b| b == 0)
        .ok_or_else(|| invalid("unterminated USB/IP bus id"))?;
    if &bytes[256..256 + busid_end] != busid.as_bytes() {
        return Err(invalid("USB/IP import returned a different bus id"));
    }
    let read = |at| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
    let (bus, device, speed) = (read(288), read(292), read(296));
    if bus > 0xFFFF || device > 0xFFFF || !matches!(speed, 1..=3) {
        return Err(invalid("USB/IP import returned invalid device identity or speed"));
    }
    Ok(Imported { devid: bus << 16 | device, speed })
}

pub(super) fn import_socket(busid: &str, addr: SocketAddr) -> io::Result<(TcpStream, Imported)> {
    if !addr.ip().is_loopback() || busid.is_empty() || busid.len() >= 32 || busid.as_bytes().contains(&0) {
        return Err(invalid("USB/IP import requires a loopback server and a valid bus id"));
    }
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut request = wire::op_common(wire::OP_REQ_IMPORT, 0).to_vec();
    request.extend_from_slice(busid.as_bytes());
    request.resize(40, 0);
    stream.write_all(&request)?;
    let mut common = [0; 8];
    stream.read_exact(&mut common)?;
    // A rejected import has no device payload.
    if common != wire::op_common(wire::OP_REP_IMPORT, 0) {
        return Err(invalid("USB/IP server rejected the import"));
    }
    let mut bytes = [0; DEVICE_BYTES];
    stream.read_exact(&mut bytes)?;
    let device = imported_device(&common, &bytes, busid)?;
    // These options belong to the open socket description transferred to vhci.
    stream.set_read_timeout(None)?;
    stream.set_write_timeout(None)?;
    Ok((stream, device))
}

pub(super) fn attach(busid: &str, addr: SocketAddr) -> io::Result<u32> {
    let (stream, device) = import_socket(busid, addr)?;
    if writable_controls() {
        direct_attach(stream.as_raw_fd(), &device)
    } else {
        let response = helper(&format!("ATTACH {} {}\n", device.devid, device.speed), Some(stream.as_raw_fd()))?;
        response.strip_prefix("OK ").and_then(|p| p.parse::<u32>().ok())
            .filter(|&p| p < i32::MAX as u32)
            .ok_or_else(|| invalid("USB/IP helper returned an invalid port"))
    }
    // Drop only closes our copy: vhci/helper owns its imported socket reference.
}

fn direct_attach(fd: RawFd, device: &Imported) -> io::Result<u32> {
    let mut status_files: Vec<_> = fs::read_dir(VHCI)?.filter_map(Result::ok)
        .map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| {
            let name = n.to_string_lossy();
            name == "status" || name.strip_prefix("status.").is_some_and(|s| s.parse::<u32>().is_ok())
        })).collect();
    status_files.sort();
    for file in status_files {
        for port in parse_status(&fs::read_to_string(file)?)? {
            // VDEV_ST_NULL = 4; full/high speed controllers use the HS root hub.
            if port.status != 4 || port.super_speed { continue; }
            let command = format!("{} {fd} {} {}", port.number, device.devid, device.speed);
            match OpenOptions::new().write(true).open(Path::new(VHCI).join("attach"))?.write_all(command.as_bytes()) {
                Ok(()) => return Ok(port.number),
                Err(e) if e.raw_os_error() == Some(libc::EBUSY) => continue,
                Err(e) => return Err(e),
            }
        }
    }
    Err(io::Error::new(io::ErrorKind::AddrNotAvailable, "vhci_hcd has no free USB 2 ports"))
}

pub(super) fn detach(port: u32) -> io::Result<()> {
    if writable_controls() {
        OpenOptions::new().write(true).open(Path::new(VHCI).join("detach"))?
            .write_all(port.to_string().as_bytes())
    } else {
        let reply = helper(&format!("DETACH {port}\n"), None)?;
        if reply == "OK" { Ok(()) } else { Err(invalid("USB/IP helper returned an invalid detach reply")) }
    }
}

fn helper(request: &str, fd: Option<RawFd>) -> io::Result<String> {
    let mut socket = UnixStream::connect(HELPER).map_err(|e| io::Error::new(e.kind(), format!("DualWave USB/IP attach service {HELPER}: {e}; run virtual-controller setup")))?;
    socket.set_read_timeout(Some(Duration::from_secs(10)))?;
    socket.set_write_timeout(Some(Duration::from_secs(10)))?;
    if let Some(fd) = fd { send_fd(&socket, request.as_bytes(), fd)?; }
    else { socket.write_all(request.as_bytes())?; }
    // Bound both time and bytes; a daemon error must never hang virtual-pad teardown.
    let mut response = Vec::new();
    for _ in 0..1024 {
        let mut byte = [0];
        socket.read_exact(&mut byte)?;
        if byte[0] == b'\n' {
            let text = String::from_utf8(response).map_err(|_| invalid("USB/IP helper reply is not UTF-8"))?;
            return if let Some(why) = text.strip_prefix("ERR ") {
                Err(io::Error::other(format!("DualWave USB/IP attach service: {why}")))
            } else { Ok(text) };
        }
        response.push(byte[0]);
    }
    Err(invalid("USB/IP helper reply too long"))
}

fn send_fd(socket: &UnixStream, request: &[u8], fd: RawFd) -> io::Result<()> {
    // usize storage guarantees cmsghdr alignment; CMSG_SPACE includes padding.
    let space = unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) as usize };
    let mut ancillary = vec![0usize; space.div_ceil(std::mem::size_of::<usize>())];
    let mut iov = libc::iovec { iov_base: request.as_ptr() as *mut _, iov_len: request.len() };
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = ancillary.as_mut_ptr() as *mut _;
    message.msg_controllen = space;
    let count = unsafe {
        let header = libc::CMSG_FIRSTHDR(&message);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as usize;
        std::ptr::write(libc::CMSG_DATA(header) as *mut RawFd, fd);
        libc::sendmsg(socket.as_raw_fd(), &message, libc::MSG_NOSIGNAL)
    };
    if count < 0 { return Err(io::Error::last_os_error()); }
    // Stream sockets may accept only part of the payload; rights accompany the first byte.
    (&*socket).write_all(&request[count as usize..])
}

pub(super) fn is_ours_instance(id: &str) -> bool {
    let Ok(path) = fs::canonicalize(id) else { return false };
    if !path.starts_with("/sys/devices") || !path.components().any(|c| c.as_os_str().to_string_lossy().starts_with("vhci_hcd.")) { return false; }
    path.ancestors().any(|p| fs::read_to_string(p.join("serial")).is_ok_and(|serial| {
        let s = serial.trim();
        s.starts_with(super::SERIAL_PREFIX) || s.starts_with(super::DS4_SERIAL_PREFIX)
    }))
}

pub(super) fn hid_for_serial(serial: &str) -> Option<String> {
    fs::read_dir("/sys/class/hidraw").ok()?.filter_map(Result::ok).find_map(|entry| {
        let path: PathBuf = fs::canonicalize(entry.path().join("device")).ok()?;
        (is_ours_instance(&path.to_string_lossy()) && path.ancestors().any(|p| {
            fs::read_to_string(p.join("serial")).is_ok_and(|s| s.trim() == serial)
        })).then(|| path.to_string_lossy().into_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vhci_status_uses_decimal_ports_and_skips_busy_and_superspeed() {
        let ports = parse_status("hub port sta spd dev sockfd local_busid\nhs 0010 004 000 00000000 000000 0-0\nhs 0011 006 003 00010002 000031 1-2\nss 0018 004 000 00000000 000000 0-0\n").unwrap();
        assert_eq!(ports, [Port { number: 10, status: 4, super_speed: false }, Port { number: 11, status: 6, super_speed: false }, Port { number: 18, status: 4, super_speed: true }]);
        assert!(parse_status("hub port sta\nhs bad 004 0 0\n").is_err());
        assert!(parse_status("hub port sta\n").is_err());
    }

    #[test]
    fn import_keeps_usbip_network_order_and_device_identity() {
        let info = wire::DeviceInfo { busid: "1-3", busnum: 1, devnum: 3, speed: 3, vid: 0x054C, pid: 0x0CE6, bcd_device: 0x0100, num_interfaces: 4 };
        let bytes: [u8; 312] = wire::usb_device(&info).try_into().unwrap();
        let reply = wire::op_common(wire::OP_REP_IMPORT, 0);
        assert_eq!(&reply, &[1, 17, 0, 3, 0, 0, 0, 0]);
        assert_eq!(&bytes[288..304], &[0, 0, 0, 1, 0, 0, 0, 3, 0, 0, 0, 3, 5, 76, 12, 230]);
        assert_eq!(imported_device(&reply, &bytes, "1-3").unwrap(), Imported { devid: 0x10003, speed: 3 });
        assert!(imported_device(&reply, &bytes, "1-4").is_err());
        assert!(imported_device(&wire::op_common(wire::OP_REP_IMPORT, 4), &bytes, "1-3").is_err());
        let mut invalid_speed = bytes;
        invalid_speed[296..300].copy_from_slice(&9u32.to_be_bytes());
        assert!(imported_device(&reply, &invalid_speed, "1-3").is_err());
    }

    #[test]
    fn helper_receives_an_owned_socket_instead_of_a_foreign_fd_number() {
        use std::os::fd::{FromRawFd, OwnedFd};
        let (sender, receiver) = UnixStream::pair().unwrap();
        let (mut payload, remote) = UnixStream::pair().unwrap();
        send_fd(&sender, b"ATTACH 65537 3\n", remote.as_raw_fd()).unwrap();
        drop(remote);
        let mut bytes = [0u8; 64];
        let mut iov = libc::iovec { iov_base: bytes.as_mut_ptr() as *mut _, iov_len: bytes.len() };
        let mut ancillary = [0usize; 8];
        let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        message.msg_control = ancillary.as_mut_ptr() as *mut _;
        message.msg_controllen = std::mem::size_of_val(&ancillary);
        let received = unsafe { libc::recvmsg(receiver.as_raw_fd(), &mut message, libc::MSG_CMSG_CLOEXEC) };
        assert_eq!(received, 15);
        assert_eq!(&bytes[..15], b"ATTACH 65537 3\n");
        let owned = unsafe {
            let header = libc::CMSG_FIRSTHDR(&message);
            assert!(!header.is_null());
            assert_eq!((*header).cmsg_level, libc::SOL_SOCKET);
            assert_eq!((*header).cmsg_type, libc::SCM_RIGHTS);
            OwnedFd::from_raw_fd(std::ptr::read(libc::CMSG_DATA(header) as *const RawFd))
        };
        let mut transferred = UnixStream::from(owned);
        payload.write_all(b"native socket ownership").unwrap();
        let mut contents = [0; 23];
        transferred.read_exact(&mut contents).unwrap();
        assert_eq!(&contents, b"native socket ownership");
    }

    #[test]
    fn descriptor_read_does_not_consume_the_first_urb() {
        let bytes = wire::usb_device(&wire::DeviceInfo { busid: "1-1", busnum: 1, devnum: 1, speed: 3, vid: 0x054C, pid: 0x0CE6, bcd_device: 0x100, num_interfaces: 4 });
        let mut transport = io::Cursor::new([wire::op_common(wire::OP_REP_IMPORT, 0).as_slice(), bytes.as_slice(), &[0x12, 0x34]].concat());
        let mut reply = [0; 8];
        let mut device = [0; 312];
        transport.read_exact(&mut reply).unwrap();
        transport.read_exact(&mut device).unwrap();
        assert_eq!(imported_device(&reply, &device, "1-1").unwrap().devid, 65537);
        let mut next = [0; 2];
        transport.read_exact(&mut next).unwrap();
        assert_eq!(next, [0x12, 0x34]);
    }
}
