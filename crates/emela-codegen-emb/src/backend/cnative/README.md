# ホストCへの接続PoC

[link](mod.rs) は、ボード接続と同じ `CompiledModule` を受け取って共通C生成を呼ぶ。I2C・SPIの名前を解釈せず、一般的なC外部関数と `Options::with_external_adapter` / `with_task_adapter` の対応付けを利用する。

共通の生成Cに [platform.c](platform.c) を加え、debug出力とdefect hookをホストのstdio・abortへ接続する。従来runtimeとの互換性のためheader・hookのC名は `emela_board.h` / `emela_board_*` を維持する。

`main`がある場合、`emela_native_start()`を生成する。同期関数は呼び出し後に戻り、中断可能な関数は完了までschedulerを進めてから戻る。返り値の所有権を解放し、動的Stringの返り値には出力bufferを供給する。呼び出すC側の `int main(void)` は利用者が用意する。生成された `.c` をすべて同じビルドへ含める。

これは既存の `emela-codegen-c` targetとの統合や互換性を保証するものではない。共通のFlow IR backendをホストCから実行する検証用の接続経路で、library testでのみ利用できる。固定heapや各capacityは共通backendの設定を使う。

標準のplatform実装は単一thread向け。外部taskの完了通知も同じthread上で届ける。並行threadやsignal handler、OSとの待機連携が必要な場合は別のplatform実装が必要。ボード用ISR設定は入力エラーとして扱う。

実行例と検証は [separation_tests.rs](../tests/separation.rs) を参照。同じ共通生成物からボードのClock bindingとホストの `host_clock_now` へ接続を分け、`-O0` / `-O2` でホストCを実行する。
