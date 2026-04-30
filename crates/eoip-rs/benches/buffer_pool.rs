use criterion::{black_box, criterion_group, criterion_main, Criterion};
use eoip_rs::packet::buffer::BufferPool;

fn bench_pool_get_drop(c: &mut Criterion) {
    let pool = BufferPool::new(1024);
    c.bench_function("pool_get_drop", |b| {
        b.iter(|| {
            let buf = pool.get();
            black_box(&buf);
            // drop returns to pool (or frees if standalone)
        })
    });
}

fn bench_pool_get_set_len_drop(c: &mut Criterion) {
    let pool = BufferPool::new(1024);
    c.bench_function("pool_get_set_len_drop", |b| {
        b.iter(|| {
            let mut buf = pool.get();
            buf.set_len(1500);
            black_box(&buf);
        })
    });
}

fn bench_pool_get_prepend_drop(c: &mut Criterion) {
    let pool = BufferPool::new(1024);
    c.bench_function("pool_get_prepend_drop", |b| {
        b.iter(|| {
            let mut buf = pool.get();
            buf.set_len(1500);
            let hdr = buf.prepend_header(8);
            hdr.copy_from_slice(&[0x20, 0x01, 0x64, 0x00, 0x00, 0x00, 0x64, 0x00]);
            black_box(&buf);
        })
    });
}

fn bench_pool_exhaustion_fallback(c: &mut Criterion) {
    c.bench_function("pool_exhaustion_fallback", |b| {
        b.iter(|| {
            // Create a tiny pool so every get allocates fallback
            let pool = BufferPool::new(1);
            let mut handles = Vec::new();
            for _ in 0..8 {
                handles.push(pool.get());
            }
            black_box(handles.len());
            // all 8 buffers drop here — 7 are standalone frees, 1 returns to pool
        })
    });
}

criterion_group!(
    benches,
    bench_pool_get_drop,
    bench_pool_get_set_len_drop,
    bench_pool_get_prepend_drop,
    bench_pool_exhaustion_fallback
);
criterion_main!(benches);
