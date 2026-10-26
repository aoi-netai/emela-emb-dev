# 組み込みインターフェースの機能一覧

`emela-codegen-emb`が提供する組み込み向けインターフェースを、利用できる機能から探すための一覧。GPIO・UART・I²C・SPIは`Std.Embedded.*`のeffectを`Std.Embedded.Board`のhandlerでボードへ接続する。対応するcontroller・pin・設定はBSPごとの表を参照する。

## 機能一覧

| 機能 | 主な操作・用途 | 標準モジュール | ボード用handler | 詳細 |
| --- | --- | --- | --- | --- |
| GPIO | ピン設定、出力初期値、読み書き・反転、edge割り込み、静的callback | `Std.Embedded.Gpio` | `BoardGpio` | [操作一覧・対応pin・C ABI](gpio.md) |
| UART | 同期送受信、固定長／可変長受信、入力破棄、受信方式の選択、割り込み通知 | `Std.Embedded.Uart` | `BoardUart` | [操作一覧・対応設定・C ABI](uart.md) |
| I²C | 7bit master送受信、8/16bitレジスタアクセス、応答確認、転送単位の割り込み通知 | `Std.Embedded.I2c` | `BoardI2c` | [操作一覧・アドレス・C ABI](i2c.md) |
| SPI | 同期送受信・同時送受信、CS付きdevice、transaction、失敗後の復旧 | `Std.Embedded.Spi` | `BoardSpi` | [操作一覧・CS管理・C ABI](spi.md) |
| ISRからmainへの情報共有 | 4本×16 `Int`の固定容量queue、`try_push`／`try_pop`、共通割り込み優先度 | `Std.Embedded.Interrupt` | —（共有APIを直接使用） | [queueとISRの制約](uart.md#優先度と情報共有) |
| Clock | `sleep`による待機、`now`による時刻取得 | `Std.Clock` | `BoardClock` | [待機と時刻の契約](common.md#clockとconsole) |
| Console | `print_line`／`print_error_line`による行出力 | `Std.Console` | `BoardConsole` | [ConsoleのC接続](common.md#clockとconsole) |

`Clock`と`Console`は共通の標準effectを、上記handlerで組み込み向けに接続する。共有APIの宣言は[Std.Embedded.Interrupt](../../../emela-core/lib/std/embedded/interrupt.emel)を参照する。

## 共通の利用条件

- 同梱BSPはSTM32F4とESP32-C3。機能ごとのcontroller・pin・設定の対応範囲は、各文書と[ボードpackage](../../README.md#同梱bsp)で確認する。
- UART・I²C・SPIの転送はmain contextから同期的に行う。ISRからの呼び出しや利用可能な操作は、[共通契約](common.md)に従う。
- GPIO・UART・I²Cの`bind_interrupt`はIDと名前付きcallbackを静的に対応付ける。割り込みの有効化・停止は別の操作で行う。
- データ幅、転送容量、timeout、エラー分類、所有権は[共通契約](common.md)と各機能の操作契約に従う。生成headerをC宣言の正とする。

## 利用例

```emela
import Std.Embedded.Gpio.{Gpio, GpioPin, OutputPushPull, NoPull, High, GpioError}
import Std.Embedded.Board.{BoardGpio}

pub fn set_output() -> () fails GpioError {
  with BoardGpio {
    pin = GpioPin(5)
    gpio = use Gpio
    gpio.configure(pin, OutputPushPull(NoPull, High))
    gpio.write(pin, High)
  }
}
```

アプリは共通effectを使い、利用側のC実装が論理ピン5の物理的な接続を定める。全ての物理ボード用driverを標準ライブラリが提供するわけではない。使うhandlerのほかのoperationも生成runtimeから到達扱いになる場合があるため、C側は生成headerとlink結果に従って必要な関数を用意する。

`Std.Embedded.*`は操作を宣言し、`Std.Embedded.Board`はC ABIへ接続する。論理pinや機器IDの物理的な対応はBSPが定める。独自handlerで模擬する場合はホスト側のsourceテストにも使える。

通常の開発は[CLIのREADME](../../../emela-cli/README.md)を参照する。C bridge・Clock・Consoleの契約は[共通契約](common.md#c-bridge)、BSPのstartup・build・flashは[STM32F4](../../boards/stm32/stm32f4/README.md)と[ESP32-C3](../../boards/esp32c3/README.md)にまとめる。検証手順と確認範囲は各機能の文書にある。

ADC・PWM等の追加機能や、各機器の未対応設定は各機能の対象範囲を参照する。
