# STM32F4 HAL を参考にした Emela API 草案

関数シグネチャは `crates/emela-core/lib/std/embedded/` に Emela ソースとして置いた。ここではその宣言の設計根拠と STM32F4 HAL との対応案を記録する。HAL の `GPIO_TypeDef*`、`UART_HandleTypeDef*` などは Emela の公開 API に出さず、将来の C ボードバインディングで対応付ける。

## 共通型

```emel
pub error PeripheralError(code: Int)
```

各エフェクトの引数・設定型はそれぞれ `Std.Embedded.Gpio`、`Std.Embedded.Uart`、`Std.Embedded.I2c`、`Std.Embedded.Spi` に置いた。

## GPIO

Emela の `GpioPin` はボード内の論理ピン ID とし、STM32 の GPIO ポート番号は公開しない。UART などの代替機能も GPIO の設定引数には出さず、ボード定義の周辺機器ルートに記録する。STM32 HAL の出力速度は HAL アダプター側の既定値とする。

```emel
pub effect GPIO {
  fn configure(pin: GpioPin, mode: GpioMode, pull: Pull) -> ()
  fn read(pin: GpioPin) -> PinState
  fn write(pin: GpioPin, state: PinState) -> ()
  fn toggle(pin: GpioPin) -> ()
  fn enable_interrupt(pin: GpioPin, edge: Edge) -> ()
  fn disable_interrupt(pin: GpioPin) -> ()
}
```

割り込み先の Emela 関数は通常処理からポーリングせず、静的なビルド時バインドで GPIO ピンと結び付ける。関数の指定方法は未決であり、次は形を示すだけの Emela 例である。

```emel
fn button_changed(event: GpioEvent) use { GPIO } {
  gpio = use GPIO
  gpio.write(GpioPin(13), event.level)
}
```

この関数は HAL コールバックから ISR 内で呼ぶ。実行時の関数ポインター登録は行わず、生成 C の静的な中継関数を介する方針とする。

ボード内の `GpioPin(id: 13)` がどの MCU ピンを指すかはボード定義で決める。初期例の NUCLEO-F401RE では ID `0`〜`13` を Arduino の D0〜D13 と対応させる。

## UART

`data_bits` はパリティビットを含まないデータ幅を表す。STM32F4 HAL の `WordLength` はフレーム幅に相当し、パリティ有効時は最上位ビットがパリティに使われるため、Emela の `data_bits` から HAL の `WordLength` へ変換する。

```emel
pub effect UART {
  fn configure(id: UartId, config: UartConfig) -> () fails PeripheralError
  fn transmit(id: UartId, bytes: List[Int], timeout_ms: Int64) -> () fails PeripheralError
  fn receive(id: UartId, size: Int, timeout_ms: Int64) -> List[Int] fails PeripheralError
}
```

## I2C（初期案はマスター）

```emel
pub effect I2C {
  fn configure(id: I2cId, config: I2cConfig) -> () fails PeripheralError
  fn master_transmit(id: I2cId, address: Int, bytes: List[Int], timeout_ms: Int64) -> () fails PeripheralError
  fn master_receive(id: I2cId, address: Int, size: Int, timeout_ms: Int64) -> List[Int] fails PeripheralError
  fn memory_write(id: I2cId, address: Int, memory_address: Int, address_size: MemoryAddressSize, bytes: List[Int], timeout_ms: Int64) -> () fails PeripheralError
  fn memory_read(id: I2cId, address: Int, memory_address: Int, address_size: MemoryAddressSize, size: Int, timeout_ms: Int64) -> List[Int] fails PeripheralError
  fn is_device_ready(id: I2cId, address: Int, trials: Int, timeout_ms: Int64) -> Bool fails PeripheralError
}
```

`address` は HAL に渡すシフト済み値ではなく、通常の 7-bit または 10-bit アドレスとする案である。ボードバインディングで HAL の引数形式へ変換する。

## SPI

```emel
pub effect SPI {
  fn configure(id: SpiId, config: SpiConfig) -> () fails PeripheralError
  fn transmit(id: SpiId, data: List[Int], timeout_ms: Int64) -> () fails PeripheralError
  fn receive(id: SpiId, size: Int, timeout_ms: Int64) -> List[Int] fails PeripheralError
  fn transfer(id: SpiId, data: List[Int], timeout_ms: Int64) -> List[Int] fails PeripheralError
}
```

## 草案上の対応範囲

