//! The sections of the profile list and the search filter.

use opusdm_network::profile::ProfileKind;
use opusdm_network::ProfileSummary;

/// A section of the list, in the order shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    Ethernet,
    Wifi,
    Vpn,
    Other,
}

impl Section {
    /// The section heading.
    pub fn label(self) -> &'static str {
        match self {
            Section::Ethernet => "Ethernet",
            Section::Wifi => "Wi-Fi",
            Section::Vpn => "VPN",
            Section::Other => "Other",
        }
    }

    /// The section of a profile kind.
    pub fn of(kind: ProfileKind) -> Self {
        match kind {
            ProfileKind::Ethernet => Section::Ethernet,
            ProfileKind::Wifi => Section::Wifi,
            ProfileKind::WireGuard | ProfileKind::OpenVpn => Section::Vpn,
            ProfileKind::Other => Section::Other,
        }
    }
}

/// The profiles whose name contains `filter` (any case), by section and
/// then by name. Empty sections are left out.
pub fn grouped<'a>(list: &'a [ProfileSummary], filter: &str) -> Vec<(Section, Vec<&'a ProfileSummary>)> {
    let filter = filter.trim().to_lowercase();
    let mut out: Vec<(Section, Vec<&ProfileSummary>)> = Vec::new();
    for section in [Section::Ethernet, Section::Wifi, Section::Vpn, Section::Other] {
        let mut rows: Vec<&ProfileSummary> =
            list.iter().filter(|p| Section::of(p.kind) == section && p.name.to_lowercase().contains(&filter)).collect();
        if rows.is_empty() { continue; }
        rows.sort_by_key(|p| p.name.to_lowercase());
        out.push((section, rows));
    }
    out
}

/// The state shown next to a profile name.
pub fn state_text(p: &ProfileSummary) -> &'static str {
    if p.active { return "Connected"; }
    if p.activating { return "Connecting..."; }
    ""
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(name: &str, kind: ProfileKind) -> ProfileSummary {
        ProfileSummary {
            uuid: name.to_string(),
            name: name.to_string(),
            kind,
            type_name: String::new(),
            active: false,
            activating: false,
            ask_password: false,
        }
    }

    #[test]
    fn profiles_are_grouped_in_order_and_sorted_by_name() {
        let list = vec![
            summary("work", ProfileKind::OpenVpn),
            summary("br0", ProfileKind::Other),
            summary("Home", ProfileKind::Wifi),
            summary("cafe", ProfileKind::Wifi),
            summary("wg", ProfileKind::WireGuard),
        ];
        let g = grouped(&list, "");
        let sections: Vec<Section> = g.iter().map(|(s, _)| *s).collect();
        assert_eq!(sections, vec![Section::Wifi, Section::Vpn, Section::Other]);
        let wifi: Vec<&str> = g[0].1.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(wifi, vec!["cafe", "Home"]);
        assert_eq!(g[1].1.len(), 2);
    }

    #[test]
    fn the_filter_ignores_case_and_drops_empty_sections() {
        let list = vec![summary("Home", ProfileKind::Wifi), summary("work", ProfileKind::OpenVpn)];
        let g = grouped(&list, " HOM ");
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].0, Section::Wifi);
        assert!(grouped(&list, "zzz").is_empty());
    }

    #[test]
    fn the_state_text_follows_the_connection() {
        let mut p = summary("x", ProfileKind::Wifi);
        assert_eq!(state_text(&p), "");
        p.activating = true;
        assert_eq!(state_text(&p), "Connecting...");
        p.active = true;
        assert_eq!(state_text(&p), "Connected");
    }
}
