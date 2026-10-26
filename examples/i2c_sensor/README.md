# I2Cセンサーの同期読出しと完了通知

STM32F4 Discoveryで、7bitアドレス`0x40`の接続先の8bitレジスタ`0`から2 byteを読む例。レジスタの意味と読出し可能な長さは接続先に合わせて変更する。

SCL=PB6、SDA=PB7、GNDを接続し、SCL/SDAに3.3Vへの外付けpull-upを用意する。PD12を通知用LEDとして使う。`configure → bind_interrupt → enable_interrupt`の順で準備する。転送と後処理の確定後にISRからLEDを変更し、callback終了後に同期呼び出しが戻る。ACKがなければ`False`、通信異常なら`I2cError`で処理する。

```sh
cargo build --locked -p emela-cli
target/debug/emela check examples/i2c_sensor
target/debug/emela build examples/i2c_sensor
```

クロスコンパイルにはBSPのArm toolchainが必要。[I2C仕様](../../crates/emela-codegen-emb/docs/embedded-interfaces/i2c.md)と[検証範囲](../../crates/emela-codegen-emb/docs/embedded-interfaces/i2c.md#検証)を参照する。

ESP32-C3では`Pome.toml`のboardを`esp32c3`、LEDを利用可能なGPIO（例：3）へ変更する。論理バスIDは同じ`I2c1`で、SCL=GPIO6、SDA=GPIO7になる。
