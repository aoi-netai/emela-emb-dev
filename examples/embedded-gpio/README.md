# 組み込み C の GPIO 割り込み例

`src/main.emel` は GPIO effect で LED とボタンを初期化し、`button_changed` を割り込みハンドラーとして定義する例です。
`GpioPin(13)` と `GpioPin(2)` は論理 ID で、NUCLEO-F401RE の物理端子への対応はボード定義が持ちます。

設計上は、ビルド時バインドが論理ピン `2` の `Edge.Both` と `button_changed` を結び付けます。HAL コールバックから生成Cの静的な中継関数を経由して `button_changed` を直接呼ぶ想定です。実行時の関数ポインター登録や、`main` からの割り込みポーリングは使いません。

| 論理ピン | エッジ | Emela ハンドラー |
| --- | --- | --- |
| `GpioPin(2)` | `Edge.Both` | `button_changed` |

このサンプルは設計形を示すものです。割り込みハンドラーの指定記法、ボード定義との結び付け、ISR 到達範囲の検査、HAL コールバックの生成はまだ実装されていません。そのため、現在の `emela build` はこのハンドラーを HAL コールバックへ接続しません。

ハンドラーとそこから呼ぶ関数には、ヒープ操作（確保・解放・参照カウントを含む）やブロッキング処理を許可しない方針です。許可する effect と実行時間・スタックの制約は今後決めます。

関数合成と `Option.map` を使う最終形の疑似コードは [`docs/design/embedded-c.md`](../../docs/design/embedded-c.md#最終目標のコード例疑似コード) にあります。
