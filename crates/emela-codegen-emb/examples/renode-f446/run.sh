#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CRATE_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
ROOT="$(cd "$CRATE_DIR/../.." && pwd)"

STM32_RENODE_ROOT="${STM32_RENODE_ROOT:-/home/aoi_nemui/bareruby/stm32_renode}"
ARM_TOOLCHAIN_PATH="${ARM_TOOLCHAIN_PATH:-$STM32_RENODE_ROOT/.tools/common/arm/xpack-arm-none-eabi-gcc-13.2.1-1.1}"
STM32CUBE_F4="${STM32CUBE_F4:-$STM32_RENODE_ROOT/.tools/stm32cube/STM32CubeF4-1.28.3}"
RENODE="${RENODE:-$STM32_RENODE_ROOT/.tools/stm32cube/renode-1.16.1/renode}"
FIXTURE="${1:-scalar}"
case "$FIXTURE" in
    scalar) EXAMPLE=renode_smoke; BOARD_SOURCE="$SCRIPT_DIR/board.c"; RUN_TIME=0.100000; MIPS=1; DEFAULT_OUT=/tmp/emela-codegen-emb-renode-smoke ;;
    completion) EXAMPLE=renode_completion; BOARD_SOURCE="$SCRIPT_DIR/completion_board.c"; RUN_TIME=0.500000; MIPS=100; DEFAULT_OUT=/tmp/emela-codegen-emb-renode-completion ;;
    *) printf 'Unknown fixture: %s (expected scalar or completion)\n' "$FIXTURE" >&2; exit 1 ;;
esac
OUT="${EMELA_EMB_RENODE_OUT:-$DEFAULT_OUT}"

CC="$ARM_TOOLCHAIN_PATH/bin/arm-none-eabi-gcc"
SIZE="$ARM_TOOLCHAIN_PATH/bin/arm-none-eabi-size"
NM="$ARM_TOOLCHAIN_PATH/bin/arm-none-eabi-nm"
CMSIS_DEVICE="$STM32CUBE_F4/Drivers/CMSIS/Device/ST/STM32F4xx"
CMSIS_CORE="$STM32CUBE_F4/Drivers/CMSIS/Include"
STARTUP="$CMSIS_DEVICE/Source/Templates/gcc/startup_stm32f446xx.s"
SYSTEM="$CMSIS_DEVICE/Source/Templates/system_stm32f4xx.c"
PLATFORM="${RENODE_PLATFORM:-$(dirname "$RENODE")/platforms/cpus/stm32f4.repl}"

for path in "$CC" "$SIZE" "$NM" "$RENODE" "$STARTUP" "$SYSTEM" "$PLATFORM"; do
    if [[ ! -e "$path" ]]; then
        printf 'Missing required tool or file: %s\n' "$path" >&2
        exit 1
    fi
done

mkdir -p "$OUT/objects" "$OUT/tmp" "$OUT/renode-config"
GENERATED="$OUT/generated"
cd "$ROOT"
CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" cargo run --locked -p emela-codegen-emb --example "$EXAMPLE" -- "$GENERATED"

ARCH_FLAGS=(-mcpu=cortex-m4 -mthumb -mfpu=fpv4-sp-d16 -mfloat-abi=hard)
COMMON_FLAGS=("${ARCH_FLAGS[@]}" -O2 -g3 -ffunction-sections -fdata-sections
    -DSTM32F446xx -std=gnu11 -Wall -Wextra -Werror -fstack-usage -I"$SCRIPT_DIR" -I"$GENERATED"
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

compile_c "$BOARD_SOURCE"
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
sysbus.cpu PerformanceInMips $MIPS
emulation RunFor "0:00:$RUN_TIME"
sysbus InfoLog "EMELA_GPIOA_ODR"
sysbus ReadDoubleWord 0x40020014
EOF
if [[ "$FIXTURE" == completion ]]; then
    python3 - "$NM" "$OUT/firmware.elf" "$OUT/run.resc" <<'PYMETRICS'
import subprocess
import sys
symbols = {}
for line in subprocess.check_output([sys.argv[1], "--defined-only", sys.argv[2]], text=True).splitlines():
    fields = line.split()
    if len(fields) == 3:
        symbols[fields[2]] = fields[0]
with open(sys.argv[3], "a", encoding="utf-8") as script:
    for metric in ["baseline", "iterations", "producers", "debugs"]:
        address = symbols[f"emela_completion_{metric}"]
        script.write(f'sysbus InfoLog "EMELA_COMPLETION_{metric.upper()}"\n')
        script.write(f"sysbus ReadDoubleWord 0x{address}\n")
PYMETRICS
fi
printf 'quit\n' >> "$OUT/run.resc"

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

python3 - "$OUT/renode.log" "$FIXTURE" <<'PY'
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
description = "const/sync/generic/derive cache, 1000 heap baselines and one initialization" if sys.argv[2] == "completion" else "generated Emela entry exercised both branches and the defect hook"
if sys.argv[2] == "completion":
    values = {}
    for metric in ["BASELINE", "ITERATIONS", "PRODUCERS", "DEBUGS"]:
        label = f"EMELA_COMPLETION_{metric}"
        try:
            metric_index = next(i for i, line in enumerate(log) if label in line)
            raw = next(line for line in log[metric_index + 1:] if re.fullmatch(r"\s*0x[0-9a-fA-F]+\s*", line))
        except StopIteration:
            raise SystemExit(f"FAIL: missing {label}")
        values[metric] = int(raw, 16)
    if values["BASELINE"] == 0 or values["ITERATIONS"] != 1000 or values["PRODUCERS"] != 1 or values["DEBUGS"] != 1:
        raise SystemExit(f"FAIL: completion metrics {values}")
    print(f"heap baseline={values['BASELINE']} bytes, iterations=1000, producers=1, generic initializations=1")
print(f"PASS: {description}, GPIOA_ODR=0x{value:08x}")
PY
