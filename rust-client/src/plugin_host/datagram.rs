//! One exclusive datagram destination per consumer. Replacing a destination
//! fences old queued sends before any packet from its new owner can be sent.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    net::{SocketAddr, UdpSocket},
};

#[derive(Default)]
pub(super) struct Outputs {
    next: u64,
    entries: BTreeMap<String, (u64, SocketAddr, UdpSocket, Vec<Vec<u8>>)>,
}
impl Outputs {
    pub fn remove(&mut self, owner: &str) {
        if let Some((_, address, socket, packets)) = self.entries.remove(owner) {
            for packet in packets {
                let _ = socket.send_to(&packet, address);
            }
        }
    }
    pub fn configure(
        &mut self,
        owner: &str,
        address: Option<&str>,
        close_packets: Vec<Vec<u8>>,
    ) -> Result<(Value, Option<String>), String> {
        validate(&close_packets)?;
        let Some(address) = address else {
            self.remove(owner);
            return Ok((Value::Null, None));
        };
        let address: SocketAddr = address.parse::<SocketAddr>().map_err(|e| e.to_string())?;
        let socket = UdpSocket::bind(if address.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        })
        .map_err(|e| e.to_string())?;
        socket.set_nonblocking(true).map_err(|e| e.to_string())?;
        let revoked = self
            .entries
            .iter()
            .find(|(id, (_, target, _, _))| id.as_str() != owner && *target == address)
            .map(|(id, _)| id.clone());
        // A transferred destination must never receive its previous owner's final clear.
        if let Some(id) = &revoked {
            self.entries.remove(id);
        }
        if self
            .entries
            .get(owner)
            .is_some_and(|(_, old, _, _)| *old != address)
        {
            self.remove(owner);
        }
        self.next += 1;
        self.entries
            .insert(owner.into(), (self.next, address, socket, close_packets));
        Ok((json!(self.next), revoked))
    }
    pub fn send(&self, owner: &str, id: u64, packets: Vec<Vec<u8>>) -> Result<Value, String> {
        let (generation, address, socket, _) = self
            .entries
            .get(owner)
            .ok_or("Datagram destination is closed")?;
        if *generation != id {
            return Err("Datagram destination was replaced".into());
        }
        validate(&packets)?;
        for packet in packets {
            socket.send_to(&packet, address).map_err(|e| e.to_string())?;
        }
        Ok(Value::Null)
    }
}
fn validate(packets: &[Vec<u8>]) -> Result<(), String> {
    if packets.len() > 16 || packets.iter().any(|p| p.is_empty() || p.len() > 65507) {
        return Err("Invalid datagram batch".into());
    }
    Ok(())
}
impl Drop for Outputs {
    fn drop(&mut self) {
        for owner in self.entries.keys().cloned().collect::<Vec<_>>() {
            self.remove(&owner);
        }
    }
}
