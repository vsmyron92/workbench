//! A decoder for the SWO stream of a Cortex-M's ITM (ARM DDI 0403, appendix D4): the bytes
//! of one stimulus port, which firmware uses for `printf`. A debug probe's SWO output is raw
//! packets; everything but instrumentation (SWIT) packets is read past: timestamps, overflow
//! and synchronisation, and the hardware sources (DWT) — whose content is not text.

/// Pulls the bytes of one stimulus port out of an SWO stream. Packets may be cut anywhere
/// between `feed` calls.
#[derive(Debug, Clone)]
pub struct Itm {
    port: u8,
    /// What the packet being read is still owed.
    pending: Pending,
    /// Zero bytes of a synchronisation packet seen.
    in_sync: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    /// The next byte is a header.
    Header,
    /// This many more payload bytes of a packet of another port or kind.
    Skip(u8),
    /// A packet whose payload ends at a byte without the continuation bit (timestamps).
    Continued,
    /// Payload bytes of an instrumentation packet on our port.
    Take(u8),
}

impl Itm {
    pub fn new(port: u8) -> Self {
        Self { port: port.min(31), pending: Pending::Header, in_sync: false }
    }

    /// Feed bytes of the stream; the port's payload bytes are appended to `out`.
    pub fn feed(&mut self, data: &[u8], out: &mut Vec<u8>) {
        for &b in data {
            match self.pending {
                Pending::Take(n) => {
                    out.push(b);
                    self.pending = if n > 1 { Pending::Take(n - 1) } else { Pending::Header };
                }
                Pending::Skip(n) => self.pending = if n > 1 { Pending::Skip(n - 1) } else { Pending::Header },
                Pending::Continued => {
                    if b & 0x80 == 0 {
                        self.pending = Pending::Header;
                    }
                }
                Pending::Header => self.header(b),
            }
        }
    }

    fn header(&mut self, h: u8) {
        // A synchronisation packet is zeros and then a one bit: 0x00 …, 0x80.
        if h == 0x00 {
            self.in_sync = true;
            return;
        }
        if self.in_sync {
            self.in_sync = false;
            if h == 0x80 {
                return;
            }
        }
        let size = match h & 0x03 {
            0 => 0,
            1 => 1,
            2 => 2,
            _ => 4,
        };
        self.pending = if size > 0 {
            // `AAAAA 0 SS`: instrumentation (software); `AAAAA 1 SS`: hardware source.
            if h & 0x04 == 0 && h >> 3 == self.port { Pending::Take(size) } else { Pending::Skip(size) }
        } else if h == 0x70 {
            Pending::Header // overflow
        } else if h == 0x94 || h == 0xB4 {
            Pending::Continued // global timestamp
        } else if h & 0x0F == 0x00 && h & 0x70 != 0x00 && h & 0x70 != 0x70 {
            // Local timestamp: `1 CC 0000` is followed by payload bytes, `0 TTT 0000` is alone.
            if h & 0x80 != 0 { Pending::Continued } else { Pending::Header }
        } else if h & 0x0F == 0x04 || h & 0x0B == 0x08 {
            // An extension packet continues while its top bit is set.
            if h & 0x80 != 0 { Pending::Continued } else { Pending::Header }
        } else {
            Pending::Header // reserved: one byte
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(port: u8, chunks: &[&[u8]]) -> Vec<u8> {
        let mut itm = Itm::new(port);
        let mut out = vec![];
        for c in chunks {
            itm.feed(c, &mut out);
        }
        out
    }

    /// An instrumentation packet: `AAAAA 0 SS` then the payload.
    fn swit(port: u8, payload: &[u8]) -> Vec<u8> {
        let ss = match payload.len() {
            1 => 1,
            2 => 2,
            4 => 3,
            n => panic!("a packet carries 1, 2 or 4 bytes, not {n}"),
        };
        let mut v = vec![(port << 3) | ss];
        v.extend_from_slice(payload);
        v
    }

    #[test]
    fn stimulus_port_zero_is_text_and_other_ports_are_not() {
        let mut s = vec![];
        s.extend(swit(0, b"H"));
        s.extend(swit(1, b"x"));
        s.extend(swit(0, b"i!"));
        s.extend(swit(7, b"zzzz"));
        s.extend(swit(0, b"\n..."));
        assert_eq!(decode(0, &[&s]), b"Hi!\n...");
        assert_eq!(decode(1, &[&s]), b"x");
        assert_eq!(decode(7, &[&s]), b"zzzz");
        assert_eq!(decode(31, &[&s]), b"");
    }

    #[test]
    fn everything_that_is_not_an_instrumentation_packet_is_read_past() {
        let mut s = vec![];
        // Synchronisation: zeros and a 0x80.
        s.extend([0, 0, 0, 0, 0, 0x80]);
        s.extend(swit(0, b"a"));
        s.push(0x70); // overflow
        s.push(0x30); // local timestamp, one byte
        s.extend(swit(0, b"b"));
        s.extend([0xC0, 0x81, 0x82, 0x03]); // local timestamp with payload (continuation bits)
        s.extend(swit(0, b"c"));
        s.extend([0x94, 0xFF, 0x7F]); // global timestamp
        s.extend(swit(0, b"d"));
        s.extend([0x0D, 0x40]); // hardware source packet (DWT) on source 1, one payload byte
        s.extend(swit(0, b"e"));
        // A hardware source whose payload looks like text must not be taken for it.
        s.extend([(1 << 3) | 0x04 | 0x01, b'X']);
        s.extend(swit(0, b"f"));
        assert_eq!(decode(0, &[&s]), b"abcdef");
    }

    #[test]
    fn packets_may_be_cut_anywhere() {
        let mut s = vec![];
        s.extend(swit(0, b"abcd"));
        s.extend([0xC0, 0x81, 0x02]);
        s.extend(swit(0, b"ef"));
        s.extend(swit(0, b"g"));
        let whole = decode(0, &[&s]);
        assert_eq!(whole, b"abcdefg");
        for cut in 1..s.len() {
            assert_eq!(decode(0, &[&s[..cut], &s[cut..]]), whole, "cut at {cut}");
        }
        let singly: Vec<&[u8]> = s.chunks(1).collect();
        assert_eq!(decode(0, &singly), whole);
    }
}
