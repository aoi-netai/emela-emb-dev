# SPI・device・transactionの検証

2026-10-26。[SPI仕様](interface/spi-interface.md)の基本転送とCS付きtransactionを、stdlib・BoardSpi・生成C・共通driver・両BSPまで接続して確認する。

## hostでの再現

```sh
cargo test --locked -p emela-codegen-emb
cargo test --locked -p emela-core
cargo test --locked -p emela-cli --test embedded_cli
```

SPI単独は`cargo test --locked -p emela-codegen-emb --test emit embedded_peripherals::spi`。C11/C++17 compilerとPython 3を使う。

| 対象 | 確認内容 |
| --- | --- |
| Emela → bridge → 共通driver | enum/Listで操作列を作り、複数deviceへ送受信し、Read/Transferの結果だけを順序どおりのListへ返す。Writeなし結果、Read(0)/Transfer([])の空List、全ゼロ長・空操作列の無通信。終了後のheap使用量0 |
| 検証と容量 | 未初期化・不正handle、未設定、重複CS、device数4、負size、8bit範囲、共通word範囲、操作数・総TX数・総RX数の上限。CS選択前の拒否。生成容量1/65,535のC構文検査 |
| エラーと復旧 | 専用分類・未知status、最初のSPI/GPIO失敗とCS解除失敗の併存、解除だけの失敗、解除失敗後のInvalidState、configureで復旧しないこと、recoverで全CS解除、初期化失敗の予約を安全な回復後だけ解放 |
| 期限・CS | setup/holdの待機、全operationで共通の期限、最終clock後のCS解除、CSのactive Low/High、別deviceへの切替、引数エラー・timeout 0での無選択 |
| STM32F4 register model | 実BSPソースを実行。mode 0〜3、8/16bit、両bit順、0/255/65535、1/2/3/31/32/33/64/65 word、TXE/RXNEと最終BSY、dummy 0、transmitの受信値を次へ混入させないこと、全prescaler、ModeFault/Overrun/timeoutと復旧 |
| ESP32-C3 register model | 実BSPソースを実行。mode 0〜3、8/16bit、両bit順、64byte buffer境界、16bitの明示byte配置、dummy 0、TRANS_DONE後の完了、全prescaler、CS保持と最終clock後のhold、timeoutとreset |
| ISR検査 | GPIO ISRから同期SPIへの到達をコンパイル時に拒否 |
| 共通時刻 | UART/I²C/SPI/Clockの時刻取得をBSP内で共通化。I²Cの既存register modelでも52bit wrapと失敗後の再利用を回帰確認 |
| C翻訳単位 | 使用例から生成したCと両BSPを`-std=c11 -Wall -Wextra -Werror`で構文検査 |

生成Cと共通driverの統合試験はO2・ASan/UBSan付きで実行する。register modelもASan/UBSanを使い、CPU assemblyとMMIO・時刻源を置換する。sample edgeや電気的な波形の実測ではない。

O2のnative buildで見つかったpayloadなしenumの未初期化警告は、生成時にunion領域を初期化して解消した。空のoperation/TX配列はNULLと長さ0でBSPへ渡し、空配列を読み取らせない。native buildの未初期化警告は抑制していない。

## CLI・cross build

```sh
cargo build --locked -p emela-cli
export ARM_TOOLCHAIN_PATH=/path/to/arm-none-eabi-toolchain
export EMELA_ESP32C3_TOOLCHAIN=/path/to/riscv32-esp-elf
export EMELA_ESPTOOL_PYTHON=/path/to/python-with-esptool-4.8.1
python3 crates/emela-codegen-emb/tools/spi_build_matrix.py \
  --emela target/debug/emela \
  --output target/spi-validation
```

[2デバイスの使用例](../../../examples/spi_devices/README.md)を、通常のCLIでcheck・C生成・cross compile/link・firmware image生成する。既定はSTM32F407VG/F412ZG/F429ZI/F446RE・ESP32-C3 × O0/O2の10 case。Flashへの書込みは行わない。`--boards`・`--optimizations`で絞り込める。出力には生成C、BSP、build log、firmware、静的資源量とhashを含める。

今回はcodegen 437件、core 60件（既存ignore 1件）、CLI 9件が合格。firmwareも全10 caseが成功した。使用toolはArm GCC 13.2.1、Espressif GCC 14.2.0_20241119、esptool 4.8.1。ローカルの`target/spi-validation-20261026/report.json`へ結果とhashを保存する。共通時刻への切替後は、STM32F407VGとESP32-C3のI²C使用例もO2でbuild/linkを回帰確認し、`target/spi-i2c-regression-20261026/report.json`へ保存した。

## 実機確認との境界

実機のCPOL/CPHAとsample edge、bit順、SCK精度、MISOの入力遅延、接続先ごとのCS setup/hold、MCU errata、IRQ負荷とstack high-waterは未確認。SPI成功はcontroller側のclock完了であり、接続先の存在・受理やセンサの値の正しさを示さない。

実機ではlogic analyzer等で、2デバイス間のCS排他、最終clockとCS解除、mode 0〜3・8/16bit・両bit順を確認する。基本APIでCSを手動管理する場合は、アプリが再設定前の非選択と失敗時の解除を担当する。SPI APIはmainから同期的に呼び、任意のISRを含む最大実行時間やdefect時の通常cleanupまで保証するものではない。
