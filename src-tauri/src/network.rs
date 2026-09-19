use serde::Serialize;
use std::net::Ipv4Addr;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NetworkProfile {
    Private,
    DomainAuthenticated,
    Public,
    Unknown,
}

impl NetworkProfile {
    pub fn lan_allowed(&self) -> bool {
        matches!(self, Self::Private | Self::DomainAuthenticated)
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct LanCandidate {
    pub adapter_id: String,
    pub adapter_name: String,
    pub address: Ipv4Addr,
    pub network_profile: NetworkProfile,
    pub recommended: bool,
}

#[derive(Clone, Debug)]
struct CandidateRank {
    candidate: LanCandidate,
    has_gateway: bool,
    ipv4_metric: u32,
}

pub fn inspect_lan_candidates() -> Result<Vec<LanCandidate>, String> {
    inspect_lan_candidates_impl()
}

pub fn select_lan_candidate(adapter_id: Option<&str>) -> Result<LanCandidate, String> {
    let candidates = inspect_lan_candidates()?;
    if let Some(adapter_id) = adapter_id {
        return candidates
            .into_iter()
            .find(|candidate| candidate.adapter_id == adapter_id)
            .ok_or_else(|| {
                "The selected LAN adapter is no longer available on an allowed private network."
                    .to_string()
            });
    }
    candidates
        .iter()
        .find(|candidate| candidate.recommended)
        .cloned()
        .or_else(|| candidates.first().cloned())
        .ok_or_else(|| {
            "No active private IPv4 network adapter is available for LAN access.".to_string()
        })
}

fn eligible_ipv4(address: Ipv4Addr) -> bool {
    !address.is_loopback()
        && !address.is_unspecified()
        && !address.is_multicast()
        && !(address.octets()[0] == 169 && address.octets()[1] == 254)
}

#[cfg(windows)]
fn inspect_lan_candidates_impl() -> Result<Vec<LanCandidate>, String> {
    use std::collections::HashMap;
    use windows::core::GUID;
    use windows::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_SUCCESS};
    use windows::Win32::NetworkManagement::IpHelper::{
        ConvertInterfaceLuidToGuid, GetAdaptersAddresses, GAA_FLAG_INCLUDE_GATEWAYS,
        IP_ADAPTER_ADDRESSES_LH,
    };
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows::Win32::Networking::NetworkListManager::{
        INetworkListManager, NetworkListManager, NLM_NETWORK_CATEGORY_DOMAIN_AUTHENTICATED,
        NLM_NETWORK_CATEGORY_PRIVATE, NLM_NETWORK_CATEGORY_PUBLIC,
    };
    use windows::Win32::Networking::WinSock::{AF_INET, SOCKADDR, SOCKADDR_IN};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };

    struct ComApartment(bool);
    impl Drop for ComApartment {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }

    fn profiles() -> HashMap<u128, NetworkProfile> {
        let initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        let _apartment = ComApartment(initialized);
        let manager: INetworkListManager = match unsafe {
            CoCreateInstance(&NetworkListManager, None, CLSCTX_INPROC_SERVER)
        } {
            Ok(manager) => manager,
            Err(_) => return HashMap::new(),
        };
        let connections = match unsafe { manager.GetNetworkConnections() } {
            Ok(connections) => connections,
            Err(_) => return HashMap::new(),
        };
        let mut result = HashMap::new();
        loop {
            let mut slots = [None];
            let mut fetched = 0_u32;
            if unsafe { connections.Next(&mut slots, Some(&mut fetched)) }.is_err() || fetched == 0
            {
                break;
            }
            let Some(connection) = slots[0].take() else {
                continue;
            };
            let Ok(adapter_id) = (unsafe { connection.GetAdapterId() }) else {
                continue;
            };
            let Ok(network) = (unsafe { connection.GetNetwork() }) else {
                continue;
            };
            let profile = match unsafe { network.GetCategory() } {
                Ok(category) if category == NLM_NETWORK_CATEGORY_PRIVATE => NetworkProfile::Private,
                Ok(category) if category == NLM_NETWORK_CATEGORY_DOMAIN_AUTHENTICATED => {
                    NetworkProfile::DomainAuthenticated
                }
                Ok(category) if category == NLM_NETWORK_CATEGORY_PUBLIC => NetworkProfile::Public,
                _ => NetworkProfile::Unknown,
            };
            result.insert(adapter_id.to_u128(), profile);
        }
        result
    }

