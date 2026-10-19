#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CRATE_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
ROOT="$(cd "$CRATE_DIR/../.." && pwd)"

STM32_RENODE_ROOT="${STM32_RENODE_ROOT:-/home/aoi_nemui/bareruby/stm32_renode}"
ARM_TOOLCHAIN_PATH="${ARM_TOOLCHAIN_PATH:-$STM32_RENODE_ROOT/.tools/common/arm/xpack-arm-none-eabi-gcc-13.2.1-1.1}"
STM32CUBE_F4="${STM32CUBE_F4:-$STM32_RENODE_ROOT/.tools/stm32cube/STM32CubeF4-1.28.3}"
RENODE="${RENODE:-$STM32_RENODE_ROOT/.tools/stm32cube/renode-1.16.1/renode}"
OUT="${EMELA_EMB_RENODE_OUT:-/tmp/emela-codegen-emb-renode-smoke}"

CC="$ARM_TOOLCHAIN_PATH/bin/arm-none-eabi-gcc"
SIZE="$ARM_TOOLCHAIN_PATH/bin/arm-none-eabi-size"
CMSIS_DEVICE="$STM32CUBE_F4/Drivers/CMSIS/Device/ST/STM32F4xx"
CMSIS_CORE="$STM32CUBE_F4/Drivers/CMSIS/Include"
STARTUP="$CMSIS_DEVICE/Source/Templates/gcc/startup_stm32f446xx.s"
SYSTEM="$CMSIS_DEVICE/Source/Templates/system_stm32f4xx.c"
PLATFORM="${RENODE_PLATFORM:-$(dirname "$RENODE")/platforms/cpus/stm32f4.repl}"

for path in "$CC" "$SIZE" "$RENODE" "$STARTUP" "$SYSTEM" "$PLATFORM"; do
    if [[ ! -e "$path" ]]; then
        printf 'Missing required tool or file: %s\n' "$path" >&2
        exit 1
    fi
done

mkdir -p "$OUT/objects" "$OUT/tmp" "$OUT/renode-config"
GENERATED="$OUT/generated"
cd "$ROOT"
cargo run --locked -p emela-codegen-emb --example renode_smoke -- "$GENERATED"

ARCH_FLAGS=(-mcpu=cortex-m4 -mthumb -mfpu=fpv4-sp-d16 -mfloat-abi=hard)
COMMON_FLAGS=("${ARCH_FLAGS[@]}" -O2 -g3 -ffunction-sections -fdata-sections
    -DSTM32F446xx -std=gnu11 -I"$SCRIPT_DIR" -I"$GENERATED"
    -I"$CMSIS_DEVICE/Include" -I"$CMSIS_CORE")
OBJECTS=()

compile_c() {
    local source="$1"
    local object="$OUT/objects/$(basename "$source").o"
    "$CC" "${COMMON_FLAGS[@]}" -c "$source" -o "$object" \
        >"$object.log" 2>&1 || {
            cat "$object.log" >&2
            return 1
        }
    OBJECTS+=("$object")
}

compile_asm() {
    local source="$1"
    local object="$OUT/objects/startup.o"
    "$CC" "${COMMON_FLAGS[@]}" -x assembler-with-cpp -c "$source" -o "$object" \
        >"$object.log" 2>&1 || {
            cat "$object.log" >&2
            return 1
        }
    OBJECTS+=("$object")
}

compile_c "$SCRIPT_DIR/board.c"
compile_c "$GENERATED/emela_embedded.c"
compile_c "$SYSTEM"
compile_asm "$STARTUP"

"$CC" "${ARCH_FLAGS[@]}" -nostartfiles --specs=nano.specs --specs=nosys.specs \
    -T"$SCRIPT_DIR/firmware.ld" -Wl,--gc-sections \
    "-Wl,-Map,$OUT/firmware.map" "${OBJECTS[@]}" -lm -o "$OUT/firmware.elf" \
    >"$OUT/link.log" 2>&1 || {
        cat "$OUT/link.log" >&2
        exit 1
    }
"$SIZE" "$OUT/firmware.elf" | tee "$OUT/size.txt"

sed '/^[[:space:]]*ApplySVD[[:space:]]/d' "$PLATFORM" > "$OUT/stm32f4.repl"
cat > "$OUT/machine.repl" <<EOF
using "$OUT/stm32f4.repl"
dwt: Miscellaneous.DWT @ sysbus 0xE0001000
    frequency: 84000000
nvic:
    systickFrequency: 84000000
syscfg: Miscellaneous.STM32_SYSCFG @ sysbus 0x40013800
flash:
    size: 524288
sram:
    size: 131072
led: Miscellaneous.LED @ gpioPortA 5
gpioPortA:
    5 -> led@0
EOF
cat > "$OUT/run.resc" <<EOF
mach create
machine LoadPlatformDescription @$OUT/machine.repl
sysbus LoadELF @$OUT/firmware.elf
sysbus.cpu VectorTableOffset 0x08000000
sysbus.cpu PerformanceInMips 1
emulation RunFor "0:00:00.100000"
sysbus InfoLog "EMELA_GPIOA_ODR"
sysbus ReadDoubleWord 0x40020014
quit
EOF

XDG_CONFIG_HOME="$OUT/renode-config" TMPDIR="$OUT/tmp" \
    "$RENODE" --disable-xwt --console --plain -e "include @$OUT/run.resc" \
    >"$OUT/renode.log" 2>&1 || {
        cat "$OUT/renode.log" >&2
        exit 1
    }
if grep -Eq 'There was an error executing command|Unhandled Exception|\[ERROR\]' "$OUT/renode.log"; then
    cat "$OUT/renode.log" >&2
    exit 1
fi

python3 - "$OUT/renode.log" <<'PY'
import re
import sys

log = open(sys.argv[1], encoding="utf-8", errors="replace").read().splitlines()
try:
    index = next(i for i, line in enumerate(log) if "EMELA_GPIOA_ODR" in line)
    value_line = next(line for line in log[index + 1:] if re.fullmatch(r"\s*0x[0-9a-fA-F]+\s*", line))
except StopIteration:
    raise SystemExit("FAIL: Renode did not print the GPIOA ODR value")

value = int(value_line, 16)
expected = (1 << 5) | (1 << 6) | (1 << 7)
if value != expected:
    raise SystemExit(f"FAIL: expected GPIOA_ODR=0x{expected:08x}, got 0x{value:08x}")
print(f"PASS: generated Emela entry exercised both branches and the defect hook, GPIOA_ODR=0x{value:08x}")
PY
