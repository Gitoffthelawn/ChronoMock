//! The TCP sockets this machine's processes are listening on, with the owning pid of each.
//!
//! An embedded web engine opens its DevTools port inside the session's own process family - the
//! WebView2 browser process, or the host itself for Qt WebEngine - on a port nobody told us. The
//! engine writes the number to a file in its profile, but a file is one engine's convention and it
//! appears when the engine feels like it. The kernel's table is every engine's and it is current:
//! `GetExtendedTcpTable` with the owner-pid listener class lists every LISTEN socket with the pid
//! that bound it, for IPv4 and for IPv6, and that list joined with the family's pids is the port.
//!
//! This reads a table and opens nothing. The connection to a port found here is made by the
//! registered socket code in the CLI, after its loopback-and-port check - see the register in
//! `crates/cli/tests/network.rs`, which names this feature and this reason.

use std::ffi::c_void;

use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCP6TABLE_OWNER_PID, MIB_TCPROW_OWNER_PID,
    MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
};

/// One listening socket: who bound it, on which port, whether it is bound to a loopback address -
/// the only kind of endpoint the session will ever speak to - on which address family, because the
/// loopback name to connect to differs between the two, and for IPv4 the exact local address, because
/// one question needs more than "loopback": whether this socket stands in the way of a bind to
/// `127.0.0.1` on the same port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Listener {
    pub pid: u32,
    pub port: u16,
    pub loopback: bool,
    pub v6: bool,
    /// The IPv4 local address as the table carries it (network byte order), zero for an IPv6 socket.
    pub addr_v4: u32,
}

/// `127.0.0.1` in network byte order, as `dwLocalAddr` carries it on this little-endian machine.
pub const IPV4_LOOPBACK_ADDR: u32 = 0x0100_007F;
/// `0.0.0.0` - the wildcard, which claims the port on every address including the loopback one.
pub const IPV4_ANY_ADDR: u32 = 0;

impl Listener {
    /// The loopback host a client connects to for this listener: `::1` for an IPv6 socket,
    /// `127.0.0.1` otherwise.
    pub fn loopback_host(&self) -> &'static str {
        if self.v6 { "::1" } else { "127.0.0.1" }
    }

    /// Whether this socket blocks a bind to `127.0.0.1` on its port - what a Qt engine told
    /// `QTWEBENGINE_REMOTE_DEBUGGING=127.0.0.1:<port>` attempts. Only an IPv4 socket on that exact
    /// address or on the wildcard does. An IPv6 socket, or an IPv4 socket on another address (a
    /// LAN interface, another `127/8` address), leaves that bind free.
    pub fn blocks_ipv4_loopback_bind(&self) -> bool {
        !self.v6 && (self.addr_v4 == IPV4_LOOPBACK_ADDR || self.addr_v4 == IPV4_ANY_ADDR)
    }
}

/// The address families as ws2def.h numbers them. Literal here on purpose: the winsock feature of
/// the windows crate is refused by the network guard, and two documented constants do not need it.
const AF_INET: u32 = 2;
const AF_INET6: u32 = 23;

/// How many times the buffer may be grown when the table changes size between the sizing call and
/// the filling call. Sockets come and go, so one retry is ordinary - a fourth failure is a machine
/// whose table grows faster than it can be read, and that is reported rather than looped on.
const GROW_ATTEMPTS: usize = 4;

/// The listening sockets that could be read, and what could not be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenerTable {
    pub sockets: Vec<Listener>,
    /// The address family whose table could not be read and the API's word for why, when one of the
    /// two failed - the sockets above are the other family's alone.
    pub unread: Option<String>,
    /// Whether the IPv4 table is among the sockets - the one a web engine opens its debugging port on
    /// (`127.0.0.1`), so a caller looking for engines cannot do without it (CodeRabbit on #110).
    pub ipv4_read: bool,
}

/// Every listening TCP socket on the machine, IPv4 and IPv6, each with its owning pid. The two tables
/// are read apart: one that fails leaves the other's sockets standing, and says which one is missing
/// (R5-P24) - a failed IPv6 read used to take the IPv4 sockets with it. An error only when neither
/// could be read, the API's own words for the caller to report - never a panic, never an empty list
/// dressed as a quiet machine.
pub fn listening_sockets() -> Result<ListenerTable, String> {
    combine(read_v4(), read_v6())
}

