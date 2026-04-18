//! AF_PACKET + TPACKET_V3 zero-copy RX ring buffer.
//!
//! Eliminates the kernel→userspace `recvmmsg` copy by sharing a ring buffer
//! between kernel and userspace. The kernel writes packets directly into
//! mmap'd memory; we read them in place with only a single copy into the
//! `PacketBuf` for the crossbeam channel.
//!
//! Ring layout (TPACKET_V3):
//!   Ring = N blocks, each block = M frames (variable-size packets).
//!   Kernel fills blocks and marks them `TP_STATUS_USER`.
//!   Userspace processes all packets in a block, then marks `TP_STATUS_KERNEL`.

use std::os::fd::RawFd;
use std::ptr;

// ── Ring buffer tuning ────────────────────────────────────────────────
/// Block size: 256 KB (must be power-of-2, page-aligned).
const BLOCK_SIZE: u32 = 1 << 18; // 262144
/// Number of blocks: 16 blocks × 256 KB = 4 MB total ring.
const BLOCK_NR: u32 = 16;
/// Frame alignment within blocks.
const FRAME_SIZE: u32 = 2048;
/// Block retire timeout in ms — how long kernel waits before delivering
/// a partially-full block. 1 ms for low latency.
const BLOCK_TIMEOUT_MS: u32 = 1;

// ── TPACKET V3 constants ─────────────────────────────────────────────
const TPACKET_V3: libc::c_int = 2;
const PACKET_VERSION: libc::c_int = 10;
const PACKET_RX_RING: libc::c_int = 5;
const SOL_PACKET: libc::c_int = 263;
const TP_STATUS_KERNEL: u32 = 0;
const TP_STATUS_USER: u32 = 1;

// ── TPACKET V3 structures (repr(C) matching kernel headers) ──────────

/// `struct tpacket_req3` — ring buffer configuration for setsockopt.
#[repr(C)]
struct TpacketReq3 {
    tp_block_size: u32,
    tp_block_nr: u32,
    tp_frame_size: u32,
    tp_frame_nr: u32,
    tp_retire_blk_tov: u32,
    tp_sizeof_priv: u32,
    tp_feature_req_u: u32,
}

/// `struct tpacket_block_desc` + embedded `tpacket_hdr_v1`.
///
/// Layout matches the kernel's union `tpacket_bd_header_u { tpacket_hdr_v1 bh1 }`.
#[repr(C)]
struct BlockDesc {
    version: u32,
    offset_to_priv: u32,
    // tpacket_hdr_v1 fields:
    block_status: u32,
    num_pkts: u32,
    offset_to_first_pkt: u32,
    blk_len: u32,
    seq_num: u64, // __aligned_u64
    _ts_first_sec: u32,
    _ts_first_nsec: u32,
    _ts_last_sec: u32,
    _ts_last_nsec: u32,
}

/// `struct tpacket3_hdr` — per-packet header in the ring buffer.
#[repr(C)]
struct Tpacket3Hdr {
    tp_next_offset: u32,
    _tp_sec: u32,
    _tp_nsec: u32,
    tp_snaplen: u32,
    _tp_len: u32,
    _tp_status: u32,
    tp_mac: u16,
    tp_net: u16,
    // tpacket_hdr_variant1:
    _tp_rxhash: u32,
    _tp_vlan_tci: u32,
    _tp_vlan_tpid: u16,
    _hv1_padding: u16,
    _tp_padding: [u8; 8],
}

/// A TPACKET_V3 memory-mapped ring buffer for zero-copy packet receive.
pub struct PacketMmapRing {
    ring: *mut u8,
    ring_size: usize,
    block_size: u32,
    block_nr: u32,
    current_block: u32,
    fd: RawFd,
}

// Safety: the ring pointer is a process-wide mmap region, safe to move across threads.
// The fd is a file descriptor that can be used from any thread.
unsafe impl Send for PacketMmapRing {}

