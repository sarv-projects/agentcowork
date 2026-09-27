//! P45.7 — session-scoped provider DNS cache.
//!
//! The first lookup of a hostname in a session is remembered. Later calls in
//! that session reuse the addresses. Ending the session drops them so a stale
//! address is not reused afterwards.

use std::collections::BTreeMap;

/// Addresses resolved for one session. The cache does not perform I/O; the
/// caller supplies the lookup.
#[derive(Debug, Default)]
pub struct SessionDnsCache {
    session_id: String,
    hosts: BTreeMap<String, Vec<String>>,
}

impl SessionDnsCache {
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            hosts: BTreeMap::new(),
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Return the cached addresses, or call `lookup` once and store the result.
    pub fn resolve(&mut self, host: &str, lookup: impl FnOnce(&str) -> Vec<String>) -> Vec<String> {
        if let Some(cached) = self.hosts.get(host) {
            return cached.clone();
        }
        let resolved = lookup(host);
        self.hosts.insert(host.to_string(), resolved.clone());
        resolved
    }

    /// Drop every address. The next resolve must look the host up again.
    pub fn end_session(&mut self) {
        self.hosts.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn one_lookup_per_host_until_the_session_ends() {
        let mut cache = SessionDnsCache::new("s1");
        let calls = AtomicUsize::new(0);
        let lookup = |host: &str| {
            calls.fetch_add(1, Ordering::SeqCst);
            vec![format!("{host}-a")]
        };
        assert_eq!(cache.resolve("api.example", lookup), vec!["api.example-a"]);
        assert_eq!(cache.resolve("api.example", lookup), vec!["api.example-a"]);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        cache.end_session();
        assert_eq!(cache.resolve("api.example", lookup), vec!["api.example-a"]);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
