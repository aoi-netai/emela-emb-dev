// SPDX-License-Identifier: Apache-2.0
// GPIO test model for the native BSP's MMIO and mtvec-mode-1 trap path.
// Not a complete ESP32-C3: no ROM, flash, watchdogs, analog signals, or IRQ nesting.
using System;
using System.Collections.Generic;
using System.Collections.ObjectModel;
using System.Text;
using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals;
using Antmicro.Renode.Peripherals.Bus;
using Antmicro.Renode.Peripherals.CPU;
using Antmicro.Renode.Peripherals.Timers;
using Antmicro.Renode.Time;

namespace Antmicro.Renode.Peripherals.CPU
{
    // ESP32-C3 CPU IRQ 2 is a platform interrupt, reserved by generic modern
    // RISC-V. Register it in Renode's pending-interrupt engine. Line 16 only
    // wakes the generic CPU; it stays masked in mie and never becomes mcause.
    public class ESP32C3GpioTestCPU : RiscV32
    {
        public ESP32C3GpioTestCPU(IMachine machine) : base(machine, "rv32imc_zicsr_zifencei", privilegedArchitecture: PrivilegedArchitecture.Priv1_10)
        {
            RegisterCustomInternalInterrupt(2, mipTriggered: true);
        }
        public override void OnGPIO(int number, bool value)
        {
            if(number != 2) { base.OnGPIO(number, value); return; }
            TlibSetMipBit(2, value ? 1u : 0u);
            base.OnGPIO(16, value);
        }
    }
}

namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    [GPIO(NumberOfInputs = 22)]
    public class ESP32C3GpioTestSoC : IDoubleWordPeripheral, IKnownSize, INumberedGPIOOutput, IGPIOReceiver
    {
        public ESP32C3GpioTestSoC(IMachine machine, RiscV32 cpu)
        {
            this.cpu = cpu;
            var connections = new Dictionary<int, IGPIO>();
            for(int i = 0; i < 22; i++) connections.Add(i, new GPIO());
            Connections = new ReadOnlyDictionary<int, IGPIO>(connections);
            counter = new LimitTimer(machine.ClockSource, 16000000, this, "counter", ulong.MaxValue, Direction.Ascending, enabled: true);
            tick = new LimitTimer(machine.ClockSource, 1000, this, "tick", 1, Direction.Descending, enabled: false, eventEnabled: true, autoUpdate: true);
            tick.LimitReached += () => { timerPending = true; UpdateInterrupts(); };
            Reset();
        }

        public void Reset()
        {
            registers.Clear();
            text.Clear();
            output = enable = input = pending = 0;
            timerPending = false;
            counter.Reset();
            counter.Enabled = true;
            tick.Reset();
            tick.Enabled = false;
            UpdateOutputs();
            UpdateInterrupts();
        }

        public void OnGPIO(int number, bool value)
        {
            if(number < 0 || number >= 22) throw new ArgumentOutOfRangeException("number");
            uint bit = 1u << number;
            bool previous = (input & bit) != 0;
            input = value ? input | bit : input & ~bit;
            uint type = (Register(0x4074 + 4 * number) >> 7) & 7;
            if(previous != value && ((value && type == 1) || (!value && type == 2) || type == 3)) pending |= bit;
            UpdateOutputs();
            UpdateInterrupts();
        }

        public uint ReadDoubleWord(long offset)
        {
            switch(offset)
            {
                case 0x1c: return 0; // UART TX FIFO is always ready in this model.
                case 0x4004: return output;
                case 0x4020: return enable;
                case 0x403c: return Levels();
                case 0x4044: return pending;
                case 0x405c: return EnabledPending();
                case 0x23004: return 1u << 29; // Counter update completed.
                case 0x23040: return (uint)(counter.Value >> 32);
                case 0x23044: return (uint)counter.Value;
                case 0x23068:
                case 0x23070: return timerPending ? 1u : 0;
                default: return Register(offset);
            }
        }

        public void WriteDoubleWord(long offset, uint value)
        {
            switch(offset)
            {
                case 0: text.Append((char)(value & 255)); break;
                case 0x80: registers[offset] = value & ~(1u << 31); break; // UART update strobe.
                case 0x4004: output = value; break;
                case 0x4008: output |= value; break;
                case 0x400c: output &= ~value; break;
                case 0x4020: enable = value; break;
                case 0x4024: enable |= value; break;
                case 0x4028: enable &= ~value; break;
                case 0x4048: pending |= value; break;
                case 0x404c: pending &= ~value; break;
                case 0x2306c: if((value & 1) != 0) timerPending = false; break;
                default: registers[offset] = value; break;
            }
            // The BSP programs target0 for a 1ms periodic tick. Other timer modes are outside this model.
            tick.Enabled = (Register(0x23000) & (1u << 24)) != 0 && (Register(0x23064) & 1) != 0;
            UpdateOutputs();
            UpdateInterrupts();
        }

        public long Size { get { return 0xd0000; } }
        public string ConsoleText { get { return text.ToString(); } }
        public IReadOnlyDictionary<int, IGPIO> Connections { get; private set; }

        private uint Register(long offset) { uint value; return registers.TryGetValue(offset, out value) ? value : 0; }
        private uint Levels()
        {
            uint levels = input;
            for(int pin = 0; pin < 22; pin++)
            {
                uint bit = 1u << pin;
                if((enable & bit) == 0 || (Register(0x4554 + pin * 4) & 255) != 128) continue;
                bool high = (output & bit) != 0;
                bool openDrain = (Register(0x4074 + pin * 4) & 4) != 0;
                if(high && openDrain) high = (input & bit) != 0 || (Register(0x9004 + pin * 4) & (1u << 8)) != 0;
                levels = high ? levels | bit : levels & ~bit;
            }
            return levels;
        }
        private uint EnabledPending()
        {
            uint value = 0;
            for(int pin = 0; pin < 22; pin++) if((Register(0x4074 + pin * 4) & (1u << 13)) != 0) value |= pending & (1u << pin);
            return value;
        }
        private void UpdateOutputs()
        {
            uint levels = Levels();
            for(int pin = 0; pin < 22; pin++) Connections[pin].Set((enable & levels & (1u << pin)) != 0);
        }
        private void UpdateInterrupts()
        {
            // Generic Renode RISC-V uses mie/mip for its pending lines. Mirror the
            // ESP controller's enable mask there; firmware still writes only native MMIO.
            uint enabled = Register(0xc2104);
            cpu.MIE = enabled;
            cpu.OnGPIO(1, (enabled & 2) != 0 && Register(0xc2094) == 1 && timerPending);
            cpu.OnGPIO(2, (enabled & 4) != 0 && Register(0xc2040) == 2 && EnabledPending() != 0);
        }

        private readonly RiscV32 cpu;
        private readonly LimitTimer counter, tick;
        private readonly Dictionary<long, uint> registers = new Dictionary<long, uint>();
        private readonly StringBuilder text = new StringBuilder();
        private uint output, enable, input, pending;
        private bool timerPending;
    }
}
