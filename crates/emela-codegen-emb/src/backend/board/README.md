# ボードへの接続

[link](mod.rs) は共通コンパイル結果を受け取り、[LinkPreparation](../compiler/foundation/link.rs) を通じて、そのcopyへ接続用の型・関数を追加する。元のIRとmetadataへの可変参照は受け取らない。元の中間生成物を使い回してホストCへ接続できる。

| 場所 | 責任 |
| --- | --- |
| [gpio_irq.rs](gpio_irq.rs) / [uart_irq.rs](uart_irq.rs) / [i2c_irq.rs](i2c_irq.rs) | 静的bindingの検査、callbackの特殊化、登録引数の除去、通知用Cの生成 |
| [interrupt_validation.rs](interrupt_validation.rs) | ISRからの再帰・中断・heap利用・外部呼び出し等の検査 |
| [peripheral.rs](peripheral.rs) | 標準周辺機器bindingの認識、signature検証、header・adapter生成 |
| [spi_bridge.rs](spi_bridge.rs) | SPI操作列と固定ABIとの変換、受信Listの構築 |
| [peripheral_status.rs](peripheral_status.rs) | 共通statusと各周辺機器のerrorの対応 |

`link`は共通生成物に残された変換前IRからbindingを収集する。closureの持ち上げ後にlambdaを名前付きcallbackと誤認しないため。この収集自体は共通コンパイルの完了後に行う。

特殊化したcallbackを明示的なrootとして `finish(roots)` へ渡し、整合性を確認した後に共通C生成へ渡す。登録用callback引数の除去は、対象関数IDを共通APIに指定して行う。共通コンパイラーは内部の関数名prefixからISRかどうかを判断しない。ISR検査は共通の配置・heap解析を受け取ってこの層で行う。

接続用Cは最終C生成のadapter・補助コードの位置へ出力する。型配置やheap内部の関数を利用するため、今回は別translation unitへ独立させず、従来と同じ生成Cへ組み込む。これは確定済みC文字列の書き換えではない。

実機側のドライバ・startupは既存のcrate配下 `boards/` を利用する。このdirectoryは、それらへ接続するRustの生成処理を担当する。