/// The two tables as one: both, either with the other's failure named, or neither. Pure, so the
/// three cases are tested without a failing system call.
fn combine(v4: Result<Vec<Listener>, String>, v6: Result<Vec<Listener>, String>) -> Result<ListenerTable, String> {
    match (v4, v6) {
        (Ok(mut sockets), Ok(v6)) => {
            sockets.extend(v6);
            Ok(ListenerTable { sockets, unread: None, ipv4_read: true })
        }
        (Ok(sockets), Err(e)) => Ok(ListenerTable { sockets, unread: Some(format!("IPv6: {e}")), ipv4_read: true }),
        (Err(e), Ok(sockets)) => Ok(ListenerTable { sockets, unread: Some(format!("IPv4: {e}")), ipv4_read: false }),
        (Err(e4), Err(e6)) => Err(format!("IPv4: {e4}, IPv6: {e6}")),
    }
}

fn read_v4() -> Result<Vec<Listener>, String> {
    let (buffer, len) = read_table(AF_INET)?;
    // SAFETY: the buffer holds a MIB_TCPTABLE_OWNER_PID the system just wrote, sized by the
    // system's own count, in storage aligned for the structure (see `Table`). The row pointer
    // arithmetic goes through the table's declared array member so padding is the structure's own,
    // as the documentation asks, and the capacity belt keeps the count inside the bytes written.
    unsafe {
        let table = buffer.as_ptr().cast::<MIB_TCPTABLE_OWNER_PID>();
        let count = (*table).dwNumEntries as usize;
        let first = std::ptr::addr_of!((*table).table).cast::<MIB_TCPROW_OWNER_PID>();
        let offset = (first as usize).saturating_sub(table as usize);
        let rows = std::slice::from_raw_parts(
            first,
            count.min(row_capacity(len, offset, std::mem::size_of::<MIB_TCPROW_OWNER_PID>())),
        );
        Ok(rows
            .iter()
            .map(|row| Listener {
                pid: row.dwOwningPid,
                port: port_from_network_order(row.dwLocalPort),
                // dwLocalAddr is in network byte order, so on this little-endian machine the first
                // octet of the address is the low byte of the DWORD: 127 is the whole 127/8 block.
                loopback: (row.dwLocalAddr & 0xFF) == 127,
                v6: false,
                addr_v4: row.dwLocalAddr,
            })
            .collect())
    }
}

fn read_v6() -> Result<Vec<Listener>, String> {
    let (buffer, len) = read_table(AF_INET6)?;
    // SAFETY: as in read_v4, over the IPv6 table type.
    unsafe {
        let table = buffer.as_ptr().cast::<MIB_TCP6TABLE_OWNER_PID>();
        let count = (*table).dwNumEntries as usize;
        let first = std::ptr::addr_of!((*table).table).cast::<MIB_TCP6ROW_OWNER_PID>();
        let offset = (first as usize).saturating_sub(table as usize);
        let rows = std::slice::from_raw_parts(
            first,
            count.min(row_capacity(len, offset, std::mem::size_of::<MIB_TCP6ROW_OWNER_PID>())),
        );
        Ok(rows
            .iter()
            .map(|row| Listener {
                pid: row.dwOwningPid,
                port: port_from_network_order(row.dwLocalPort),
                loopback: row.ucLocalAddr == IPV6_LOOPBACK,
                v6: true,
                addr_v4: 0,
            })
            .collect())
    }
}

/// `::1`, as sixteen bytes in network order.
const IPV6_LOOPBACK: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];

/// How many rows of `row_size` bytes fit in a buffer of `len` bytes past the first `offset` bytes
/// of header. A belt beside the system's own count: a count that claimed more rows than the buffer
/// holds would otherwise read past it.
fn row_capacity(len: usize, offset: usize, row_size: usize) -> usize {
    len.saturating_sub(offset) / row_size.max(1)
}

/// The port as the table stores it - network byte order in the low sixteen bits - back to a number.
fn port_from_network_order(raw: u32) -> u16 {
    u16::from_be((raw & 0xFFFF) as u16)
}

/// Storage for a table the system fills: `u64` cells, so the buffer's alignment is at least the
/// eight bytes either table structure could ask for - a `Vec<u8>` promises an alignment of one,
/// and casting it to a structure with wider fields is undefined behaviour however the allocator
/// happens to behave. The byte length the system wrote travels beside it.
type Table = Vec<u64>;

