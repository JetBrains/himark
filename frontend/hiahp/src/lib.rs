pub mod docsync;
pub mod find;
pub mod fs;
pub mod fsroute;
pub mod lsproute;
pub mod open;
pub mod registry;
pub mod transport;
pub mod uris;
pub mod wire;

pub fn uuid_v4() -> String {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or(0);
    let stack = &seed as *const _ as usize as u128;
    let mut bits = seed ^ stack.rotate_left(64) ^ (std::process::id() as u128) << 96;
    let mut nibbles = String::with_capacity(36);
    for index in 0..32 {
        let nibble = (bits & 0xf) as u32;
        bits = bits >> 4 | (u128::from(nibble.wrapping_mul(2654435769)) << 100);
        match index {
            8 | 12 | 16 | 20 => nibbles.push('-'),
            _ => {}
        }
        let value = match index {
            12 => 4,
            16 => 8 | (nibble & 0x3),
            _ => nibble,
        };
        nibbles.push(char::from_digit(value, 16).expect("nibble"));
    }
    nibbles
}
