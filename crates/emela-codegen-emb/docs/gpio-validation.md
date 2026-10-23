# GPIOの実装・検証記録

2026-10-24、GPIOの出力初期値・専用エラー・静的ISR bindingを、通常のEmela frontendとCLIからfirmwareへ変換し、Renodeで検証した。利用するAPIとBSPの対応表は[GPIO仕様](interface/gpio-interface.md)を参照する。UART・I²C・SPIの新driverはこの改訂に含まない。

## 実装

- `GpioConfig`の出力variantに初期値を持たせ、出力を解放してからlatch・出力型・pullを設定し、駆動を有効にする。
- 全6operationとbindingは`GpioError`を返す。共通statusの数値・名称・分類は[単一定義表](../src/peripheral_status.rs)から生成する。未知の非0はOtherとする。
- GPIOのC ABIをstatus戻り値・readの出力pointerへ移行した。Cに内部enumのlayoutを公開しない。
- pin・const・不変な別名と名前付きcallbackを静的に解決する。private callbackを保持し、GPIO read/writeをISR専用driverへ特殊化する。
- 禁止operation、他effect、heap、再帰・末尾再帰、suspend、動的handlerや呼び出し、重複bindingを診断する。既存のImmediate制約とbackendの検査を使い、新しい言語構文は追加しない。
- mainのList/String・文字列付きエラーと、ISRの固定エラー捕捉を共存させた。関数値のconstを、その関数自体の呼び出し署名としてlowerしてしまうcoreの不具合も修正し、回帰試験を追加した。
- STM32F4はEXTI/SYSCFG/NVIC、C3はGPIO/matrix/CPU IRQ2からイベントを生成し、同じISR内でcallbackを直接呼ぶ。queueやfiberの起動は介さない。

## 試験アプリと完成条件

[共通ソース](../examples/support/gpio_interrupt.emel)の出力pin・Console予約pin・Analog対応をprofileごとに具体化する。GPIO・Clock・Consoleは通常のBoard handlerで選択し、callbackにはmainのhandlerを持ち込まない。ソース・Core・Flow・生成Cを手動で書き換える試験ではない。

| MCU / profile | controller / IRQ | 配線・設定 |
| --- | --- | --- |
| STM32F407VG / `stm32f407vg` | GPIOA/GPIOD、SYSCFG、EXTI6/7、NVIC IRQ23 | 入力PA6/PA7、出力PD12。Input Down、初期High、Both Normal → Rising High → Falling Low。PB6はEXTI6競合を拒否。 |
| ESP32-C3 / `esp32c3` | GPIO、matrix source16、CPU IRQ2 | 入力GPIO6/7、出力GPIO3。同じ設定遷移。AnalogはUnsupported、既存Inputを保持。 |

各構成は次を満たせば成功とする。

1. CLIの`check`・C生成・cross compile・link・firmware manifest検証が成功する。
2. 外部入力のedgeから実際のCPU trap/ISRへ入り、private callbackがenumをmatchし、別pinをread/writeする。mainではList/Stringと文字列付きエラーを利用する。
3. 共有IRQ上の2pinを識別し、同時pendingを両方処理する。片方の停止後も他方を配送する。
4. 停止中にhardware pending bitを残してから再有効化し、旧pendingを配送しない。Rising/Fallingに反するedgeと停止後のedgeではcallbackを呼ばない。
5. 不正pin、未設定、予約pin、未binding、割り込み有効中の再設定、共有IRQ優先度競合、STM32のEXTI入力競合、Analogの対応差、Inputへのwriteを確認する。検証失敗で既存設定を変更しない。
6. GPIO IRQは9回、各段階のevent数と外部出力の18チェックポイントが一致し、mainの結果42、defect 0、stack guard正常で終了する。

## 結果

通常のCLIでbuildした同一のアプリをO0/O2で確認した。固定heapは4,096 byte、fiber 4、frame 12 × 1,536 byte、task/event各2、stack予約16 KiBとguard 256 byte。system allocatorはlinkしていない。C3のguardは16 KiBの予約に含まれ、guardを除くstack容量は16,128 byteである。

| profile | 最適化 | 結果 | code / Flash byte | static RAM byte | stack観測 byte |
| --- | --- | --- | --- | --- | --- |
| stm32f407vg | O0 | 成功、IRQ 9 / checkpoint 18 | 51,040 | 24,784 | 3,300 |
| stm32f407vg | O2 | 成功、IRQ 9 / checkpoint 18 | 29,664 | 24,784 | 996 |
| esp32c3 | O0 | 成功、IRQ 9 / checkpoint 18 | 65,016 | 24,596 | 3,396 |
| esp32c3 | O2 | 成功、IRQ 9 / checkpoint 18 | 33,908 | 24,592 | 944 |

STM32のcode欄はFlash image byte、C3はSRAM上のtext/rodata byteである。C3の既存64 KiB IRAM・48 KiB DRAM・16 KiB stack配置を維持した。O0でもIRAM内へ収まるよう、同期の設定検査とmainの値処理を関数に分けた。stack値はこのアプリの観測結果であり、任意のアプリやISRネスト時の最大値ではない。`.su`とlink mapもfirmwareの出力に残る。

その他のSTM32F4 profileも、同じGPIO fixtureをO2で生成・build/linkして成功した。

