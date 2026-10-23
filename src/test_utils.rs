pub fn u256_from_int(n: u32) -> binary_sv2::U256Owned {
    let mut bytes = [0; 32];
    bytes[..4].copy_from_slice(&n.to_le_bytes());
    bytes.into()
}
