#!/usr/bin/env python3
"""Compatibility entry point for the original STM32F446 board identifier."""
from pathlib import Path
import sys

FAMILY = Path(__file__).resolve().parent.parent / "stm32/stm32f4"
sys.path.insert(0, str(FAMILY))
from build import main

if __name__ == "__main__":
    sys.exit(main(profile_path=FAMILY / "boards/stm32f446.json"))