impl PacketMmapRing {
    /// Set up TPACKET_V3 on an AF_PACKET socket fd and mmap the ring buffer.
    pub fn new(fd: RawFd) -> Result<Self, std::io::Error> {
        // 1. Set TPACKET version to V3
        let version: libc::c_int = TPACKET_V3;
        setsockopt(fd, SOL_PACKET, PACKET_VERSION, &version)?;

        // 2. Configure ring buffer geometry
        let frame_nr = (BLOCK_SIZE / FRAME_SIZE) * BLOCK_NR;
        let req = TpacketReq3 {
            tp_block_size: BLOCK_SIZE,
            tp_block_nr: BLOCK_NR,
            tp_frame_size: FRAME_SIZE,
            tp_frame_nr: frame_nr,
            tp_retire_blk_tov: BLOCK_TIMEOUT_MS,
            tp_sizeof_priv: 0,
            tp_feature_req_u: 0,
        };
        setsockopt(fd, SOL_PACKET, PACKET_RX_RING, &req)?;

        // 3. mmap the ring buffer (shared between kernel and userspace)
        let ring_size = (BLOCK_SIZE as usize) * (BLOCK_NR as usize);
        let ring = unsafe {
            libc::mmap(
                ptr::null_mut(),
                ring_size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if ring == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }

        tracing::info!(
            block_size = BLOCK_SIZE,
            block_nr = BLOCK_NR,
            ring_mb = ring_size / (1024 * 1024),
            "PACKET_MMAP ring buffer mapped"
        );

        Ok(Self {
            ring: ring as *mut u8,
            ring_size,
            block_size: BLOCK_SIZE,
            block_nr: BLOCK_NR,
            current_block: 0,
            fd,
        })
    }

    /// Process the next ready block of packets via a callback.
    ///
    /// Polls for up to `timeout_ms` for a block to become ready, then calls
    /// `process_pkt(ip_data, ip_len)` for each packet in the block. The block
    /// is released back to the kernel after all packets are processed.
    ///
    /// Returns the number of packets processed, or 0 if poll timed out.
    pub fn process_block<F>(&mut self, timeout_ms: i32, mut process_pkt: F) -> usize
    where
        F: FnMut(&[u8], usize),
    {
        let block_ptr = self.block_ptr(self.current_block);
        let desc = block_ptr as *const BlockDesc;

        // Check if block is ready (kernel marked it TP_STATUS_USER)
        if (unsafe { ptr::read_volatile(&(*desc).block_status) } & TP_STATUS_USER) == 0 {
            // Not ready — poll for readiness
            let mut pfd = libc::pollfd {
                fd: self.fd,
                events: libc::POLLIN | libc::POLLERR,
                revents: 0,
            };
            let ret = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
            if ret <= 0 {
                return 0;
            }

            // Re-check after poll wakeup
            if (unsafe { ptr::read_volatile(&(*desc).block_status) } & TP_STATUS_USER) == 0 {
                return 0;
            }
        }

        let num_pkts = unsafe { ptr::read_volatile(&(*desc).num_pkts) } as usize;
        let first_offset = unsafe { ptr::read_volatile(&(*desc).offset_to_first_pkt) } as usize;

        // Walk all packets in the block
        let mut offset = first_offset;
        for _ in 0..num_pkts {
            let hdr_ptr = unsafe { block_ptr.add(offset) };
            let hdr = unsafe { &*(hdr_ptr as *const Tpacket3Hdr) };

            // For SOCK_RAW: tp_mac points to L2 header, tp_net points to IP header.
            // tp_snaplen is bytes captured from tp_mac. We want the IP data
            // starting at tp_net with length = snaplen - (tp_net - tp_mac).
            let mac_offset = hdr.tp_mac as usize;
            let net_offset = hdr.tp_net as usize;
            let l2_len = net_offset - mac_offset;
            let data_len = (hdr.tp_snaplen as usize).saturating_sub(l2_len);

            if data_len == 0 {
                if hdr.tp_next_offset != 0 {
                    offset += hdr.tp_next_offset as usize;
                } else {
                    break;
                }
                continue;
            }

            // Safety: data is within the mmap'd block, valid until we release it
            let data = unsafe { std::slice::from_raw_parts(hdr_ptr.add(net_offset), data_len) };

            process_pkt(data, data_len);

            // Advance to next packet
            if hdr.tp_next_offset != 0 {
                offset += hdr.tp_next_offset as usize;
            } else {
                break;
            }
        }

        // Release block back to kernel
        unsafe {
            ptr::write_volatile(&mut (*(block_ptr as *mut BlockDesc)).block_status, TP_STATUS_KERNEL);
        }
        self.current_block = (self.current_block + 1) % self.block_nr;

        num_pkts
    }

    fn block_ptr(&self, idx: u32) -> *mut u8 {
        unsafe { self.ring.add((idx as usize) * (self.block_size as usize)) }
    }
}

impl Drop for PacketMmapRing {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.ring as *mut libc::c_void, self.ring_size);
        }
    }
}

fn setsockopt<T>(fd: RawFd, level: i32, name: i32, val: &T) -> Result<(), std::io::Error> {
    let ret = unsafe {
        libc::setsockopt(
            fd,
            level,
            name,
            val as *const T as *const libc::c_void,
            std::mem::size_of::<T>() as libc::socklen_t,
        )
    };
    if ret < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