    unsafe fn ipv4_from_sockaddr(sockaddr: *mut SOCKADDR) -> Option<Ipv4Addr> {
        if sockaddr.is_null() || (*sockaddr).sa_family != AF_INET {
            return None;
        }
        let address = &*(sockaddr as *const SOCKADDR_IN);
        let bytes = address.sin_addr.S_un.S_un_b;
        Some(Ipv4Addr::new(bytes.s_b1, bytes.s_b2, bytes.s_b3, bytes.s_b4))
    }

    let profile_by_adapter = profiles();
    let mut buffer_size = 0_u32;
    let first = unsafe {
        GetAdaptersAddresses(
            AF_INET.0 as u32,
            GAA_FLAG_INCLUDE_GATEWAYS,
            None,
            None,
            &mut buffer_size,
        )
    };
    if first != ERROR_BUFFER_OVERFLOW.0 {
        return Err(format!(
            "Windows network inspection failed while sizing the adapter buffer (error {first})."
        ));
    }
    let mut buffer = vec![0_u8; buffer_size as usize];
    let head = buffer.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH;
    let status = unsafe {
        GetAdaptersAddresses(
            AF_INET.0 as u32,
            GAA_FLAG_INCLUDE_GATEWAYS,
            None,
            Some(head),
            &mut buffer_size,
        )
    };
    if status != ERROR_SUCCESS.0 {
        return Err(format!(
            "Windows network inspection failed while reading adapters (error {status})."
        ));
    }

    let mut ranked = Vec::<CandidateRank>::new();
    let mut current = head;
    while !current.is_null() {
        let adapter = unsafe { &*current };
        current = adapter.Next;
        if adapter.OperStatus != IfOperStatusUp {
            continue;
        }
        let mut guid = GUID::zeroed();
        if unsafe { ConvertInterfaceLuidToGuid(&adapter.Luid, &mut guid) } != ERROR_SUCCESS {
            continue;
        }
        let profile = profile_by_adapter
            .get(&guid.to_u128())
            .cloned()
            .unwrap_or(NetworkProfile::Unknown);
        if !profile.lan_allowed() {
            continue;
        }
        let adapter_name = unsafe { adapter.FriendlyName.to_string() }
            .unwrap_or_else(|_| "Windows network adapter".into());
        let adapter_id = format!("{:032x}", guid.to_u128());
        let mut unicast = adapter.FirstUnicastAddress;
        while !unicast.is_null() {
            let address = unsafe { &*unicast };
            unicast = address.Next;
            let Some(ipv4) = (unsafe { ipv4_from_sockaddr(address.Address.lpSockaddr) }) else {
                continue;
            };
            if !eligible_ipv4(ipv4) {
                continue;
            }
            ranked.push(CandidateRank {
                candidate: LanCandidate {
                    adapter_id: adapter_id.clone(),
                    adapter_name: adapter_name.clone(),
                    address: ipv4,
                    network_profile: profile.clone(),
                    recommended: false,
                },
                has_gateway: !adapter.FirstGatewayAddress.is_null(),
                ipv4_metric: adapter.Ipv4Metric,
            });
        }
    }
    ranked.sort_by_key(|item| (!item.has_gateway, item.ipv4_metric, item.candidate.adapter_id.clone()));
    if let Some(first) = ranked.first_mut() {
        first.candidate.recommended = true;
    }
    Ok(ranked.into_iter().map(|item| item.candidate).collect())
}

#[cfg(not(windows))]
fn inspect_lan_candidates_impl() -> Result<Vec<LanCandidate>, String> {
    Err("Phase 8.1 LAN inspection is currently implemented for Windows only.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lan_ipv4_filter_rejects_internal_and_apipa_addresses() {
        assert!(!eligible_ipv4(Ipv4Addr::new(127, 0, 0, 1)));
        assert!(!eligible_ipv4(Ipv4Addr::new(0, 0, 0, 0)));
        assert!(!eligible_ipv4(Ipv4Addr::new(169, 254, 10, 20)));
        assert!(!eligible_ipv4(Ipv4Addr::new(224, 0, 0, 1)));
        assert!(eligible_ipv4(Ipv4Addr::new(192, 168, 1, 25)));
    }
}