/// The raw table for one address family and the number of bytes the system wrote into it: size it,
/// then fill it, growing the buffer while the table keeps outgrowing the size it reported a moment
/// earlier.
fn read_table(family: u32) -> Result<(Table, usize), String> {
    let mut size: u32 = 0;
    // SAFETY: a null table with size zero is the documented way to ask for the size.
    let sizing = unsafe { GetExtendedTcpTable(None, &mut size, false, family, TCP_TABLE_OWNER_PID_LISTENER, 0) };
    if WIN32_ERROR(sizing) != ERROR_INSUFFICIENT_BUFFER && WIN32_ERROR(sizing) != ERROR_SUCCESS {
        return Err(format!("GetExtendedTcpTable sizing failed with {sizing}"));
    }
    for _ in 0..GROW_ATTEMPTS {
        let cells = (size as usize).max(4).div_ceil(std::mem::size_of::<u64>());
        let mut buffer: Table = vec![0u64; cells];
        let mut capacity = u32::try_from(buffer.len() * std::mem::size_of::<u64>())
            .map_err(|_| "GetExtendedTcpTable asked for a table larger than the API can address".to_string())?;
        // SAFETY: the buffer holds `capacity` bytes, and the call writes at most that many, updating
        // the size when it needs more.
        let filled = unsafe {
            GetExtendedTcpTable(
                Some(buffer.as_mut_ptr().cast::<c_void>()),
                &mut capacity,
                false,
                family,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        match WIN32_ERROR(filled) {
            ERROR_SUCCESS => return Ok((buffer, capacity as usize)),
            ERROR_INSUFFICIENT_BUFFER => size = capacity,
            other => return Err(format!("GetExtendedTcpTable failed with {}", other.0)),
        }
    }
    Err("GetExtendedTcpTable kept outgrowing its buffer".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_port_in_network_order_reads_back_as_the_number_that_was_bound() {
        // 9222 = 0x2406 - the table holds it as 0x0624 in the low sixteen bits.
        assert_eq!(port_from_network_order(0x0624), 9222);
        assert_eq!(port_from_network_order(0xFFFF_0624), 9222);
        assert_eq!(port_from_network_order(0), 0);
    }

    #[test]
    fn the_loopback_host_follows_the_address_family() {
        let v4 = Listener { pid: 1, port: 1, loopback: true, v6: false, addr_v4: IPV4_LOOPBACK_ADDR };
        let v6 = Listener { pid: 1, port: 1, loopback: true, v6: true, addr_v4: 0 };
        assert_eq!(v4.loopback_host(), "127.0.0.1");
        assert_eq!(v6.loopback_host(), "::1");
    }

    /// R5-P24: one table that cannot be read leaves the other's sockets standing and names itself,
    /// and only both failing is an error.
    #[test]
    fn a_table_that_cannot_be_read_does_not_take_the_other_with_it() {
        let v4 = Listener { pid: 1, port: 9222, loopback: true, v6: false, addr_v4: IPV4_LOOPBACK_ADDR };
        let v6 = Listener { pid: 2, port: 9333, loopback: true, v6: true, addr_v4: 0 };
        let both = combine(Ok(vec![v4]), Ok(vec![v6])).unwrap();
        assert_eq!((both.sockets, both.unread, both.ipv4_read), (vec![v4, v6], None, true));
        let no_v6 = combine(Ok(vec![v4]), Err("failed with 8".into())).unwrap();
        assert_eq!(no_v6.sockets, vec![v4], "the IPv4 sockets stay");
        assert_eq!(no_v6.unread.as_deref(), Some("IPv6: failed with 8"));
        assert!(no_v6.ipv4_read);
        let no_v4 = combine(Err("failed with 8".into()), Ok(vec![v6])).unwrap();
        assert_eq!(no_v4.sockets, vec![v6]);
        assert_eq!(no_v4.unread.as_deref(), Some("IPv4: failed with 8"));
        assert!(!no_v4.ipv4_read, "said, for a caller that needs that table");
        assert_eq!(combine(Err("a".into()), Err("b".into())), Err("IPv4: a, IPv6: b".to_string()));
    }

    #[test]
    fn the_capacity_belt_never_lets_a_count_read_past_the_buffer() {
        let row = std::mem::size_of::<MIB_TCPROW_OWNER_PID>();
        assert_eq!(row_capacity(4 + row * 3, 4, row), 3);
        assert_eq!(row_capacity(4 + row * 3 + row - 1, 4, row), 3);
        assert_eq!(row_capacity(2, 4, row), 0);
        assert_eq!(row_capacity(100, 4, 0), 96);
    }
}
