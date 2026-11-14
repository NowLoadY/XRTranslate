//! Loopback transport shared by the CLI and native automation clients.
use serde_json::Value;
use std::{
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpStream},
    time::Duration,
};

pub fn send(port: u16, request: &Value, timeout: Duration) -> Result<Value, String> {
    let mut stream =
        TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), timeout)
            .map_err(|e| format!("Cannot connect to the application on port {port}: {e}"))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    let body = request.to_string();
    write!(stream,"POST / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).map_err(|e|e.to_string())?;
    let mut response = String::new();
    stream
        .take(2_097_152)
        .read_to_string(&mut response)
        .map_err(|e| e.to_string())?;
    let (header, body) = response
        .split_once("\r\n\r\n")
        .ok_or("Invalid Director HTTP response")?;
    if !header.starts_with("HTTP/1.1 200 ") {
        return Err(header.lines().next().unwrap_or("HTTP error").into());
    }
    serde_json::from_str(body).map_err(|e| format!("Invalid Director JSON response: {e}"))
}
