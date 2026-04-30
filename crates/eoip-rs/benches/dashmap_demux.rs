use criterion::{black_box, criterion_group, criterion_main, Criterion};
use eoip_rs::tunnel::registry::TunnelRegistry;
use eoip_rs::tunnel::handle::TunnelHandle;
use eoip_proto::DemuxKey;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

fn make_config(tid: u16) -> eoip_rs::config::TunnelConfig {
    eoip_rs::config::TunnelConfig {
        tunnel_id: tid,
        local: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
        remote: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
        iface_name: None,
        mtu: eoip_rs::config::MtuConfig::Fixed(1500),
        enabled: true,
        keepalive_interval_secs: 10,
        keepalive_timeout_secs: 30,
        clamp_tcp_mss: true,
        ipsec_secret: None,
    }
}

fn bench_registry_insert(c: &mut Criterion) {
    c.bench_function("registry_insert", |b| {
        let registry = TunnelRegistry::new();
        let mut counter = 0u16;
        b.iter(|| {
            counter = counter.wrapping_add(1);
            let key = DemuxKey {
                tunnel_id: counter,
                peer_addr: IpAddr::V4(Ipv4Addr::new(10, 0, 0, (counter % 255) as u8 + 1)),
            };
            let handle = TunnelHandle::new(make_config(counter));
            registry.insert(key, Arc::new(handle));
        })
    });
}

fn bench_registry_get_hit(c: &mut Criterion) {
    let registry = TunnelRegistry::new();
    let key = DemuxKey {
        tunnel_id: 100,
        peer_addr: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
    };
    let handle = TunnelHandle::new(make_config(100));
    registry.insert(key, Arc::new(handle));

    c.bench_function("registry_get_hit", |b| {
        b.iter(|| {
            let result = registry.get_ref(black_box(&key));
            black_box(result);
        })
    });
}

fn bench_registry_get_miss(c: &mut Criterion) {
    let registry = TunnelRegistry::new();
    let key = DemuxKey {
        tunnel_id: 999,
        peer_addr: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 99)),
    };

    c.bench_function("registry_get_miss", |b| {
        b.iter(|| {
            let result = registry.get_ref(black_box(&key));
            black_box(result);
        })
    });
}

criterion_group!(
    benches,
    bench_registry_insert,
    bench_registry_get_hit,
    bench_registry_get_miss
);
criterion_main!(benches);
