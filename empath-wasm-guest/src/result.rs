/// Clamp a rejection code into the SMTP error range expected by the host.
#[must_use]
pub fn normalize_rejection_code(code: u16) -> u16 {
    if (400..600).contains(&code) {
        code
    } else {
        550
    }
}
