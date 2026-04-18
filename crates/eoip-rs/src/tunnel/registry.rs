//! Lock-free tunnel registry backed by `DashMap`.
//!
//! Provides O(1) lookup by `DemuxKey` for the RX demux hot path.

use std::sync::Arc;

use dashmap::DashMap;

use eoip_proto::DemuxKey;

use crate::tunnel::handle::TunnelHandle;

/// Thread-safe tunnel registry for O(1) packet demultiplexing.
#[derive(Debug)]
pub struct TunnelRegistry {
    map: DashMap<DemuxKey, Arc<TunnelHandle>>,
}

impl TunnelRegistry {
    pub fn new() -> Self {
        Self {
            map: DashMap::new(),
        }
    }

    /// Insert a tunnel handle. Returns the previous handle if the key existed.
    pub fn insert(&self, key: DemuxKey, handle: Arc<TunnelHandle>) -> Option<Arc<TunnelHandle>> {
        tracing::debug!(?key, "registry: inserting tunnel");
        self.map.insert(key, handle)
    }

    /// Remove a tunnel by key. Returns the handle if it existed.
    pub fn remove(&self, key: &DemuxKey) -> Option<(DemuxKey, Arc<TunnelHandle>)> {
        tracing::debug!(?key, "registry: removing tunnel");
        self.map.remove(key)
    }

    /// Look up a tunnel by demux key (clones Arc — use `get_ref` on hot paths).
    pub fn get(&self, key: &DemuxKey) -> Option<Arc<TunnelHandle>> {
        self.map.get(key).map(|entry| Arc::clone(entry.value()))
    }

    /// Look up a tunnel by demux key, returning a borrow guard (no Arc clone).
    /// Holds a DashMap shard read-lock for the guard's lifetime — keep it short.
    #[inline]
    pub fn get_ref(&self, key: &DemuxKey) -> Option<dashmap::mapref::one::Ref<'_, DemuxKey, Arc<TunnelHandle>>> {
        self.map.get(key)
    }

    /// Number of registered tunnels.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Find all tunnels with a given tunnel ID (across different peers).
    pub fn find_by_tunnel_id(&self, tid: u16) -> Vec<(DemuxKey, Arc<TunnelHandle>)> {
        self.map
            .iter()
            .filter(|entry| entry.key().tunnel_id == tid)
            .map(|entry| (*entry.key(), Arc::clone(entry.value())))
            .collect()
    }

    /// Iterate over all registered tunnels.
    pub fn iter(&self) -> impl Iterator<Item = (DemuxKey, Arc<TunnelHandle>)> + '_ {
        self.map
            .iter()
            .map(|entry| (*entry.key(), Arc::clone(entry.value())))
    }
}

impl Default for TunnelRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn make_key(tid: u16, last_octet: u8) -> DemuxKey {
        DemuxKey {
            tunnel_id: tid,
            peer_addr: IpAddr::V4(Ipv4Addr::new(10, 0, 0, last_octet)),
        }
    }

    fn make_handle(tid: u16) -> Arc<TunnelHandle> {
        Arc::new(TunnelHandle::new(crate::config::TunnelConfig {
            tunnel_id: tid,
            local: "10.0.0.1".parse().unwrap(),
            remote: "10.0.0.2".parse().unwrap(),
            iface_name: None,
            mtu: 1500,
            enabled: true,
            keepalive_interval_secs: 10,
            keepalive_timeout_secs: 30,
        }))
    }

    #[test]
    fn insert_and_get() {
        let reg = TunnelRegistry::new();
        let key = make_key(100, 1);
        reg.insert(key, make_handle(100));
        assert!(reg.get(&key).is_some());
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn insert_duplicate_returns_old() {
        let reg = TunnelRegistry::new();
        let key = make_key(100, 1);
        assert!(reg.insert(key, make_handle(100)).is_none());
        assert!(reg.insert(key, make_handle(100)).is_some());
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn remove() {
        let reg = TunnelRegistry::new();
        let key = make_key(100, 1);
        reg.insert(key, make_handle(100));
        assert!(reg.remove(&key).is_some());
        assert!(reg.get(&key).is_none());
        assert!(reg.is_empty());
    }

    #[test]
    fn find_by_tunnel_id() {
        let reg = TunnelRegistry::new();
        // Same tunnel ID, different peers
        reg.insert(make_key(42, 1), make_handle(42));
        reg.insert(make_key(42, 2), make_handle(42));
        reg.insert(make_key(99, 3), make_handle(99));

        let results = reg.find_by_tunnel_id(42);
        assert_eq!(results.len(), 2);

        let results = reg.find_by_tunnel_id(99);
        assert_eq!(results.len(), 1);

        let results = reg.find_by_tunnel_id(0);
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn iter_all() {
        let reg = TunnelRegistry::new();
        for i in 0..100u16 {
            reg.insert(make_key(i, (i % 255) as u8 + 1), make_handle(i));
        }
        assert_eq!(reg.iter().count(), 100);
    }

    #[test]
    fn concurrent_access() {
        use std::sync::Arc;
        let reg = Arc::new(TunnelRegistry::new());
        let mut handles = vec![];

        // Spawn writer threads
        for i in 0..100u16 {
            let reg = Arc::clone(&reg);
            handles.push(std::thread::spawn(move || {
                reg.insert(make_key(i, (i % 254) as u8 + 1), make_handle(i));
            }));
        }

        // Spawn reader threads
        for i in 0..100u16 {
            let reg = Arc::clone(&reg);
            handles.push(std::thread::spawn(move || {
                // May or may not find it depending on timing
                let _ = reg.get(&make_key(i, (i % 254) as u8 + 1));
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(reg.len(), 100);
    }
}