- UART、I2C、SPI の転送は HAL のポーリング API に対応する同期操作としている。`_IT`、`_DMA`、HAL コールバックは Emela の API に出していない。
- 操作はそれぞれ `HAL_GPIO_Init` / `HAL_GPIO_ReadPin` / `HAL_GPIO_WritePin` / `HAL_GPIO_TogglePin`、`HAL_UART_Init` / `HAL_UART_Transmit` / `HAL_UART_Receive`、`HAL_I2C_Master_*` / `HAL_I2C_Mem_*`、`HAL_SPI_Transmit` / `HAL_SPI_Receive` / `HAL_SPI_TransmitReceive` に対応する想定である。
- GPIO 割り込みは HAL の EXTI コールバックから、静的にバインドした Emela ハンドラーを ISR 内で直接呼ぶ方針に改めた。ISR 制約の検査とハンドラーのバインド記法は未実装。遅延処理が必要な場合に限り、固定容量キューを別 API として検討する。
- GPIO の Emela API は論理ピン ID を受け取る。HAL のポート、ピン番号、UART などの代替機能番号はボード定義と HAL アダプターに置く。
- `List[Int]` の要素範囲は UART が `data_bits` に応じて 0〜127 / 0〜255 / 0〜511、I2C が 0〜255、SPI が `data_size` に応じて 0〜255 / 0〜65535 とする。HAL が 9-bit UART 用に要求するバッファー形式への変換は C のボードバインディングで行う。`List` は通常処理で使う型であり、ISR からは使わない。
- タイムアウトはミリ秒単位で、`Int64` の非負値を HAL の `uint32_t` 範囲へ検査して渡す。
- F4 の型番やボードに存在しないポート・周辺機器 ID、またはその UART が実現できないデータ幅・パリティの組み合わせは、ボードバインディング時にエラーにする。設定値が実行時まで決まらない場合は `configure` が HAL を呼ぶ前に検査し、`PeripheralError` を返す。
- Emela 側では STM32 の `USART` / `UART` という物理名称を公開せず、シリアル通信の論理 ID を `Uart0` から割り当てる。各 ID と実際の HAL ハンドルの対応はボード設定で決める。たとえば `Uart0` が `USART1` に対応するとは限らない。
- HAL の全設定項目は公開しない。UART の高度なサンプリング設定、I2C のスレーブ設定、SPI の TI モードや CRC などは初期 API の対象外とし、必要になった段階で追加する。SPI のチップセレクトは GPIO として操作する。

## ボードバインディング検査

ボードバインディングでは、少なくとも次をコード生成時に検査し、成立しない場合は対象の論理 ID と理由を示す診断を出す。

- `Uart0` などの論理 ID が、対象 MCU に実在する UART/USART とピン設定に結び付いていること。
- 要求する `data_bits`、パリティ、ストップビットなどの組み合わせを、その周辺機器と HAL で実現できること。
- 同じ周辺機器や競合するピンを複数の論理 ID に割り当てていないこと。

STM32F4 HAL の UART フレーム形式では、7-bit データはパリティ有効時に実現する（8-bit word length の最上位ビットをパリティに使う）。したがって、たとえば 7-bit + even/odd parity は対応可能だが、7-bit + parity none はこの HAL の UART 設定では表現できず、ボードバインディングで拒否する。STM32F4 以外のボードは機能が異なる可能性があるため、同じ API を使いつつボードごとの対応能力で検査する。

この検査は Emela の `PeripheralError` とは別の、コンパイル時のボード設定エラーとして扱う。実行時に決まる設定値については、実行時検査のエラーを返す。

## 参照した HAL 宣言

- [GPIO HAL ヘッダー](https://github.com/STMicroelectronics/stm32f4xx-hal-driver/blob/master/Inc/stm32f4xx_hal_gpio.h)
- [UART HAL ヘッダー](https://github.com/STMicroelectronics/stm32f4xx-hal-driver/blob/master/Inc/stm32f4xx_hal_uart.h)
- [I2C HAL ヘッダー](https://github.com/STMicroelectronics/stm32f4xx-hal-driver/blob/master/Inc/stm32f4xx_hal_i2c.h)
- [SPI HAL ヘッダー](https://github.com/STMicroelectronics/stm32f4xx-hal-driver/blob/master/Inc/stm32f4xx_hal_spi.h)

## 参照した NUCLEO-F401RE のピン割り当て

ボード定義の例は NUCLEO-F401RE（STM32F401RET6）を対象とする。UM1724 では Arduino D0/D1 が PA3/PA2（USART2_RX/TX）、D2/D8 が PA10/PA9 と示されている。STM32F401 のデータシートでは PA10/PA9 の USART1 機能は AF7 である。D0/D1 と STM32 の PA2/PA3 の接続は SB62/SB63 に依存する。

- [STM32 Nucleo-64 boards user manual UM1724](https://www.st.com/resource/en/user_manual/dm00105823.pdf)
- [STM32F401xD/xE datasheet](https://www.st.com/resource/en/datasheet/stm32f401ve.pdf)
