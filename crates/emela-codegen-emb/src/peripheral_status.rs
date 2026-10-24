//! Shared numeric BSP status ABI and per-peripheral error classifications.
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
    ("NACK", -13, "Other"),
    ("BUS_ERROR", -14, "Other"),
    ("ARBITRATION_LOST", -15, "Other"),
];

/// Classify a shared BSP status for each peripheral's public error enum.
pub(super) fn error_kind(peripheral: &str, status: &str, common: &'static str) -> &'static str {
    match (peripheral, status) {
        ("uart" | "i2c", "CAPACITY_EXCEEDED") => "CapacityExceeded",
        ("uart" | "i2c", "TIMEOUT") => "Timeout",
        ("uart", "PARITY") => "Parity",
        ("uart", "FRAMING") => "Framing",
        ("uart", "OVERRUN") => "Overrun",
        ("i2c", "NACK") => "Nack",
        ("i2c", "BUS_ERROR") => "BusError",
        ("i2c", "ARBITRATION_LOST") => "ArbitrationLost",
        _ => common,
    }
}
