# 2つのSPIデバイス

STM32F4では論理`Spi2`をSPI2、SCK=PB13、MISO=PB14、MOSI=PB15へ接続する。CSはPB0/PB1。配線・mode・word幅・コマンドは接続先に合わせて変更する。SPI成功はcontroller側の転送完了を表し、機器の存在やコマンド受理の確認にはならない。

`SpiDeviceConfig`を`create_device`へ渡してCSを予約・初期化する。`transaction`はデバイスの設定を適用してCSを選択し、全operationの終了とhold時間を待ってから解除する。Writeには戻り値を作らず、Read/Transferの結果を順番に`List[List[Int]]`へ返す。

```sh
cargo build --locked -p emela-cli
target/debug/emela check examples/spi_devices
target/debug/emela build examples/spi_devices
```

ESP32-C3ではboardを`esp32c3`、CSをGPIO2/GPIO3へ変更する。論理バスは同じ`Spi2`で、SCK=GPIO0、MOSI=GPIO1、MISO=GPIO10。CSを接続する際はボードの起動時pin条件にも従う。

`SpiTransactionError`の`primary`は最初のSPI/GPIOエラー、`cleanup`は別途発生したCS解除エラーを持つ。解除に失敗したバスは`recover`でcontroller停止と全CSの解除が成功するまで使えず、単なる`configure`では復旧しない。

[SPI仕様](../../crates/emela-codegen-emb/docs/interface/spi-interface.md)と[検証範囲](../../crates/emela-codegen-emb/docs/spi-validation.md)を参照する。
