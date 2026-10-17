# 組み込み C の GPIO 割り込み例

`src/main.emel` は GPIO effect で LED とボタンを初期化し、`button_changed` を割り込みハンドラーとして定義する例です。`button_changed` は公開関数としてC出力に含まれ、`emela_embedded.h` にボード側から呼べる宣言が出ます。
`GpioPin(13)` と `GpioPin(2)` は論理 ID で、この動作確認では NUCLEO-F446RE の PA5（LED）と PC13（ボタン）に対応付けます。B1 は押下時 Low のため、入力をプルアップし、イベントの Low レベルを押下として扱います。

設計上は、ビルド時バインドが論理ピン `2` の `Edge.Both` と `button_changed` を結び付けます。HAL コールバックから生成Cの静的な中継関数を経由して `button_changed` を直接呼ぶ想定です。実行時の関数ポインター登録や、`main` からの割り込みポーリングは使いません。

| 論理ピン | エッジ | Emela ハンドラー |
| --- | --- | --- |
| `GpioPin(2)` | `Edge.Both` | `button_changed` |

`renode-f446/` には NUCLEO-F446RE 用の HAL アダプターと Renode 実行スクリプトがあります。`./examples/embedded-gpio/renode-f446/run.sh` で Emela C を生成して ELF にリンクし、PC13 の両エッジ割り込みから PA5 を操作する経路を確認できます。このアダプターは現在の実験用で、汎用 `emela build` によるボード別リンク生成にはまだ統合していません。

Windows 11 + WSL2 から NUCLEO-F446RE へ ELF を書き込む手順は [`renode-f446/README.md`](renode-f446/README.md) を参照してください。

ハンドラーから到達する関数には、失敗・中断・外部関数・間接呼び出し・組み込み関数・リスト・クロージャー・fiber等を許可しません。effect呼び出しは GPIO の `read`、`write`、`toggle` に限定します。ボード側の実装も ISR 内で安全かつ非ブロッキングにする必要があります。

`Option.map` を使った設計背景は [`docs/design/embedded-c.md`](../../docs/design/embedded-c.md#最終目標のコード例疑似コード) にあります。
