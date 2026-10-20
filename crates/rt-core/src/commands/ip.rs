use lt_common::{Extents, Object};
use rt_rpc::command_flags::*;
use rt_rpc::{convert_to_string, convert_to_value, CommandMap, RpcError};
use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::sync::{Arc, RwLock};

pub fn ipv4_range_parse(address: &str) -> Option<(u32, u32)> {
    let s = address.trim();
    if s.is_empty() {
        return None;
    }

    // Check for explicit range: A-B
    if let Some((start_s, end_s)) = s.split_once('-') {
        let start_ip: Ipv4Addr = start_s.trim().parse().ok()?;
        let end_ip: Ipv4Addr = end_s.trim().parse().ok()?;
        let start_u32 = u32::from(start_ip);
        let end_u32 = u32::from(end_ip);
        if start_u32 <= end_u32 {
            return Some((start_u32, end_u32));
        } else {
            return None;
        }
    }

    // Check for CIDR: IP/mask
    if let Some((ip_s, mask_s)) = s.split_once('/') {
        let ip: Ipv4Addr = ip_s.trim().parse().ok()?;
        let mask: u32 = mask_s.trim().parse().ok()?;
        if mask > 32 {
            return None;
        }

        let ip_u32 = u32::from(ip);
        if mask == 0 {
            return Some((0, u32::MAX));
        }

        let mask_bits = 32 - mask;
        let mask_val = (!0u32) << mask_bits;
        let start = ip_u32 & mask_val;
        let end = start | (!mask_val);
        return Some((start, end));
    }

    // Single IP
    let ip: Ipv4Addr = s.parse().ok()?;
    let ip_u32 = u32::from(ip);
    Some((ip_u32, ip_u32))
}

#[derive(Default)]
pub struct IpTableManager {
    tables: BTreeMap<String, Extents<u32, u32>>,
}

impl IpTableManager {
    pub fn new() -> Self {
        Self {
            tables: BTreeMap::new(),
        }
    }

    pub fn insert_table(&mut self, name: &str) -> Result<(), RpcError> {
        if self.tables.contains_key(name) {
            return Err(RpcError::InputError("IP table already exists.".to_string()));
        }
        self.tables.insert(name.to_string(), Extents::new());
        Ok(())
    }

    pub fn size_data(&self, name: &str) -> Result<u32, RpcError> {
        let table = self
            .tables
            .get(name)
            .ok_or_else(|| RpcError::InputError("IP table does not exist.".to_string()))?;
        Ok(table.len() as u32)
    }

    pub fn add_address(&mut self, name: &str, address: &str, value: u32) -> Result<(), RpcError> {
        let (start, end) = ipv4_range_parse(address)
            .ok_or_else(|| RpcError::InputError("Invalid address format.".to_string()))?;
        let table = self
            .tables
            .get_mut(name)
            .ok_or_else(|| RpcError::InputError("Could not find ip table.".to_string()))?;
        table.insert(start, end, value);
        Ok(())
    }

    pub fn get(&self, name: &str, address: &str) -> Result<u32, RpcError> {
        let (start, end) = ipv4_range_parse(address)
            .ok_or_else(|| RpcError::InputError("Invalid address format.".to_string()))?;
        let table = self
            .tables
            .get(name)
            .ok_or_else(|| RpcError::InputError("Could not find ip table.".to_string()))?;

        table
            .at_range(start, end)
            .map_err(|_| RpcError::InputError("No value defined for specified IP(s).".to_string()))
    }
}

fn to_str(obj: &Object) -> Result<String, RpcError> {
    convert_to_string(obj)
}

fn to_val(obj: &Object) -> Result<i64, RpcError> {
    convert_to_value(obj, 10, 1)
}

pub fn register_ip_commands(
    map: &mut CommandMap,
    manager: Arc<RwLock<IpTableManager>>,
) -> Result<(), RpcError> {
    let mgr1 = Arc::clone(&manager);
    map.insert_func(
        "ip_tables.insert_table",
        move |_target, obj| {
            let name = to_str(obj)?;
            let mut mgr = mgr1.write().unwrap();
            mgr.insert_table(&name)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<table>",
        "Create a new IP table.",
    )?;

    let mgr2 = Arc::clone(&manager);
    map.insert_func(
        "ip_tables.size_data",
        move |_target, obj| {
            let name = to_str(obj)?;
            let mgr = mgr2.read().unwrap();
            let size = mgr.size_data(&name)?;
            Ok(Object::from(size as i64))
        },
        FLAG_PUBLIC_RPC,
        "<table>",
        "Get size of IP table data.",
    )?;

    let mgr3 = Arc::clone(&manager);
    map.insert_func(
        "ip_tables.add_address",
        move |_target, obj| {
            let list = obj.as_list()?;
            if list.len() < 2 {
                return Err(RpcError::InputError(
                    "Incorrect number of arguments.".to_string(),
                ));
            }
            let name = to_str(&list[0])?;
            let addr = to_str(&list[1])?;
            let val = if list.len() > 2 {
                to_val(&list[2])? as u32
            } else {
                1
            };

            let mut mgr = mgr3.write().unwrap();
            mgr.add_address(&name, &addr, val)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<table>, <address>, [value]",
        "Add IP address or range to table.",
    )?;

    let mgr4 = Arc::clone(&manager);
    map.insert_func(
        "ip_tables.get",
        move |_target, obj| {
            let list = obj.as_list()?;
            if list.len() != 2 {
                return Err(RpcError::InputError(
                    "Incorrect number of arguments.".to_string(),
                ));
            }
            let name = to_str(&list[0])?;
            let addr = to_str(&list[1])?;

            let mgr = mgr4.read().unwrap();
            let val = mgr.get(&name, &addr)?;
            Ok(Object::from(val as i64))
        },
        FLAG_PUBLIC_RPC,
        "<table>, <address>",
        "Look up IP address in table.",
    )?;

    Ok(())
}
