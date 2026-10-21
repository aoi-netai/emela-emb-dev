// SPDX-License-Identifier: Apache-2.0
// Register layout: ESP32-C3 TRM v1.4, chapter 5; ESP-IDF v5.4.1 soc headers.
// This model implements digital GPIO only; see README.md for its boundaries.
using System;
using System.Collections.Generic;
using System.Collections.ObjectModel;

using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Bus;

namespace Antmicro.Renode.Peripherals.GPIOPort
{
    [GPIO(NumberOfInputs = 22)]
    public class EmelaESP32C3_GPIO : IDoubleWordPeripheral, IKnownSize, INumberedGPIOOutput, IGPIOReceiver
    {
        public EmelaESP32C3_GPIO()
        {
            var connections = new Dictionary<int, IGPIO>();
            for(var pin = 0; pin < PinCount; pin++)
            {
                connections.Add(pin, new GPIO());
            }
            Connections = new ReadOnlyDictionary<int, IGPIO>(connections);
            input = new bool[PinCount];
            pinConfig = new uint[PinCount];
            outputSelection = new uint[PinCount];
            Reset();
        }

        public void Reset()
        {
            output = 0;
            enable = 0;
            Array.Clear(input, 0, input.Length);
            Array.Clear(pinConfig, 0, pinConfig.Length);
            for(var pin = 0; pin < PinCount; pin++)
            {
                outputSelection[pin] = GpioOutputSignal;
            }
            UpdateOutputs();
        }

        public void OnGPIO(int number, bool value)
        {
            if(number < 0 || number >= PinCount)
            {
                this.Log(LogLevel.Warning, "Unsupported GPIO input {0}; valid pins are 0..21", number);
                return;
            }
            input[number] = value;
        }

        public uint ReadDoubleWord(long offset)
        {
            int pin;
            if(TryPin(offset, PinConfigBase, out pin))
            {
                return pinConfig[pin];
            }
            if(TryPin(offset, OutputSelectionBase, out pin))
            {
                return outputSelection[pin];
            }
            switch(offset)
            {
                case 0x04:
                    return output;
                case 0x20:
                    return enable;
                case 0x3C:
                    uint value = 0;
                    for(pin = 0; pin < PinCount; pin++)
                    {
                        // Digital input observes the driven pad or an external input.
                        var high = IsDriving(pin) ? OutputLevel(pin) : input[pin];
                        if(high)
                        {
                            value |= 1u << pin;
                        }
                    }
                    return value;
                case 0x08:
                case 0x0C:
                case 0x24:
                case 0x28:
                    // The W1TS/W1TC aliases are write-only.
                    return 0;
                default:
                    this.Log(LogLevel.Warning, "Unsupported GPIO read at offset 0x{0:X}", offset);
                    return 0;
            }
        }

        public void WriteDoubleWord(long offset, uint value)
        {
            int pin;
            if(TryPin(offset, PinConfigBase, out pin))
            {
                pinConfig[pin] = value;
                if(value != 0)
                {
                    this.Log(LogLevel.Warning, "Unsupported GPIO{0} PIN configuration 0x{1:X}; output is suppressed", pin, value);
                }
                UpdateOutputs();
                return;
            }
            if(TryPin(offset, OutputSelectionBase, out pin))
            {
                outputSelection[pin] = value;
                if(!HasSupportedConfiguration(pin))
                {
                    this.Log(LogLevel.Warning, "Unsupported GPIO{0} output selection 0x{1:X}; output is suppressed", pin, value);
                }
                UpdateOutputs();
                return;
            }
            switch(offset)
            {
                case 0x04:
                    output = ValidPins(value);
                    break;
                case 0x08:
                    output |= ValidPins(value);
                    break;
                case 0x0C:
                    output &= ~ValidPins(value);
                    break;
                case 0x20:
                    enable = ValidPins(value);
                    break;
                case 0x24:
                    enable |= ValidPins(value);
                    break;
                case 0x28:
                    enable &= ~ValidPins(value);
                    break;
                default:
                    this.Log(LogLevel.Warning, "Unsupported GPIO write at offset 0x{0:X}, value 0x{1:X}", offset, value);
                    return;
            }
            UpdateOutputs();
        }

        public long Size { get { return 0x1000; } }

        public IReadOnlyDictionary<int, IGPIO> Connections { get; private set; }

        private static bool TryPin(long offset, long first, out int pin)
        {
            var relative = offset - first;
            pin = (int)(relative / 4);
            return relative >= 0 && relative < PinCount * 4 && relative % 4 == 0;
        }

        private uint ValidPins(uint value)
        {
            if((value & ~PinMask) != 0)
            {
                this.Log(LogLevel.Warning, "GPIO value 0x{0:X} selects unavailable pins above GPIO21", value);
            }
            return value & PinMask;
        }

        private bool HasSupportedConfiguration(int pin)
        {
            return pinConfig[pin] == 0
                && (outputSelection[pin] & 0xFF) == GpioOutputSignal
                && (outputSelection[pin] & ~0x7FFu) == 0;
        }

        private bool IsDriving(int pin)
        {
            // Signal 128 selects GPIO_ENABLE even when OEN_SEL is zero.
            return HasSupportedConfiguration(pin)
                && (((enable & (1u << pin)) != 0) ^ ((outputSelection[pin] & OutputEnableInvert) != 0));
        }

        private bool OutputLevel(int pin)
        {
            return ((output & (1u << pin)) != 0) ^ ((outputSelection[pin] & OutputInvert) != 0);
        }

        private void UpdateOutputs()
        {
            for(var pin = 0; pin < PinCount; pin++)
            {
                // Renode GPIO is binary; an undriven endpoint is represented as low.
                Connections[pin].Set(IsDriving(pin) && OutputLevel(pin));
            }
        }

        private uint output;
        private uint enable;
        private readonly bool[] input;
        private readonly uint[] pinConfig;
        private readonly uint[] outputSelection;
        private const int PinCount = 22;
        private const uint PinMask = (1u << PinCount) - 1;
        private const uint GpioOutputSignal = 128;
        private const uint OutputInvert = 1u << 8;
        private const uint OutputEnableInvert = 1u << 10;
        private const long PinConfigBase = 0x74;
        private const long OutputSelectionBase = 0x554;
    }
}

namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    // An independent wire receiver: no MMIO access and no controller internals.
    [GPIO(NumberOfInputs = 2)]
    public class EmelaGPIOObserver : IGPIOReceiver
    {
        public void Reset()
        {
            State = 0;
            TransitionCount = 0;
        }

        public void OnGPIO(int number, bool value)
        {
            if(number < 0 || number >= 2)
            {
                this.Log(LogLevel.Warning, "Unsupported observer input {0}; valid inputs are 0 and 1", number);
                return;
            }
            var mask = 1u << number;
            var previous = (State & mask) != 0;
            if(previous == value)
            {
                return;
            }
            State = value ? State | mask : State & ~mask;
            TransitionCount++;
            this.Log(LogLevel.Info, "EMELA_GPIO_PIN pin={0} level={1}", number, value ? 1 : 0);
        }

        public uint State { get; private set; }

        public ulong TransitionCount { get; private set; }
    }
}