| MCU / board profile | build/link | 実行の確認範囲 |
| --- | --- | --- |
| stm32f446（互換ID） | 成功 | この改訂のIRQシナリオは未実行 |
| stm32f407vg | 成功 | 上表のRenode O0/O2 |
| stm32f412zg | 成功 | この改訂のIRQシナリオは未実行 |
| stm32f429zi | 成功 | この改訂のIRQシナリオは未実行 |
| stm32f446re | 成功 | この改訂のIRQシナリオは未実行 |
| stm32f4-discovery | 成功 | この改訂のIRQシナリオは未実行 |
| nucleo-f446re | 成功 | この改訂のIRQシナリオは未実行 |

hostではconfigの全variant、status分類・未知status、alias/private callback、失敗時のoutを使わないこと、ISRのheap確保が増えないことを確認した。core・embedded codegen・CLIの関連回帰試験、Python toolsの21試験、clippyとformat検査も成功した。

## Renodeの確認境界

STM32は同梱STM32F4のGPIO/EXTI/NVICモデルにSYSCFG routingを追加し、Flash binaryからresetする。ネットワーク取得だけを行うApplySVD metadataを外し、Flash aliasとMCUのFlash/SRAM容量、reset HSIのSysTick周波数を設定する。BSPの割り込み処理・生成C・callbackは変更しない。

Renode 1.16.1のSTM32 GPIOはOTYPERのopen-drain動作を実装していない。PD12のOTYPER書き込みで出る既知の警告を正確に1件だけ記録し、電気的な解放・外部Lowとの競合を確認済みとはしない。他の警告やエラーは試験失敗とする。

ESP32-C3には[GPIO用テストSoCモデル](../tools/models/ESP32C3GpioTestSoC.cs)を追加した。native BSPのGPIO、pin matrix、IO_MUX、UART、1ms SysTimer、interrupt matrixの使用範囲をモデル化し、ELFのSRAM entryから既存startupを実行する。汎用RISC-Vコアのmie/mipへcontrollerのenable/pendingを対応付け、プラットフォームIRQ2を登録する。firmwareは引き続きnative MMIOとmtvec mode 1を使い、runnerによるISR関数呼び出し、PCをISRへ移すhook、CLIC modeへの変更は行わない。

このC3モデルは完全なMCUではない。ROM/Flash load、watchdog、アナログ電圧、回路のglitch、IRQ優先度の競合やネスト、物理的な応答時間を保証しない。従来の[QEMUによるROM/Flash確認](boards.md)や実機確認とは別の範囲として扱う。C3モデルのレジスタ配置は[ESP-IDF v5.4.1 GPIO定義](https://github.com/espressif/esp-idf/blob/v5.4.1/components/soc/esp32c3/register/soc/gpio_reg.h)と[interrupt定義](https://github.com/espressif/esp-idf/blob/v5.4.1/components/soc/esp32c3/include/soc/interrupts.h)に対応させた。

## 再実行

Rust workspaceのrootでCLIをbuildする。Arm GNU toolchain、Espressif GCC 14.2.0、esptool 4.8.1、Renode 1.16.1は既存の環境を使う。必要なpathは自分の環境に合わせて指定する。

```sh
cargo build --offline -p emela-cli
export ARM_TOOLCHAIN_PATH=/path/to/xpack-arm-none-eabi-gcc-13.2.1-1.1
export EMELA_ESP32C3_TOOLCHAIN=/path/to/riscv32-esp-elf
# esptool 4.8.1を利用できるPython。未指定なら実行中のPythonを使う。
export EMELA_ESPTOOL_PYTHON=/path/to/python

python3 crates/emela-codegen-emb/tools/gpio_matrix.py \
  --emela target/debug/emela \
  --renode /path/to/renode-1.16.1/renode \
  --output target/gpio-validation
```

[matrix runner](../tools/gpio_matrix.py)はprofileごとの`Pome.toml`・`embedded.toml`・`src/main.emel`を作り、通常のCLIでcheck/buildした後に[GPIO observer runner](../tools/gpio_probe.py)を起動する。既定は両BSP × O0/O2。`--boards stm32f407vg`や`--optimizations O2`で絞れる。各構成のproject・build・生成C・firmware・Renode/model log・reportと、全体の`report.json`を残す。出力directoryの所有markerとlockを使い、不明な既存directoryを上書きしない。失敗時には成功reportを維持せず、失敗した段階を記録する。

build済みfirmwareだけを再実行する場合：

```sh
python3 crates/emela-codegen-emb/tools/gpio_probe.py \
  --firmware-dir target/gpio-validation/stm32f407vg/O2/build/firmware \
  --renode /path/to/renode-1.16.1/renode \
  --output target/gpio-observed-f4
```

## 移行

旧`gpio.configure(pin, OutputPushPull, NoPull)`は`gpio.configure(pin, OutputPushPull(NoPull, High))`等へ変更し、`GpioError`を捕捉または宣言する。Input/Analogはpullだけを指定する。`enable_interrupt`には共通moduleの`InterruptPriority`を渡し、設定とbinding成功後に呼ぶ。

C実装は新しい生成headerに合わせ、全operationのstatus戻り値・readのout pointer・configureの初期値・ISR用read/write・binding検証を実装する。一般の外部FFIにenumを直接渡す変更は不要である。
