//! Mining version validation required by BIP323.

/// BIP323 general-purpose version bits (5 through 28).
pub const VERSION_ROLLING_MASK: u32 = 0x1fff_ffe0;

/// Validate against the job's advertised version and rolling permission.
/// Standard-channel jobs always allow the BIP323 general-purpose bits.
pub fn validate_version(job: u32, submitted: u32, rolling: bool) -> Result<(), &'static str> {
    if !rolling && submitted != job {
        return Err(mining_sv2::ERROR_CODE_VERSION_ROLLING_NOT_ALLOWED);
    }
    if (submitted ^ job) & !VERSION_ROLLING_MASK != 0 {
        return Err(mining_sv2::ERROR_CODE_SUBMIT_SHARES_INVALID_NON_ROLLABLE_VERSION_BIT);
    }
    Ok(())
}
