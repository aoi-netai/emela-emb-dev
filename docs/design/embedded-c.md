# 組み込み向け C ターゲット

この文書は、組み込み向け C バックエンドの作業場所と、共有済みの設計条件を記録する。具体的なランタイム方式、C 規格、言語機能の対応範囲はこれから決める。

## 共有されている前提

- 出力言語は C とする。
- 静的に決まる値や処理は、可能な範囲でコンパイル時に解決する。
- 通常の実行はシングルスレッドとする。
- 割り込みコンテキストから Emela のヒープに触れない。確保だけでなく、解放や参照カウント操作も対象に含める。
- 組み込み機能は Emela 側に専用インターフェースを定義し、アプリケーションからはそのインターフェース経由でのみ利用できるようにする。GPIO と GPIO 割り込みは対象例である。
- 初期に対応したい周辺機器は GPIO、UART、I2C、SPI とする。
- ボードバインディング時に、論理 ID の割り当て、周辺機器の対応能力、ピン競合などを静的に検査し、不整合は理由が分かるエラーとして報告する。
- 当面の参照環境は STM32F4 系と STM32CubeF4 HAL とする。最初のボード定義例は NUCLEO-F401RE（STM32F401RET6）とする。
- ボード固有の実装方法、外部関数の許可方法、ISR ハンドラーの記法と制約詳細は未決。

シングルスレッドでも割り込みは通常処理を中断する。共有状態の同期方法と ISR 安全性は別に定める。

## 現行 C バックエンドから分かっていること

現行 C バックエンドは、エントリから到達する関数だけを単相化し、Heap IR から C11 を生成する。文字列リテラル、引数のない構成子、捕獲のないクロージャは静的データになっている。自己末尾呼び出しも lowering でループ化される。

一方、現行の `const` は初回使用時に実行時評価される。フィールド付きの構成子、タプル、リスト、捕獲のあるクロージャ、継続フレームなども実行時に確保される。Heap IR には参照カウントの `dup` / `drop` が明示されている。

現行 C ランタイムは `malloc` / `free` に加え、fiber と外部の非同期処理のため POSIX のイベントループと pthread を使う。そのまま組み込み用にはできないため、組み込み用のランタイムとプラットフォーム境界を別に設計する。

## 参照する組み込み環境

最初のプラットフォーム実装は STM32F4 系の STM32Cube HAL を前提に検討する。生成 C と Emela の周辺機器インターフェースの間に C のボードバインディングを置き、HAL の型や関数をコード生成器へ直接持ち込まない。これにより、Emela 側の API と HAL 呼び出しの対応を個別ボードの設定として管理できる。

## 決定: GPIO 割り込みは HAL コールバックから静的に Emela ハンドラーへ接続する

GPIO 割り込みは、通常処理からのポーリングを基本経路にしない。ボードバインドが論理ピンと Emela の割り込みハンドラーを静的に結び付け、HAL コールバックから生成 C の中継関数を通してハンドラーを直接呼ぶ。実行時のコールバック登録や関数ポインター、クロージャーの生成は使わない。

この呼び出しは ISR 内で実行される。ハンドラーから到達する関数全体に ISR 制約を適用し、Emela ヒープの確保・解放・参照カウント操作を禁止する。ブロッキング処理や中断可能な処理も許可しない方針とする。許可する effect、実行時間・スタックの上限、共有状態の扱いは別途定義する。シングルスレッドでも ISR は通常処理を中断するため、共有可変状態の安全性を検査対象に含める。

固定容量キューは、応答を通常コンテキストへ明示的に遅らせる用途が必要になった場合の選択肢として残す。GPIO 割り込みの標準経路にはしない。既存の試作 API にある `poll_interrupt` はこの決定より前の案であり、最終インターフェースでは置き換える。

割り込みハンドラーの Emela 上の指定方法、ボードバインドの記述形式、HAL コールバックの所有者（ボード側か生成側か）は未決である。設計例は [`examples/embedded-gpio`](../../examples/embedded-gpio) を参照。

## 最終目標のコード例（疑似コード）

次の例では、GPIO 割り込みを静的に `button_changed` へ結び付ける。割り込みハンドラー内では `Option.map` で入力値を純粋変換し、最後に GPIO effect で LED を更新する。`bind` の行は将来のビルド時バインドを表す疑似記法で、構文は未確定である。

```emel
import Std.Embedded.Gpio.{GPIO, GpioPin, GpioEvent, GpioMode, Pull, PinState, Edge}

fn button_level(event: GpioEvent) -> Option[PinState] {
  if event.pin == GpioPin(2) {
    Some(event.level)
  } else {
    None
  }
}

fn is_pressed(level: PinState) -> Bool {
  level == PinState.Low
}

fn led_state(pressed: Bool) -> PinState {
  if pressed { PinState.High } else { PinState.Low }
}

fn button_changed(event: GpioEvent) use { GPIO } {
  gpio = use GPIO
  output = button_level(event)
    |> Option.map(is_pressed)
    |> Option.map(led_state)

  output match {
    Some(state) -> gpio.write(GpioPin(13), state)
    None -> ()
  }
}

fn main() use { GPIO } {
  gpio = use GPIO
  gpio.configure(GpioPin(13), GpioMode.OutputPushPull, Pull.NoPull)
  gpio.configure(GpioPin(2), GpioMode.Input, Pull.Up)
  gpio.enable_interrupt(GpioPin(2), Edge.Both)
}

bind GPIO.interrupt(GpioPin(2), Edge.Both) to button_changed
```

