use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use eoip_rs::packet::buffer::BufferPool;
use eoip_rs::packet::tx::TxPacket;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

// Dummy mmsghdr for cross-platform benchmarking (macOS lacks libc::mmsghdr)
#[cfg(not(target_os = "linux"))]
#[repr(C)]
struct mmsghdr {
    msg_hdr: libc::msghdr,
    msg_len: libc::c_uint,
}
#[cfg(target_os = "linux")]
type mmsghdr = libc::mmsghdr;

/// Simulates the current flush_batch + send_batch Vec allocation pattern
/// without making any syscalls. This isolates the heap-allocation overhead.
fn build_sendmmsg_buffers_current(pkts: &[&TxPacket]) {
    let len = pkts.len();
    let mut _iovecs: Vec<libc::iovec> = Vec::with_capacity(len);
    let mut _addrs_v4: Vec<libc::sockaddr_in> = Vec::new();
    let mut _addrs_v6: Vec<libc::sockaddr_in6> = Vec::new();
    let mut _is_v6: Vec<bool> = Vec::with_capacity(len);

    for pkt in pkts {
        let data = pkt.buf.as_slice();
        _iovecs.push(libc::iovec {
            iov_base: data.as_ptr() as *mut libc::c_void,
            iov_len: data.len(),
        });
        match pkt.dest {
            SocketAddr::V4(v4) => {
                _is_v6.push(false);
                let mut addr: libc::sockaddr_in = unsafe { std::mem::zeroed() };
                addr.sin_family = libc::AF_INET as libc::sa_family_t;
                addr.sin_addr.s_addr = u32::from(*v4.ip()).to_be();
                _addrs_v4.push(addr);
            }
            SocketAddr::V6(v6) => {
                _is_v6.push(true);
                let mut addr: libc::sockaddr_in6 = unsafe { std::mem::zeroed() };
                addr.sin6_family = libc::AF_INET6 as libc::sa_family_t;
                addr.sin6_addr.s6_addr = v6.ip().octets();
                _addrs_v6.push(addr);
            }
        }
    }

    let mut _msgs: Vec<mmsghdr> = Vec::with_capacity(len);
    let mut _v4_idx = 0usize;
    let mut _v6_idx = 0usize;
    for v6 in _is_v6.iter() {
        let mut hdr: mmsghdr = unsafe { std::mem::zeroed() };
        hdr.msg_hdr.msg_iovlen = 1;
        if *v6 {
            hdr.msg_hdr.msg_name = &mut _addrs_v6[_v6_idx] as *mut _ as *mut libc::c_void;
            hdr.msg_hdr.msg_namelen = std::mem::size_of::<libc::sockaddr_in6>() as u32;
            _v6_idx += 1;
        } else {
            hdr.msg_hdr.msg_name = &mut _addrs_v4[_v4_idx] as *mut _ as *mut libc::c_void;
            hdr.msg_hdr.msg_namelen = std::mem::size_of::<libc::sockaddr_in>() as u32;
            _v4_idx += 1;
        }
        _msgs.push(hdr);
    }

    black_box(_msgs.len());
}

/// Same logic but reuses pre-allocated Vecs (the PRD-3 target pattern).
fn build_sendmmsg_buffers_reused(
    pkts: &[&TxPacket],
    iovecs: &mut Vec<libc::iovec>,
    addrs_v4: &mut Vec<libc::sockaddr_in>,
    addrs_v6: &mut Vec<libc::sockaddr_in6>,
    is_v6: &mut Vec<bool>,
    msgs: &mut Vec<mmsghdr>,
) {
    iovecs.clear();
    addrs_v4.clear();
    addrs_v6.clear();
    is_v6.clear();
    msgs.clear();

    for pkt in pkts {
        let data = pkt.buf.as_slice();
        iovecs.push(libc::iovec {
            iov_base: data.as_ptr() as *mut libc::c_void,
            iov_len: data.len(),
        });
        match pkt.dest {
            SocketAddr::V4(v4) => {
                is_v6.push(false);
                let mut addr: libc::sockaddr_in = unsafe { std::mem::zeroed() };
                addr.sin_family = libc::AF_INET as libc::sa_family_t;
                addr.sin_addr.s_addr = u32::from(*v4.ip()).to_be();
                addrs_v4.push(addr);
            }
            SocketAddr::V6(v6) => {
                is_v6.push(true);
                let mut addr: libc::sockaddr_in6 = unsafe { std::mem::zeroed() };
                addr.sin6_family = libc::AF_INET6 as libc::sa_family_t;
                addr.sin6_addr.s6_addr = v6.ip().octets();
                addrs_v6.push(addr);
            }
        }
    }

    let mut v4_idx = 0usize;
    let mut v6_idx = 0usize;
    for v6 in is_v6.iter() {
        let mut hdr: mmsghdr = unsafe { std::mem::zeroed() };
        hdr.msg_hdr.msg_iovlen = 1;
        if *v6 {
            hdr.msg_hdr.msg_name = &mut addrs_v6[v6_idx] as *mut _ as *mut libc::c_void;
            hdr.msg_hdr.msg_namelen = std::mem::size_of::<libc::sockaddr_in6>() as u32;
            v6_idx += 1;
        } else {
            hdr.msg_hdr.msg_name = &mut addrs_v4[v4_idx] as *mut _ as *mut libc::c_void;
            hdr.msg_hdr.msg_namelen = std::mem::size_of::<libc::sockaddr_in>() as u32;
            v4_idx += 1;
        }
        msgs.push(hdr);
    }

    black_box(msgs.len());
}

fn make_test_packets(pool: &BufferPool, count: usize) -> Vec<TxPacket> {
    let dest = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 1), 0));
    (0..count)
        .map(|_| {
            let mut buf = pool.get();
            buf.set_len(1500);
            TxPacket { buf, dest }
        })
        .collect()
}

fn bench_tx_batch_build_current(c: &mut Criterion) {
    let pool = BufferPool::new(256);
    let packets = make_test_packets(&pool, 64);
    let refs: Vec<&TxPacket> = packets.iter().collect();

    let mut group = c.benchmark_group("tx_batch_build");
    group.throughput(Throughput::Elements(64));
    group.bench_function("current_alloc_per_flush", |b| {
        b.iter(|| {
            build_sendmmsg_buffers_current(black_box(&refs));
        })
    });
    group.finish();
}

fn bench_tx_batch_build_reused(c: &mut Criterion) {
    let pool = BufferPool::new(256);
    let packets = make_test_packets(&pool, 64);
    let refs: Vec<&TxPacket> = packets.iter().collect();

    let mut iovecs: Vec<libc::iovec> = Vec::with_capacity(64);
    let mut addrs_v4: Vec<libc::sockaddr_in> = Vec::with_capacity(64);
    let mut addrs_v6: Vec<libc::sockaddr_in6> = Vec::with_capacity(64);
    let mut is_v6: Vec<bool> = Vec::with_capacity(64);
    let mut msgs: Vec<mmsghdr> = Vec::with_capacity(64);

    let mut group = c.benchmark_group("tx_batch_build");
    group.throughput(Throughput::Elements(64));
    group.bench_function("reused_vecs", |b| {
        b.iter(|| {
            build_sendmmsg_buffers_reused(
                black_box(&refs),
                &mut iovecs,
                &mut addrs_v4,
                &mut addrs_v6,
                &mut is_v6,
                &mut msgs,
            );
        })
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_tx_batch_build_current,
    bench_tx_batch_build_reused
);
criterion_main!(benches);
