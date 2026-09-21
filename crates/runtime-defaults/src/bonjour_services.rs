//! Every Bonjour service type Kanna's macOS bundle registers or browses.
//!
//! macOS gates DNS-SD *per service type*, not per app: an app may only
//! register or browse a type it listed in `NSBonjourServices`, and asking for
//! any other one is refused with `kDNSServiceErr_NoAuth` (-65555) even when
//! the user has granted Local Network access. The refusal is indistinguishable
//! from a transient responder failure at the call site, so an undeclared type
//! does not fail loudly — it simply never appears on the network.
//!
//! That is not hypothetical. `_kanna-lan._tcp` was added on 2026-09-10 with no
//! matching `Info.plist` entry, and from then until 2026-09-20 every shipped
//! Mac silently failed to advertise or discover the LAN routing service while
//! `_kanna-mobile._tcp`, declared in the same list, kept publishing normally.
//! The bundle test that was supposed to catch it asserted against its own
//! hard-coded copy of the list, so it never learned about the new type.
//!
//! This table is the one list. `apps/desktop/src-tauri/tests/local_network_bundle_policy.rs`
//! asserts the bundle declares every entry, and the crates that own the
//! registrations take their service type from here rather than restating it,
//! so a type cannot exist in the code and be missing from the bundle.

/// One Bonjour service type, in both spellings Kanna needs.
pub struct BonjourService {
    /// As `NSBonjourServices` declares it, e.g. `_kanna-lan._tcp`.
    pub service_type: &'static str,
    /// As DNS-SD register/browse take it, e.g. `_kanna-lan._tcp.local.`.
    pub registration_type: &'static str,
    /// What Kanna uses it for. Operator-facing: this is the sentence a
    /// diagnostic prints when the type is refused.
    pub purpose: &'static str,
}

/// Mobile pairing and LAN access: the phone browses for this to find a
/// desktop without the relay.
pub const MOBILE_PAIRING: BonjourService = BonjourService {
    service_type: "_kanna-mobile._tcp",
    registration_type: "_kanna-mobile._tcp.local.",
    purpose: "mobile pairing and LAN access",
};

/// Task transfer between desktops on one LAN.
pub const TASK_TRANSFER: BonjourService = BonjourService {
    service_type: "_kanna-xfer._tcp",
    registration_type: "_kanna-xfer._tcp.local.",
    purpose: "task transfer between machines",
};

/// The LAN machine-invoke listener siblings route requests through.
pub const LAN_ROUTING: BonjourService = BonjourService {
    service_type: "_kanna-lan._tcp",
    registration_type: "_kanna-lan._tcp.local.",
    purpose: "LAN routing between machines",
};

/// Every service type the packaged app must declare.
pub const BONJOUR_SERVICES: &[&BonjourService] = &[&MOBILE_PAIRING, &TASK_TRANSFER, &LAN_ROUTING];

/// The entry owning `registration_type`, accepting either spelling.
pub fn service_for(registration_type: &str) -> Option<&'static BonjourService> {
    let wanted = registration_type.trim_end_matches('.');
    BONJOUR_SERVICES.iter().copied().find(|service| {
        service.service_type == wanted || service.registration_type.trim_end_matches('.') == wanted
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registration_type_is_its_service_type_in_the_local_domain() {
        for service in BONJOUR_SERVICES {
            assert_eq!(
                service.registration_type,
                format!("{}.local.", service.service_type),
                "{} declares a registration type that is not its own service type",
                service.service_type
            );
        }
    }

    #[test]
    fn service_names_fit_the_rfc_6763_limit() {
        for service in BONJOUR_SERVICES {
            // RFC 6763 §7.2: the service name between the leading underscore
            // and `._tcp` is at most 15 bytes. mDNSResponder silently fails to
            // resolve a longer one.
            let name = service
                .service_type
                .trim_start_matches('_')
                .trim_end_matches("._tcp");
            assert!(
                (1..=15).contains(&name.len()),
                "{} has a {}-byte service name",
                service.service_type,
                name.len()
            );
        }
    }

    #[test]
    fn a_registration_type_resolves_in_either_spelling() {
        assert_eq!(
            service_for("_kanna-lan._tcp.local.").map(|s| s.service_type),
            Some("_kanna-lan._tcp")
        );
        assert_eq!(
            service_for("_kanna-lan._tcp").map(|s| s.service_type),
            Some("_kanna-lan._tcp")
        );
        assert!(service_for("_example._tcp").is_none());
    }
}
