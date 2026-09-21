//! Best-effort "who runs this endpoint" labels. Many AI-tool connections go
//! to IPs with no reverse-DNS record, or to cloud hosts whose PTR names the
//! cloud, not the customer. This is a small, hand-kept table, not a WHOIS
//! lookup, so the UI shows these labels as approximate.

use std::net::{IpAddr, Ipv4Addr};

/// Reverse-DNS suffix → owner. Lowercase.
const HOST_SUFFIXES: &[(&str, &str)] = &[
    ("anthropic.com", "Anthropic"),
    ("googleusercontent.com", "Google Cloud"),
    ("1e100.net", "Google"),
    ("google.com", "Google"),
    ("amazonaws.com", "AWS"),
    ("cloudfront.net", "AWS CloudFront"),
    ("cloudflare.com", "Cloudflare"),
    ("github.com", "GitHub"),
    ("githubusercontent.com", "GitHub"),
    ("azure.com", "Microsoft Azure"),
    ("cloudapp.net", "Microsoft Azure"),
    ("msedge.net", "Microsoft"),
    ("akamaitechnologies.com", "Akamai"),
    ("fastly.net", "Fastly"),
];

/// IPv4 prefixes → owner, for IPs with no useful PTR record. Published ranges.
const V4_PREFIXES: &[([u8; 4], u8, &str)] = &[
    ([160, 79, 104, 0], 23, "Anthropic"),
    ([104, 16, 0, 0], 13, "Cloudflare"),
    ([104, 24, 0, 0], 14, "Cloudflare"),
    ([172, 64, 0, 0], 13, "Cloudflare"),
    ([162, 158, 0, 0], 15, "Cloudflare"),
    ([188, 114, 96, 0], 20, "Cloudflare"),
    ([140, 82, 112, 0], 20, "GitHub"),
    ([17, 0, 0, 0], 8, "Apple"),
];

pub fn owner(host: &str, ip: &str) -> Option<&'static str> {
    let host = host.trim_end_matches('.').to_lowercase();
    if let Some((_, o)) = HOST_SUFFIXES.iter().find(|(s, _)| host == *s || host.ends_with(&format!(".{s}"))) {
        return Some(o);
    }
    let IpAddr::V4(v4) = ip.parse().ok()? else { return None };
    V4_PREFIXES.iter().find(|(net, bits, _)| in_prefix(v4, Ipv4Addr::from(*net), *bits)).map(|(_, _, o)| *o)
}

fn in_prefix(ip: Ipv4Addr, net: Ipv4Addr, bits: u8) -> bool {
    let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
    u32::from(ip) & mask == u32::from(net) & mask
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_by_reverse_dns() {
        assert_eq!(owner("17.46.190.35.bc.googleusercontent.com", "35.190.46.17"), Some("Google Cloud"));
        assert_eq!(owner("ec2-18-97-36-77.compute-1.amazonaws.com", "18.97.36.77"), Some("AWS"));
        assert_eq!(owner("notgoogle.com", "10.0.0.1"), None);
    }

    #[test]
    fn labels_by_ip_range_when_there_is_no_ptr() {
        assert_eq!(owner("160.79.104.10", "160.79.104.10"), Some("Anthropic"));
        assert_eq!(owner("", "104.18.24.159"), Some("Cloudflare"));
        assert_eq!(owner("", "104.26.7.153"), Some("Cloudflare"));
        assert_eq!(owner("", "8.8.8.8"), None);
        assert_eq!(owner("", "::1"), None);
    }
}
