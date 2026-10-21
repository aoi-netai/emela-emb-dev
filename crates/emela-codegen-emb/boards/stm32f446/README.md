# STM32F446互換ボードID

従来の`stm32f446`は引き続き利用できる。STM32F446RE、NUCLEO-F446RE相当のUSART2／PA2・PA3配線、Flash 512 KiB、SRAM 128 KiB、HSI 16 MHzを使う。manifestと出力directoryのボードIDも`stm32f446`を維持する。

実装は[STM32F4共通package](../stm32/stm32f4/README.md)へ移した。`stm32f446re`はMCU向け、`nucleo-f446re`は基板向けの明示的な選択肢で、同じchip定義とruntimeを共有する。既存の`Pome.toml`の変更は不要である。

```sh
emela build . --board stm32f446
emela flash . --board stm32f446 --dry-run
```

`build.py`はcheckoutから直接呼ぶ場合の互換入口として残す。インストール済みCLIには選択profileと共通runtimeが平坦なBSPとして同梱され、開発checkoutは不要である。利用可能な標準API、Flash reset起動、OpenOCD書き込み、toolchain、資源と制約は[共通README](../stm32/stm32f4/README.md)を参照する。
