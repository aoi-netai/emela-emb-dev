#!/usr/bin/env python3
"""Observe a CLI-built STM32F4 temperature controller after Flash reset."""
from pathlib import Path
import sys

CRATE = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(CRATE / "tools"))
from stm32f4_flash import main


if __name__ == "__main__":
    sys.exit(main())
