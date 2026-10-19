use std::net::Ipv6Addr;

pub(crate) struct Ipv6Route {
    pub prefix: Ipv6Addr,
    pub length: u8,
}

// Windows can report this complete discovery-default list on interfaces with
// no configured IPv6 DNS. Exclude it only with known automatic settings and no
// route anywhere on the machine that could carry a query to any of the three.
pub(crate) fn inactive_discovery_defaults(
    servers: &[Ipv6Addr],
    automatic: bool,
    routes: Option<&[Ipv6Route]>,
) -> bool {
    let defaults = [1, 2, 3].map(|last| Ipv6Addr::new(0xfec0, 0, 0, 0xffff, 0, 0, 0, last));
    if !automatic || servers.len() != 3 || !defaults.iter().all(|s| servers.contains(s)) {
        return false;
    }
    routes.is_some_and(|routes| {
        !routes.iter().any(|route| {
            if route.length > 128 || route.length == 0 {
                return true;
            }
            let mask = u128::MAX << (128 - route.length);
            defaults
                .iter()
                .any(|s| u128::from(*s) & mask == u128::from(route.prefix) & mask)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn defaults() -> Vec<Ipv6Addr> {
        ["fec0:0:0:ffff::1", "fec0:0:0:ffff::2", "fec0:0:0:ffff::3"]
            .map(|s| s.parse().unwrap())
            .to_vec()
    }
    fn route(prefix: &str, length: u8) -> Ipv6Route {
        Ipv6Route {
            prefix: prefix.parse().unwrap(),
            length,
        }
    }
    #[test]
    fn automatic_defaults_without_a_matching_route_are_inactive() {
        let routes = [route("fe80::", 64), route("ff00::", 8), route("::1", 128)];
        assert!(inactive_discovery_defaults(
            &defaults(),
            true,
            Some(&routes)
        ));
        assert!(inactive_discovery_defaults(&defaults(), true, Some(&[])));
    }
    #[test]
    fn configured_unknown_or_mixed_dns_remains_active() {
        assert!(!inactive_discovery_defaults(&defaults(), false, Some(&[])));
        assert!(!inactive_discovery_defaults(&defaults(), true, None));
        let mut mixed = defaults();
        mixed.push("fd00::53".parse().unwrap());
        assert!(!inactive_discovery_defaults(&mixed, true, Some(&[])));
        assert!(!inactive_discovery_defaults(
            &defaults()[..2],
            true,
            Some(&[])
        ));
        assert!(!inactive_discovery_defaults(
            &[defaults()[0]; 3],
            true,
            Some(&[])
        ));
    }
    #[test]
    fn default_specific_and_invalid_routes_preserve_the_scope() {
        for routes in [
            vec![route("::", 0)],
            vec![route("fec0::", 10)],
            vec![route("fec0:0:0:ffff::2", 128)],
            vec![route("::", 129)],
        ] {
            assert!(!inactive_discovery_defaults(
                &defaults(),
                true,
                Some(&routes)
            ));
        }
    }
}
