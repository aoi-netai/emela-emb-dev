# 温度調節器用のESP32-C3 GPIOモデル

`EmelaESP32C3_GPIO.cs`は、温度調節器のGPIO出力を外部のLED・観測receiverへ接続するための独自実装である。上流`renode-espemu`のGPIO実装は取り込まず、Espressifのレジスタ仕様とRenodeの公開GPIO APIに従って実装した。リポジトリと同じApache-2.0で提供する。

既存のCPU/MMIO/IRQ probeとSDK Helloは、従来の固定上流モデルを引き続き使う。温度調節器のplatformだけでこのモデルを選択する。

## 実装範囲

GPIOのbaseは`0x60004000`、公開pinは実チップのGPIO0〜21に対応する。GPIO22以上への書き込みはwarningを出し、そのbitを無視する。

| offset | register | 動作 |
| --- | --- | --- |
| `0x04` | `GPIO_OUT` | 出力latchの読み書き |
| `0x08` / `0x0C` | `OUT_W1TS` / `OUT_W1TC` | 指定したbitだけをset／clear |
| `0x20` | `GPIO_ENABLE` | 出力enableの読み書き |
| `0x24` / `0x28` | `ENABLE_W1TS` / `ENABLE_W1TC` | 指定したenableだけをset／clear |
| `0x3C` | `GPIO_IN` | 出力有効時は実際の出力level、無効時は外部receiver入力 |
| `0x74 + 4*n` | `GPIO_PINn` | 通常push-pullの設定`0`に対応 |
| `0x554 + 4*n` | `GPIO_FUNCn_OUT_SEL_CFG` | GPIO出力信号`128`の選択と出力／enable反転 |

`OUT_SEL=128`は`GPIO_OUT`と`GPIO_ENABLE`を選択する。reset値も128である。`OEN_SEL=1`を明示する`0x280`にも対応する。温度ボードはconfigureで`PINn=0`、`OUT_SEL=0x280`、`ENABLE_W1TS`を書き、出力には`OUT_W1TS`／`OUT_W1TC`を使う。

W1TS／W1TCは書き込み専用で、読み出すと0になる。`GPIO_OUT`の読み出しは出力latchであり、enableが無効でも変更できる。外部endpointはenableと選択した出力levelが両方有効な場合にhighになる。出力有効時は外部入力より内部のpush-pull出力を優先する。

## 外部への接続

Monitorで先に次を実行する。

```text
include @EmelaESP32C3_GPIO.cs
```

platformの例は次のとおり。LEDと観測receiverは`sysbus`の下へ登録し、`|`で同じ出力を分岐する。

```text
gpio: GPIOPort.EmelaESP32C3_GPIO @ sysbus 0x60004000
    3 -> heater@0 | observer@0
    4 -> cooler@0 | observer@1
    5 -> gpio@6
heater: Miscellaneous.LED @ sysbus
cooler: Miscellaneous.LED @ sysbus
observer: Miscellaneous.EmelaGPIOObserver @ sysbus
```

上のGPIO5→6は外部endpointを通すwire loopbackの例であり、モデル内部の`IN=OUT`の代入ではない。入力は`IGPIOReceiver.OnGPIO`で更新される。未接続の外部入力はreset後lowとする。

`EmelaGPIOObserver`は独立したreceiverで、GPIOレジスタもCプログラムの変数も参照しない。入力0をheater、入力1をcoolerとして受け、変化時に`EMELA_GPIO_PIN pin=0 level=1`形式のInfo logを出す。公開`State`のbit0／1が現在のlevel、`TransitionCount`が変化数である。初期lowおよび同じ値の通知は変化数に含めない。runnerはこの履歴とLEDの`State`を、C側のsample履歴と照合する。

## 未対応と近似

- IO MUX、padの駆動強度、pull-up／down、アナログ機能、sleep、input filterはモデル化しない。このplatformではpadを通常のdigital GPIOかつinput有効と仮定する。GPIO3／4のレジスタ操作からLEDまでの接続確認であり、実機のpad設定を検証するものではない。
- GPIO以外の周辺機器出力信号、input matrix、GPIO割り込み、wake-up、open-drain、入力同期は未対応である。`PINn`の非0設定・GPIO信号以外の出力選択・不明なoffsetへの操作はwarningを出す。未対応pin設定／出力選択のpinは出力を抑止する。設定値のreadbackだけを成功の根拠にしない。
- RenodeのGPIO endpointはboolなのでhigh impedanceや電圧・電流・競合を表せない。出力無効のendpointはlowとして通知する一方、`GPIO_IN`は独立した外部入力を読む。外部receiver側のpull-upを再現したり、入力値を出力endpointへ再送したりはしない。
- 複数pinへの一括writeは同じMMIO操作内でpin番号順に通知する。電気的な同時性やglitch幅の測定には使わない。
- GPIOの未対応warningは温度調節器runnerでは失敗として扱う。未対応機能を成功するstubとして使わない。

## 単体確認

Renode 1.16.1で、一時的なMonitor scriptにより46項目を確認した。出力latchとenableの独立性、W1TS／W1TC、外部入力のhigh／low、GPIO5→6の実endpoint loopback、出力／enable反転、LEDとobserverへの分岐、reset後の状態が期待どおりだった。未対応open-drain・別matrix信号・GPIO22以上・未知offset・未整列offsetの6件はwarningと抑止／無視を確認した。単体platformのreset macro未登録warningは、これらの6件と区別している。

この確認はGPIOモデルの接続と動作を対象とする。実Emelaアプリ・scheduler・割り込みとの統合結果は温度調節器のrunnerとreportを参照する。

## 根拠資料

- [ESP32-C3 Technical Reference Manual v1.4](https://documentation.espressif.com/esp32-c3_technical_reference_manual_en.pdf)：第5章、特に5.14のレジスタ一覧、5.15のGPIO registerと出力選択。
- [ESP-IDF v5.4.1 gpio_reg.h](https://github.com/espressif/esp-idf/blob/v5.4.1/components/soc/esp32c3/register/soc/gpio_reg.h)：offset、bit位置、reset値。
- [ESP-IDF v5.4.1 gpio_sig_map.h](https://github.com/espressif/esp-idf/blob/v5.4.1/components/soc/esp32c3/include/soc/gpio_sig_map.h)：`SIG_GPIO_OUT_IDX=128`。
- [ESP-IDF v5.4.1 gpio_ll.h](https://github.com/espressif/esp-idf/blob/v5.4.1/components/hal/esp32c3/include/hal/gpio_ll.h)：GPIO出力とenableのW1TS／W1TC、GPIO matrixの既定選択。
- [Renode platform description](https://renode.readthedocs.io/en/latest/advanced/platform_description_format.html#interrupt-attributes)：GPIO receiverへの接続と分岐構文。
