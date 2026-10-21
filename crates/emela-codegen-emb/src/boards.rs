//! Board support packages bundled with the compiler.

/// A file installed into the selected board's build directory.
pub struct BoardAsset {
    pub name: &'static str,
    pub contents: &'static [u8],
}

/// A board's reusable build, startup, and peripheral implementation.
pub struct BoardPackage {
    pub id: &'static str,
    pub assets: &'static [BoardAsset],
}

macro_rules! asset {
    ($board:literal, $name:literal) => {
        BoardAsset {
            name: $name,
            contents: include_bytes!(concat!("../boards/", $board, "/", $name)),
        }
    };
}

/// Canonical board identifiers accepted by the CLI and project configuration.
pub const BOARD_NAMES: &[&str] = &["stm32f446", "esp32c3"];

/// Verifies build inputs and images before invoking a board flasher.
pub const BUILD_STATE_SCRIPT: &str = include_str!("../boards/build_state.py");

/// Returns a package without depending on the compiler's source checkout at runtime.
pub fn package(id: &str) -> Option<BoardPackage> {
    let (id, assets): (_, &'static [BoardAsset]) = match id {
        "stm32f446" => (
            "stm32f446",
            &[
                asset!("stm32f446", "build.py"),
                asset!("stm32f446", "startup.S"),
                asset!("stm32f446", "firmware.ld"),
                asset!("stm32f446", "runtime.c"),
                asset!("stm32f446", "validation_stack.c"),
                asset!("stm32f446", "validation_stack.h"),
                asset!("stm32f446", "README.md"),
            ],
        ),
        "esp32c3" => (
            "esp32c3",
            &[
                asset!("esp32c3", "build.py"),
                asset!("esp32c3", "board.c"),
                asset!("esp32c3", "startup.S"),
                asset!("esp32c3", "firmware.ld"),
                asset!("esp32c3", "README.md"),
            ],
        ),
        _ => return None,
    };
    Some(BoardPackage { id, assets })
}
