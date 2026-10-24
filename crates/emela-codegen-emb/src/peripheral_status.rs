//! One definition of the numeric BSP status ABI and its GPIO classification.
pub(super) const STATUSES: &[(&str, i32, &str)] = &[
    ("INVALID_ARGUMENT", -1, "InvalidArgument"),
    ("CAPACITY_EXCEEDED", -2, "Other"),
    ("NOT_CONFIGURED", -3, "NotConfigured"),
    ("UNSUPPORTED", -4, "Unsupported"),
    ("TIMEOUT", -5, "Other"),
    ("BUSY", -6, "Busy"),
    ("RESOURCE_CONFLICT", -7, "ResourceConflict"),
    ("INVALID_STATE", -8, "InvalidState"),
    ("OTHER", -9, "Other"),
    ("PARITY", -10, "Other"),
    ("FRAMING", -11, "Other"),
    ("OVERRUN", -12, "Other"),
];
