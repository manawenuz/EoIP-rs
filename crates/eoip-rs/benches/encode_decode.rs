use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use eoip_proto::gre;
use eoip_proto::etherip;

fn bench_gre_encode(c: &mut Criterion) {
    let mut group = c.benchmark_group("gre_encode");
    group.throughput(Throughput::Bytes(8));
    group.bench_function("encode", |b| {
        let mut buf = [0u8; 8];
        b.iter(|| {
            gre::encode_eoip_header(black_box(100), black_box(1500), &mut buf).unwrap();
            black_box(&buf);
        })
    });
    group.finish();
}

fn bench_gre_decode(c: &mut Criterion) {
    let mut group = c.benchmark_group("gre_decode");
    group.throughput(Throughput::Bytes(8));
    let buf = [0x20, 0x01, 0x64, 0x00, 0x05, 0xDC, 0x64, 0x00];
    group.bench_function("decode", |b| {
        b.iter(|| {
            let result = gre::decode_eoip_header(black_box(&buf)).unwrap();
            black_box(result);
        })
    });
    group.finish();
}

fn bench_etherip_encode(c: &mut Criterion) {
    let mut group = c.benchmark_group("etherip_encode");
    group.throughput(Throughput::Bytes(2));
    group.bench_function("encode", |b| {
        let mut buf = [0u8; 2];
        b.iter(|| {
            etherip::encode_eoipv6_header(black_box(100), &mut buf).unwrap();
            black_box(&buf);
        })
    });
    group.finish();
}

fn bench_etherip_decode(c: &mut Criterion) {
    let mut group = c.benchmark_group("etherip_decode");
    group.throughput(Throughput::Bytes(2));
    let buf = [0x03, 0x64]; // version=3, tid=100
    group.bench_function("decode", |b| {
        b.iter(|| {
            let result = etherip::decode_eoipv6_header(black_box(&buf)).unwrap();
            black_box(result);
        })
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_gre_encode,
    bench_gre_decode,
    bench_etherip_encode,
    bench_etherip_decode
);
criterion_main!(benches);
