//! Keeps the packaged app's local-network declaration aligned with the
//! Bonjour services registered by its bundled sidecars.
//!
//! macOS refuses a DNS-SD register or browse for any service type the bundle
//! did not declare, with `kDNSServiceErr_NoAuth` (-65555) — the same code a
//! revoked Local Network grant produces, and one the call site cannot tell
//! from a transient responder fault. `_kanna-lan._tcp` shipped undeclared for
//! ten days that way. The earlier version of this test named the services in
//! its own literal list, so a type added to the code never reached it; the
//! list now comes from `kanna_runtime_defaults::bonjour_services`, which is
//! also where the registering crates read their service type.

use kanna_runtime_defaults::bonjour_services::BONJOUR_SERVICES;

#[test]
fn desktop_bundle_declares_every_bonjour_service() {
    let info_plist = include_str!("../Info.plist");

    for service in BONJOUR_SERVICES {
        assert!(
            info_plist.contains(&format!("<string>{}</string>", service.service_type)),
            "Info.plist must declare {} ({}) in NSBonjourServices; macOS refuses an \
             undeclared service type with DNS-SD error -65555",
            service.service_type,
            service.purpose
        );
    }
    assert!(
        info_plist.contains("<key>NSLocalNetworkUsageDescription</key>"),
        "Info.plist must explain Kanna's local-network use"
    );
}
