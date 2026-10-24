# UARTのCLI・BSP・Renode検証

2026-10-24。STM32F407VGとESP32-C3のアプリ用UART1を、Consoleと分離して確認する。[対応MCU/profile/controller/pin/設定とAPI](interface/uart-interface.md)を参照する。

## 再現手順

開発checkoutのCLIをビルドする。PATH上にある古いEmelaでは追加した標準モジュール/APIを利用できないため、下記ではバイナリを明示する。

```sh
cargo build --locked -p emela-cli
export ARM_TOOLCHAIN_PATH=/path/to/arm-none-eabi-toolchain
export EMELA_ESP32C3_TOOLCHAIN=/path/to/riscv32-esp-elf
# BSPで指定されたesptool 4.8.1をPythonから利用できる状態にする。
python3 crates/emela-codegen-emb/tools/uart_matrix.py \
  --emela target/debug/emela \
  --renode /path/to/renode-1.16.1/renode \
  --output target/uart-validation
```

既定で2 MCU × 2受信方式 × O0/O2の8 caseを生成する。各caseは別のfirmware起動であり、UARTの受信方式を動作中に切り替えない。`--boards`・`--modes`・`--optimizations`で範囲を指定できる。

通常の`emela check`・`emela build`から生成し、BSPをクロスコンパイルして実行する。生成先は`<board>/<mode>/<optimization>/project`・`build`・`observed`。総合`report.json`にfirmware hash、モデル情報、受信方式、観測値、stack、資源量を記録する。PCの書換えやISR関数の直接呼出しによる代用はしない。

## 自動確認の範囲

| 試験 | 確認内容 |
| --- | --- |
| host生成C | 3イベント入口、複数error flag、正しいID、同じprivate関数の複数IDへのbinding、callback/ID/関数別名、ISRでのheap確保0、mainのList/String/errorとの共存、status分類 |
| compiler拒否 | 動的ID・lambda・重複binding、Console等の禁止操作、heap、ISRのpop、複数ISRの同一channel使用、main producerとの競合（高階関数・関数別名・record保存・closure経由を含む）、動的channel |
| 共通BSP state machine | 実装そのものをhostでコンパイルし、設定維持・方向/受信方式・空転送・負の引数・word幅・32word ring overflow・受信異常・prefix消費・ゼロ期限の無消費・全体期限・TC/timeout競合・失敗後の再利用を確認。ASan/UBSan付き |
| 共有queue | 16 Intの上限、満杯のFalse、FIFO、channel分離、slot再利用、IRQ lockの保存/復元 |
| Renode Polling | 0/改行/255の順序、RX IRQ→通常受信ring、要求数不足時の無消費、固定長/receive_some、上限付き破棄、期限切れ後の受信、9600から115200への再設定、Pollingと送信完了通知の併用、GPIO pin予約競合 |
| Renode InterruptReceive | TC通知後の同期返却、受信→GPIO、size 0も通常受信禁止、ISR→固定queue→main、停止後の入力抑止、旧pendingの除去、再設定時の方式維持、再開、ESP32-C3のparity+framing集合 |
| GPIO回帰 | 両BSP O2で既存の9 IRQ・18checkpoint、guardを確認 |

UART matrixの合否は8 caseすべてでアプリ結果42、defect 0、stack guard正常、送信wordが正確に`[0, 10, 255]`、イベント数・LED観測値の一致とする。PollingはTC 1回、InterruptReceiveはSTM32F4でTC 1回+RX 4回、ESP32-C3でさらにerror集合1回。空送信からは通知しない。

## 今回の結果

8 caseすべて合格。codegen 422件、core全件（既存ignore 1件）、CLI関連12件も合格。ローカルの`target/uart-validation-20261024/report.json`と`target/uart-gpio-validation-20261024/report.json`に観測結果を保存する。使用toolはArm GCC 13.2.1、Espressif GCC 14.2.0_20241119、esptool 4.8.1、Renode 1.16.1。

| MCU | 受信方式 | 最適化 | code / rodata (byte) | static RAM (byte) | 観測stack (byte) |
| --- | --- | --- | ---: | ---: | ---: |
| stm32f407vg | polling | O0 | 42968 | 24856 | 2092 |
| stm32f407vg | polling | O2 | 25084 | 24864 | 576 |
| stm32f407vg | interrupt | O0 | 40804 | 25144 | 1956 |
| stm32f407vg | interrupt | O2 | 23732 | 25152 | 568 |
| esp32c3 | polling | O0 | 52816 | 24680 | 2372 |
| esp32c3 | polling | O2 | 30564 | 24668 | 620 |
| esp32c3 | interrupt | O0 | 50608 | 24968 | 2212 |
| esp32c3 | interrupt | O2 | 28944 | 24956 | 660 |

stackはこの検証アプリの観測high-waterであり、任意のISRの最大値ではない。STM32F4は16KiBのstackと別領域の256byte guard、ESP32-C3は16KiB予約に256byte guardを含む。

## モデルと実機確認の境界

STM32F4はRenode標準のUSART1・NVIC・GPIOを使い、Flash binaryからresetする。1.16.1のUSARTはPEIE/EIEを未実装tagとして報告するため、指定した2種類のwarningを各4回（Polling）/各3回（InterruptReceive）だけを照合して記録する。他のwarning/errorは失敗。parity/frame波形は未検証で、TCはモデル上即時になる。

ESP32-C3は`ESP32C3GpioTestSoC.cs`でGPIO/UART1/FIFO/TC・SysTimer・interrupt matrixの必要な部分をモデル化し、generic RV32のSRAMへnative ELFをロードする。TXは115200/8N1相当のword時間を経てTCを発生させる。ROM・Flash boot、実デバイスの全レジスタ、baud/divider・parityの波形、優先度競合・IRQネストを網羅しない。BSPはnative MMIOと通常のtrap vectorを実行する。

実機でのbaud誤差・stop bit・7/9bit・parity・高負荷overflow・割り込み遅延、非代表STM32F4 profileの通信は未確認。RTS/CTS等はUnsupportedとして拒否する。デジタルword注入による成功を、電気特性や全設定の確認済みという意味にはしない。
