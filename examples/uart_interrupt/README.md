# UART受信割り込みでLEDを操作する

STM32F4 DiscoveryのUSART1へ届くバイナリ値1でPD12のLEDを点灯し、それ以外で消灯する。mainからは`Emela\n`を送信し、60秒後に割り込みを停止する。115200 baud / 8N1 / flow controlなし。

| 信号 | pin | 接続 |
| --- | --- | --- |
| UART1 TX | PA9 | 相手のRX |
| UART1 RX | PA10 | 相手のTX |
| LED | PD12 | 基板上の緑LED |
| GND | GND | 相手と共通 |

3.3Vロジックのserial adapterを使う。Consoleは別のUSART2（PA2/PA3）。送信する1は文字`1`（0x31）ではなく1byteの`0x01`。payload付きイベントのmatchとGPIO操作はISRで実行する。Consoleを使う処理はmainへ置く。

```sh
cargo build --locked -p emela-cli
./target/debug/emela check examples/uart_interrupt
./target/debug/emela build examples/uart_interrupt
```

ESP32-C3ではPome.tomlのboardを`esp32c3`へ変更し、LEDのGpioPinを3、UartConfigのoversamplingを`Uart.Native`へ変更する。UART1 TX=GPIO4 / RX=GPIO5、LEDはGPIO3へ抵抗付きで外付けする。

受信値をmainで処理する場合はISRで`Interrupt.try_push(Interrupt.Channel0, value)`を使い、mainで`Interrupt.try_pop`する。容量は16 Intで、pushのFalseが満杯を表す。1つのISR producerと1つのmain consumerを対応させる。キューはmainを自動で再開しない。

[API・対応設定](../../crates/emela-codegen-emb/docs/interface/uart-interface.md)と[Renodeでの検証手順](../../crates/emela-codegen-emb/docs/uart-validation.md)も参照。
