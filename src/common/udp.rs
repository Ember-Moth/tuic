use crate::protocol::{Address, Command};
use bytes::Bytes;

#[derive(Clone, Copy, Debug)]
pub enum UdpRelayMode {
    Native,
    Quic,
}

pub fn split_packet(pkt: Bytes, addr: &Address, max_datagram_len: usize) -> SplitPacket {
    SplitPacket::new(pkt, addr, max_datagram_len)
}

#[derive(Debug)]
pub struct SplitPacket {
    pkt: Bytes,
    max_pkt_size: usize,
    start: usize,
    end: usize,
    len: usize,
}

impl SplitPacket {
    #[inline]
    fn new(pkt: Bytes, addr: &Address, max_datagram_size: usize) -> Self {
        const DEFAULT_HEADER: Command = Command::Packet {
            assoc_id: 0,
            pkt_id: 0,
            frag_total: 0,
            frag_id: 0,
            len: 0,
            addr: None,
        };

        let first_pkt_size =
            max_datagram_size - DEFAULT_HEADER.serialized_len() - addr.serialized_len();
        let max_pkt_size = max_datagram_size - DEFAULT_HEADER.serialized_len();
        let len = if first_pkt_size > pkt.len() {
            1 + (pkt.len() - first_pkt_size) / max_pkt_size + 1
        } else {
            1
        };

        Self {
            pkt,
            max_pkt_size,
            start: 0,
            end: first_pkt_size,
            len,
        }
    }
}

impl Iterator for SplitPacket {
    type Item = Bytes;

    fn next(&mut self) -> Option<Self::Item> {
        if self.start <= self.pkt.len() {
            let next = self.pkt.slice(self.start..self.end.min(self.pkt.len()));
            self.start += self.max_pkt_size;
            self.end += self.max_pkt_size;
            Some(next)
        } else {
            None
        }
    }
}

impl ExactSizeIterator for SplitPacket {
    fn len(&self) -> usize {
        self.len
    }
}
