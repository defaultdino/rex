use std::io::{self, ErrorKind};
use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::time::{Duration, Instant};

const MDNS: (Ipv4Addr, u16) = (Ipv4Addr::new(224, 0, 0, 251), 5353);
const SERVICE: &str = "_googlecast._tcp.local";
const PTR: u8 = 12;
const TXT: u16 = 16;
const SRV: u16 = 33;
const CAST_PORT: u16 = 8009;

#[derive(Debug)]
pub struct Device {
    pub name: String,
    pub ip: IpAddr,
}

/// Calls `found` once for each device that answers within `wait`
// shortcut: ipv4 on the default route's interface only, query each interface if devices go missing
pub fn discover(wait: Duration, mut found: impl FnMut(Device)) -> io::Result<()> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    socket.send_to(&query(), MDNS)?;

    let deadline = Instant::now() + wait;
    let mut seen: Vec<IpAddr> = Vec::new();
    let mut buf = [0; 9000];

    while let Some(left) = deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
    {
        socket.set_read_timeout(Some(left))?;
        let (len, from) = match socket.recv_from(&mut buf) {
            Ok(r) => r,
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => break,
            Err(e) => return Err(e),
        };
        if let Some(name) = parse(&buf[..len])
            && !seen.contains(&from.ip())
        {
            seen.push(from.ip());
            found(Device {
                name,
                ip: from.ip(),
            });
        }
    }
    Ok(())
}

fn query() -> Vec<u8> {
    let mut q = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    for label in SERVICE.split('.') {
        q.push(label.len() as u8);
        q.extend(label.as_bytes());
    }
    q.extend([0, 0, PTR, 0, 1]);
    q
}

fn parse(p: &[u8]) -> Option<String> {
    let questions = u16_at(p, 4)?;
    let records = (6..12)
        .step_by(2)
        .map(|i| u16_at(p, i).map(usize::from))
        .sum::<Option<usize>>()?;
    let mut i = 12;

    for _ in 0..questions {
        i = name_at(p, i)?.1 + 4;
    }

    let (mut ports, mut names) = (Vec::new(), Vec::new());

    for _ in 0..records {
        let (owner, at) = name_at(p, i)?;
        let kind = u16_at(p, at)?;
        let len = usize::from(u16_at(p, at + 8)?);
        let data = p.get(at + 10..at + 10 + len)?;
        match kind {
            SRV => ports.push((owner, u16_at(data, 4)?)),
            TXT => names.extend(friendly_name(data).map(|n| (owner, n))),
            _ => {}
        }
        i = at + 10 + len;
    }

    names
        .into_iter()
        .find(|(owner, _)| {
            ports
                .iter()
                .all(|(o, port)| o != owner || *port == CAST_PORT)
        })
        .map(|(_, name)| name)
}

fn u16_at(p: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_be_bytes(p.get(i..i + 2)?.try_into().ok()?))
}

fn name_at(p: &[u8], mut i: usize) -> Option<(String, usize)> {
    let mut name = String::new();
    let mut end = None;

    for _ in 0..128 {
        // prevent hostile packet loop
        let len = usize::from(*p.get(i)?);
        if len & 0xc0 == 0xc0 {
            end.get_or_insert(i + 2);
            i = usize::from(u16_at(p, i)? & 0x3fff);
        } else if len == 0 {
            return Some((name, end.unwrap_or(i + 1)));
        } else {
            if !name.is_empty() {
                name.push('.');
            }
            name.push_str(&String::from_utf8_lossy(p.get(i + 1..i + 1 + len)?));
            i += 1 + len;
        }
    }

    None
}

fn friendly_name(mut txt: &[u8]) -> Option<String> {
    while let Some((&len, rest)) = txt.split_first() {
        let entry = rest.get(..usize::from(len))?;
        if let Some(name) = entry.strip_prefix(b"fn=") {
            return Some(String::from_utf8_lossy(name).into_owned());
        }
        txt = &rest[usize::from(len)..];
    }
    None
}
