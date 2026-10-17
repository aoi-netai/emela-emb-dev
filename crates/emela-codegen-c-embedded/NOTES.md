# 組み込みCバックエンド

このcrateはヒープランタイムやPOSIX APIを使わず、対応範囲を絞ったCore IRをC11へ出力する。

## 現在の対応

- GPIO effect を `emela_gpio.h` のボード関数へ接続する。
- `GpioEvent` を引数に取る公開関数を割り込み入口として出力する。
- `Option[A]` を型ごとの固定サイズ構造体として出力する。
- 名前付き関数を渡す `Option.map` を、静的な直接呼び出し関数へ具体化する。
- 割り込み入口から到達する関数のeffect、呼び出し、ヒープを使う構文を制限する。
- ボードが呼び出す公開関数の宣言を `emela_embedded.h` に出力する。

## 未実装

- ボード定義の読み込みと論理ピンの静的解決。
- ピン、エッジ、Emela関数の宣言的バインドとHALコールバックの生成。
- ボード実装のISR安全性を検査する方法。
- GPIO以外の組み込みeffect。

詳細は [`docs/design/c-embedded-codegen.md`](../../docs/design/c-embedded-codegen.md) を参照。
