# GPIO割り込みでLEDを切り替える

STM32F4 DiscoveryのPA6入力に立ち上がりエッジが入るたびに、PD12の緑LEDを反転する。60秒後に割り込みを無効化し、LEDを消灯して終了する。

PA6に3.3 Vのデジタル信号を接続し、信号源とboardのGNDを共通にする。入力には内部pull-downを設定している。ボタンを使う場合はPA6と3.3 Vの間に接続できるが、この例はチャタリング除去を行わないため、1回の押下で複数回反転することがある。周期信号を入力すれば、その立ち上がりごとに点灯・消灯を繰り返す。

このブランチの開発版CLIをworkspace rootでbuildして実行する。GPIO未対応の旧版`emela`がPATHにある場合に混在させないため、実行ファイルのpathを明示する。

```sh
cargo build --locked -p emela-cli
./target/debug/emela check examples/gpio_interrupt
./target/debug/emela build examples/gpio_interrupt
```

`on_rising`はcaptureのない名前付き関数で、`Gpio.bind_interrupt`が静的に割り込みへ接続する。コールバック内の`read`と`write`はISR専用driverに接続される。ISR内では`toggle`や`sleep`を呼ばず、`GpioError`を内部で処理している。

main側の`GpioError`は捕捉して`-1`を返す。ISR内で捕捉した`GpioError`はその回のLED操作を終了する。いずれのエラーもConsoleには出力しない。

ESP32-C3で使う場合は`Pome.toml`のboardを`esp32c3`、`LED`のpin IDを`3`に変更し、GPIO3に抵抗を介して外付けLEDを接続する。`INPUT`のGPIO6はそのまま使用できる。
