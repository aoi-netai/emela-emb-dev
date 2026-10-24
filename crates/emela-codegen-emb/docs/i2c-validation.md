# I²Cの検証

2026-10-25。[操作契約・対応controller/pin・状態遷移](interface/i2c-interface.md)に沿って、stdlib、BoardI2c、生成C、共通driver、STM32F4/ESP32-C3 BSPを確認する。

## hostでの再現

```sh
cargo test --locked -p emela-codegen-emb
cargo test --locked -p emela-core
cargo test --locked -p emela-cli --test embedded_cli
```

I²C単独では`cargo test --locked -p emela-codegen-emb --test emit embedded_peripherals::i2c`を使う。C11・C++17のhost compilerとPython 3が必要。

| 試験 | 確認範囲 |
| --- | --- |
| 生成CとEmela | 専用エラー全分類・未知status、static ID/const/関数別名、private callback、scalar ABIからの3イベント、operation code変換、ISR内のGPIO write、ISRでのheap確保0 |
| 言語機能と共通driverの統合 | EmelaからNACK・Timeoutを`escape`で捕捉して次の通信を成功させ、ISR→固定queue→mainで4件の最終結果を取得。応答確認3試行は1通知、失敗時の受信prefixをList化しないこと |
| compiler拒否 | 動的ID、lambda、重複binding、同期I²C再入、heap、GPIO/UART/I²Cのqueue producer競合、main producer競合 |
| 共通driver | バス操作前のアドレス・data・size・memory・設定・timeout検証、空転送、設定維持、失敗後の未設定、容量、短い成功を返さないこと、最初の通信異常の保持、復旧状態、通知停止と旧pending排除 |
| 応答確認・期限 | 初回ACK、NACK後のACK、全試行NACKのFalse、通信異常、trials 0、負timeout、開始前timeout 0、全phase・全試行で共通の期限、最終NACK時の期限切れをFalseにしないこと、Int64上限 |
| 最終通知 | 検証・空転送・timeout 0では無通知、開始済み要求は1回、応答確認の全試行を1回に集約、同期結果との一致、callback終了後の復帰。hostのISR代替入口で確認 |
| STM32F4のレジスタモデル | 実BSPのC sourceを実行。7bit address、Bits8/16 prefix、Repeated START、1/2/3/4/32/33/64byte受信、ACK/POSと最終NACK、NACK時のSTOP、address/data NACKを再送しないこと、arbitration lost時のSTOP抑止、STOP失敗と再設定、100kHz/400kHzと16:9のregister設定 |
| ESP32-C3のレジスタモデル | 実BSPのC sourceを実行。FIFO32byte境界、command batch間のENDでbusを保持、Repeated START、最終byteだけの正常NACK、STOP、NACK/調停/後処理失敗と回復、16:9 timing、START前の期限切れで不要なSTOPを出さないこと、52bit時刻counterのwrap |
| C構文検査 | センサー使用例から生成したCと両BSP全translation unitを`-std=c11 -Wall -Wextra -Werror -fsyntax-only`で確認 |

共通driverとレジスタモデルはASan/UBSan付きで実行する。レジスタモデルではCPU assemblyと時刻・MMIOを置換する。STOP後のcallbackを本物のNVIC/RISC-V trapで実行したという意味ではない。共通bridgeの従来mockは16bit wordの移送も検査し、I²C固有の8bit検査とは分ける。

## CLIとfirmware build

[センサー使用例](../../../examples/i2c_sensor/README.md)を通常のCLIで検査・生成し、配布BSPでcross compile/linkしてfirmware imageを作る。実機への書込みは行わない。

```sh
cargo build --locked -p emela-cli
export ARM_TOOLCHAIN_PATH=/path/to/arm-none-eabi-toolchain
export EMELA_ESP32C3_TOOLCHAIN=/path/to/riscv32-esp-elf
export EMELA_ESPTOOL_PYTHON=/path/to/python-with-esptool-4.8.1
python3 crates/emela-codegen-emb/tools/i2c_build_matrix.py \
  --emela target/debug/emela \
  --output target/i2c-validation
```

既定はSTM32F407VG/F412ZG/F429ZI/F446RE・ESP32-C3 × O0/O2の10 case。生成C、board資産、compile/link log、firmware、resource reportとhashを出力し、総合`report.json`へまとめる。`--boards`・`--optimizations`で範囲を指定できる。host試験はcodegen 432件、core 60件（既存ignore 1件）、CLI 9件が合格。

## 今回の結果

STM32F407VG/F412ZG/F429ZI/F446RE・ESP32-C3 × O0/O2の10 caseすべてで、通常のCLIから検査・C生成・cross compile/link・firmware image生成が成功した。使用toolはArm GCC 13.2.1、Espressif GCC 14.2.0_20241119、esptool 4.8.1。ローカルの`target/i2c-validation-20261025/report.json`に全caseのhashと静的資源量を保存する。Flash実行や実機通信の確認とは区別する。

## 実機確認との境界

実機のSCL/SDA波形、pull-upと配線容量、clock精度、clock stretching、複数controllerの電気的調停、MCU errata、IRQ latency・優先度ネスト、実行時stack high-waterは未確認。レジスタモデルはdriverの手順と境界条件を確認するもので、実デバイスの全レジスタとtimingの証明にはしない。

実機では通常のSTART/Repeated START/STOPと最終NACK、100/400kHz、16:9、通信異常と失敗後の次の要求、ISRからのGPIO変更と同期結果の一致をlogic analyzer等で確認する。callbackと独自のISR専用FFIは有界に実行し、通知IRQの進行を妨げない構成を使う。