GPIO 操作を直接呼ぶ関数は、シグネチャに `use { GPIO }` を明記し、本体の先頭で `gpio = use GPIO` と一度だけ束縛する書き方にそろえる。

NUCLEO-F401RE のボード定義では `GpioPin(2)` は PA10、`GpioPin(13)` は PA5 に対応する。ビルド時にこの対応と handler を静的に解決し、HAL コールバックから生成 C の中継関数、Emela の `button_changed` の順に直接呼ぶ。

この例の `Option.map` は小さな値の変換であり、呼び出し先を静的に特定できる想定である。ISR で使えるようにするには `Option[PinState]` を inline の値として表し、変換中にヒープ操作を生成しないことをコンパイラーが保証する必要がある。保証できない場合は ISR 到達範囲での使用を拒否する。通常の `List.map` は結果リストの確保を伴うため ISR からは呼ばない。固定長コレクションへの map を許す場合は、要素数と出力領域を静的に定め、各要素の処理を含めて実行時間とヒープ不使用を検査する。

実際のハンドラーでは、`button_level` から `GPIO.write` までの到達範囲全体を ISR 制約の検査対象にする。ボード実装の `GPIO.write` も、割り込み中に安全に呼べる実装であることを確認する。

Emela 側の初期インターフェース宣言は `crates/emela-core/lib/std/embedded/` に置き、`Std.Embedded.Gpio`、`Std.Embedded.Uart`、`Std.Embedded.I2c`、`Std.Embedded.Spi` として同梱する。GPIO ピンは論理 ID を使い、物理ピンへの対応をボード定義に置く。ボード定義形式と NUCLEO-F401RE の初期例は [`board-definition.md`](board-definition.md) と [`boards/stm32f4/nucleo-f401re.toml`](../../boards/stm32f4/nucleo-f401re.toml) を参照。STM32F4 HAL との対応案は [`stm32f4-hal-interface-draft.md`](stm32f4-hal-interface-draft.md) を参照。宣言とボード定義例は記述したが、ボード定義の読み込み、C/HAL バインディング、コンパイル時バインディング検査は未実装である。

## IR の使い分け案

Flow IR で静的評価と ISR の呼び出し・型を調べ、その後 Heap IR に変換して C を出力する。Heap IR 化後にも ISR 到達関数に確保、`dup`、`drop` が残っていないことを確認する。静的評価の結果は Heap IR 化より前に静的データ参照へ置き換える必要がある。

```text
単相化
  → Flow IR
  → 静的評価・ターゲット制約の検査
  → Heap IR
  → ISR 到達範囲のヒープ操作を再検査
  → C 出力
```

## 作業用のファイル分割

初期のコード配置だけを用意する。各モジュールの責務は仮置きで、API や実装方式を確定するものではない。

```text
crates/emela-codegen-c-embedded/
├── NOTES.md
└── src/
    ├── lib.rs          # バックエンドの入口とパイプライン
    ├── check.rs        # ターゲット機能・ISR 制約の検査
    ├── const_eval.rs   # 静的に評価できる式の評価
    ├── emit.rs         # C の出力
    ├── names.rs        # C 識別子への変換
    └── runtime/        # 生成物に同梱するランタイム素材
```

## 後で決める項目

- GPIO、UART、I2C、SPI の各 API が提供する操作と共通のエラー表現
- UART / I2C / SPI の同期・非同期動作、タイムアウト、バッファーの所有権
- ボード定義の読み込みと検査の実装、診断の詳細。静的に分からない実行時設定も、HAL 呼び出し前に検査する。
- 対象とする追加の STM32F4 型番・ボード、STM32CubeF4 HAL の版、クロックや起動コードの設定方法
- 出力する C 規格と、コンパイラ・ボード設定の指定方法
- 静的評価の対象、評価不能な `const` の扱い、循環参照の診断
- 静的オブジェクトの出力形式と、動的確保が残る場合の固定容量 allocator
- Emela の組み込みインターフェースの宣言方法と、ボード実装を登録・制限する方法
- ISR ハンドラーを指定する Emela 記法と、ピン・エッジ・関数を結び付ける静的バインド形式
- ISR から呼べる関数の型・effect・外部関数の条件、実行時間・スタックの制約と検査段階
- ISR と通常処理で共有する状態の安全な表現。必要な場合に固定容量キューを使う API
- `suspend`、fiber、スケジューラを提供するか
- Unicode、浮動小数点、標準ライブラリなどの対応範囲
- OOM・defect の報告方法と、実行時 ABI
- CLI / driver への統合方法

## 既存仕様との関係

言語仕様 16.4 は値のメモリ表現と解放タイミングを実装依存としている。組み込み C ターゲットは表現を選べるが、整数の巻き戻りや文字列など、観測可能な言語上の意味は既存ターゲットと揃える必要がある。
