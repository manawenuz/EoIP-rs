//! Zero-allocation buffer pool for hot-path packet processing.
//!
//! Each `PacketBuf` has 64 bytes of headroom before the payload area,
//! allowing the TX path to prepend EoIP/EtherIP headers without copying.
//! Buffers are returned to the pool on drop (RAII).

use std::sync::Arc;

use crossbeam::queue::ArrayQueue;

/// Headroom reserved before the payload for header prepend.
pub const HEADER_HEADROOM: usize = 64;

/// Maximum Ethernet frame: 1500 MTU + 14 header + 4 FCS + 4 VLAN.
pub const MAX_FRAME_SIZE: usize = 1522;

/// Total buffer allocation.
pub const BUF_TOTAL: usize = HEADER_HEADROOM + MAX_FRAME_SIZE;

/// A packet buffer with headroom for zero-copy header prepend.
///
/// Layout: `[headroom (64B)][payload (up to 1522B)]`
///
/// On TX: TAP read fills `data[head..head+len]`, then `prepend_header(n)`
/// shifts `head` backward by `n` to include the protocol header.
pub struct PacketBuf {
    data: Box<[u8; BUF_TOTAL]>,
    /// Start of valid data within `data`.
    head: usize,
    /// Length of valid data from `head`.
    len: usize,
    /// Pool to return to on drop (None = standalone, not pooled).
    pool: Option<Arc<ArrayQueue<PacketBuf>>>,
}

impl PacketBuf {
    /// Create a new standalone buffer (not from a pool).
    pub fn new() -> Self {
        Self {
            data: Box::new([0u8; BUF_TOTAL]),
            head: HEADER_HEADROOM,
            len: 0,
            pool: None,
        }
    }

    /// Get a mutable slice to the payload area (for reading from TAP/socket).
    pub fn payload_mut(&mut self) -> &mut [u8] {
        &mut self.data[HEADER_HEADROOM..HEADER_HEADROOM + MAX_FRAME_SIZE]
    }

    /// Set the valid payload length after a read operation.
    pub fn set_len(&mut self, len: usize) {
        debug_assert!(len <= MAX_FRAME_SIZE);
        self.head = HEADER_HEADROOM;
        self.len = len;
    }

    /// Prepend a header by shifting the head backward into headroom.
    /// Returns a mutable slice to the header area for writing.
    pub fn prepend_header(&mut self, header_len: usize) -> &mut [u8] {
        debug_assert!(header_len <= self.head, "header exceeds headroom");
        self.head -= header_len;
        self.len += header_len;
        &mut self.data[self.head..self.head + header_len]
    }

    /// The complete valid data slice (header + payload).
    pub fn as_slice(&self) -> &[u8] {
        &self.data[self.head..self.head + self.len]
    }

    /// Length of valid data.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Reset the buffer for reuse.
    pub fn reset(&mut self) {
        self.head = HEADER_HEADROOM;
        self.len = 0;
    }
}

impl Default for PacketBuf {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for PacketBuf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PacketBuf")
            .field("head", &self.head)
            .field("len", &self.len)
            .field("pooled", &self.pool.is_some())
            .finish()
    }
}

/// Lock-free buffer pool backed by `crossbeam::ArrayQueue`.
///
/// Exhausted pool → callers create standalone buffers (graceful degradation,
/// no panic). Those standalone buffers are simply freed on drop rather than
/// returned to the pool.
pub struct BufferPool {
    queue: Arc<ArrayQueue<PacketBuf>>,
    capacity: usize,
}

impl BufferPool {
    /// Create a pool pre-filled with `capacity` buffers.
    pub fn new(capacity: usize) -> Self {
        let queue = Arc::new(ArrayQueue::new(capacity));
        for _ in 0..capacity {
            let _ = queue.push(PacketBuf::new());
        }
        Self { queue, capacity }
    }

    /// Get a buffer from the pool. Falls back to heap allocation if exhausted.
    pub fn get(&self) -> PacketBuf {
        match self.queue.pop() {
            Some(mut buf) => {
                buf.reset();
                buf.pool = Some(Arc::clone(&self.queue));
                buf
            }
            None => {
                // Pool exhausted — allocate standalone (will be freed on drop)
                PacketBuf::new()
            }
        }
    }

    /// Number of available buffers.
    pub fn available(&self) -> usize {
        self.queue.len()
    }

    /// Total pool capacity.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

impl Drop for PacketBuf {
    fn drop(&mut self) {
        if let Some(pool) = self.pool.take() {
            self.reset();
            // Detach from pool before returning to avoid recursive drop
            let mut returned = PacketBuf {
                data: std::mem::replace(&mut self.data, Box::new([0u8; BUF_TOTAL])),
                head: HEADER_HEADROOM,
                len: 0,
                pool: None,
            };
            returned.reset();
            let _ = pool.push(returned); // If pool is full, buffer is freed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_buffer() {
        let mut buf = PacketBuf::new();
        assert_eq!(buf.len(), 0);
        assert!(buf.is_empty());

        let payload = buf.payload_mut();
        payload[..4].copy_from_slice(&[1, 2, 3, 4]);
        buf.set_len(4);
        assert_eq!(buf.len(), 4);
        assert_eq!(buf.as_slice(), &[1, 2, 3, 4]);
    }

    #[test]
    fn prepend_header() {
        let mut buf = PacketBuf::new();
        buf.payload_mut()[..4].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        buf.set_len(4);

        let hdr = buf.prepend_header(8);
        hdr.copy_from_slice(&[0x20, 0x01, 0x64, 0x00, 0x00, 0x04, 0x01, 0x00]);

        assert_eq!(buf.len(), 12);
        assert_eq!(&buf.as_slice()[..8], &[0x20, 0x01, 0x64, 0x00, 0x00, 0x04, 0x01, 0x00]);
        assert_eq!(&buf.as_slice()[8..], &[0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn pool_get_and_return() {
        let pool = BufferPool::new(4);
        assert_eq!(pool.available(), 4);

        {
            let _buf = pool.get();
            assert_eq!(pool.available(), 3);
        }
        // Buffer returned on drop
        assert_eq!(pool.available(), 4);
    }

    #[test]
    fn pool_exhaustion_fallback() {
        let pool = BufferPool::new(2);
        let _a = pool.get();
        let _b = pool.get();
        assert_eq!(pool.available(), 0);

        // Still works — allocates standalone
        let c = pool.get();
        assert!(!c.pool.is_some()); // standalone, no pool reference
    }

    #[test]
    fn pool_no_leak_after_many_cycles() {
        let pool = BufferPool::new(8);
        for _ in 0..1000 {
            let mut buf = pool.get();
            buf.payload_mut()[0] = 0xFF;
            buf.set_len(1);
            // drop returns to pool
        }
        assert_eq!(pool.available(), 8);
    }
}
