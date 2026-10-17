# 組み込みCバックエンドのアーキテクチャ見直し（案）

2026-10-17 時点の第0フェーズ（GPIO 割り込み例が Renode で動くところまで）の実装を調べ、このまま機能を足していけるかを検討したメモである。方針の提案であり、まだ決定ではない。現行実装の説明は [`c-embedded-codegen.md`](c-embedded-codegen.md)、前提条件は [`embedded-c.md`](embedded-c.md) を参照。

## 結論

- 外側の設計（effect によるインターフェース、Cのボード境界、論理ピンとボード定義、ISR の静的バインド）は維持する。
- 内側のコード生成（`emela-codegen-c-embedded/src/emit.rs` が Core IR から C を直接組み立てる方式）は、このままでは機能追加に耐えない。UART / I2C / SPI を足す前に、ホスト向け C と同じく `emela-lir`（Flow IR → Heap IR）を経由する形へ切り替える。
- 機能ごとに独自の展開を足すのではなく、値型化・静的特殊化・静的評価・静的確保・境界フックの5つの共通の仕組みにまとめる。
- 切り替えの最大の論点は、lir に固定サイズの値型（`Option[PinState]`、`GpioEvent` など）をヒープを使わずに表す表現を足すことである。

## 現状

```text
Core IR（単相化済み） → emit.rs → main.c / emela_gpio.h / emela_embedded.h
```

ホスト向け C（`emela-codegen-c`）は次の経路を通る。組み込み向けはこれを使っていない。

```text
Core IR → monomorphize_from → Flow IR（from_core） → Heap IR（to_heap: closure 変換、defect、状態機械、RC） → C
```

## 維持する部分

- 組み込み機能を `Std.Embedded.*` の effect として宣言し、ボード側が `emela_gpio_*` などの C 関数を実装する分担。
- `GpioPin(id)` を論理 ID とし、物理ピンはボード定義（TOML）に閉じ込める方針。
- GPIO 割り込みを HAL コールバックから静的に Emela 関数へ結び付け、関数ポインターや実行時登録を使わない方針。
- Renode で ELF を動かしてレジスターを確認する結合検証。

## 現行方式の問題

### Core IR から直接 C を書いている

Core IR は式の木で、C は文の並びである。`emit.rs` はこの差を `write_tail`（末尾）、`write_binding`（`let`）、`value`（式）の組み合わせで個別に埋めている。新しい構文を足すたびに3か所の組み合わせを考える必要があり、漏れがそのまま誤った出力になる。実際に、値が `()` の `let` で値が関数呼び出し以外のとき何も出力しない箇所があり、文として書いた `if` / `match` が黙って消える（付録A）。

Flow IR は ANF に結果変数（`dst`）と `yield` を持たせた形で、この変換をすでに解いている。パターン照合も Flow IR への変換で決定木にしている（現行の組み込み出力は `Option` の `match` しか扱えない）。評価順や整数演算の意味も Flow IR 側で他ターゲットとそろっている。

### 機能を名前で特別扱いしている

- `Option.map` を関数名で見分け、専用の直接呼び出し関数に置き換えている。同じ方法では `Option.unwrap_or`、`Result.map` などを関数ごとに足すことになる。
- GPIO の型を `ends_with(".Edge")` のような名前の末尾で判別し、境界ヘッダーの内容と enum の値を Rust の文字列で持っている。別モジュールのユーザー定義型を GPIO の型と取り違えうるうえ、`gpio.emel` のバリアント順を変えると黙って値がずれる。
- 周辺機器を足すたびに Rust の特別扱いが増える。

### ISR 検査の段階が設計とずれている

[`embedded-c.md`](embedded-c.md#ir-の使い分け案) は、Heap IR にした後で ISR 到達範囲に確保・`dup`・`drop` がないことを確かめる方針である。現行実装は Core IR の式の種類で判定しており、組み込み関数を一律に拒否する一方で、再帰（スタック上限が決まらない）は通している。

## 提案するパイプライン

```text
Core IR（単相化）
  → Flow IR                                   共有
  → ③ 静的評価                                 新規・共有
  → ② 静的特殊化                               新規・共有
  → ① 値型化                                   新規・共有
  → Heap IR                                   共有（値型には RC が付かない）
  → ④ 静的確保                                 新規
  → 組み込み制約の検査                           組み込み専用
  → 組み込み C 出力（⑤ 境界フック）               組み込み専用（codegen-c の writer / names の共有を検討）
```

①〜⑤は次節の共通の仕組みである。特殊化を値型化より前に置くのは、関数値と handler の証拠を消してから型を決めるためである（特殊化で handler の状態が具体的な型になり、値型化の対象になる）。

## 組み込み向け展開の共通の仕組み

第0フェーズでは `Option` を独自に展開した。同じやり方で機能ごとに展開を足すと、`Result`、レコード、handler、`const` などのたびに特別扱いが増える。組み込み向けに展開が要る機能の大半は、次の5つの仕組みにまとめられる。

| 仕組み | 対象になる主な機能 | 置き換える現行実装 |
| --- | --- | --- |
| ① 値型化 | `Option`、`Ordering`、引数付き enum、`type` のレコード、タプル、`Exit[A, E]`、error、handler の状態 | `EmelaOption_N` 構造体、`GpioEvent` の特別扱い |
| ② 静的特殊化 | 高階関数への静的な関数、捕獲のあるラムダ、ユーザー定義の handler、多相な R | `Option.map` の特別扱い、`use GPIO` の束縛の追跡 |
| ③ 静的評価 | `const`、フィールドを持つ構成子・リスト・文字列のリテラル | なし（現行は `const` 未対応） |
| ④ 静的確保 | 残ったヒープ確保（`List`、`String`、`Cell`、捕獲の残るクロージャ、fiber のフレーム） | なし（現行は型ごとに拒否） |
| ⑤ 境界フック | defect、`dbg`、`main` の待機、外部関数 | 空の `for (;;)` |

これらで lir の上の表現を組み込み向けに整えれば、制御の変換、パターン照合の決定木、自己末尾呼び出しのループ化、導出される等価関数、整数演算の意味などは lir の既存実装をそのまま使える。Trait は 0.21 で単相化の中で解決する予定（`emela-core/src/mono.rs`）なので、組み込み向けの作業は要らない。

### ① 値型化

lir の表現（`Repr`）は `I32 / I64 / F64 / Void / Ref` だけで、引数を持つ enum とタプルは `Ref`（ヒープ上の箱と参照カウント）になる。このままでは `Option[PinState]` も `GpioEvent` もヒープに載り、ISR から使えない。

値として持つ対象の条件案:

- 再帰しない（自分自身を直接・間接に含まない）。
- 型引数を置き換えた後に、すべてのフィールドが値として持てる型（整数、真偽値、値型の enum・タプル・レコード）である。
- `String`、`List`、関数値、`Cell` は対象外。`@external type` は C のポインター（`void *`）をそのまま値として持つ（「決定: 外部関数」）。

error は lir では構成子に引数がなくても必ず `Ref` である。別々の error の enum が混ざって届き、実行時に見分けるためである（`emela-lir/NOTES.md`）。組み込みでは、単相化の後に関数の `fails` の集合が決まるので、その集合ごとに「どの error か」のタグと各 error の値を持つ共用体を値として作る。`escape` の `is_error` はタグの比較になる。

実現方法の候補:

| 案 | 内容 | 長所 | 短所 |
| --- | --- | --- | --- |
| A. 値型の表現を lir に足す | `Repr` に値の集成型（レイアウト付き）を加え、構成子・フィールド読み・`switch` を値に対しても定義する | C では構造体にそのまま写る。ホスト C や WASM の性能改善にもなる | verify、Heap IR の各パス、codegen-c、codegen-wasm に手が入る |
| B. スカラーへの分解 | Flow IR 上で `Option[A]` を `(タグ, A)` の複数変数に分解する | 既存の `Repr` を変えずに済む | 引数・戻り値が複数値になる。C では構造体か出力引数が要る。入れ物の中の値には効かない |

案A を軸に検討する。既存バックエンドへの影響範囲を見積もってから決める。

### ② 静的特殊化

実行時に決まる呼び出し先を、コンパイル時に分かる範囲で直接呼び出しに置き換える。対象は3種類ある。

- **静的な関数引数:** 関数値の引数に静的に分かる関数（`Expr::Fn`）が渡される呼び出しについて、呼び出し先をその関数で特殊化した複製を作る。`Option.map` の特別扱いはこれで置き換える。
- **捕獲のあるラムダ:** 捕獲を追加引数にする（lambda lifting）ことで、静的な関数引数と同じ枠組みに載せる。初期は捕獲なしに限る。
- **handler:** lir の証拠の表は実行時に引く写像（`ev_lookup` / `ev_extend`）である。組み込みでは `with` ごとに handler を静的に決め、本体から到達する関数をその handler の組み合わせごとに複製して、操作の呼び出しを handler の関数への直接呼び出しにする。handler の状態（フィールド）は `with` の枠の構造体にし、`init` と `release` は `with` の入口と出口に展開する。layer はこの展開の前に handler の並びに直す。能力の持ち出しの禁止（仕様 8.6）により handler の値は `with` の外へ出ないので、静的に決められる範囲は広い。ボードが提供する effect（`GPIO`）は、handler がボード側の C 関数である特別な場合として扱う。

多相な R を持つ関数（`List.map` の `R(f)`）も、渡される関数の effect ごとに複製する。仕様 16.5 の中断用の写しと同じ考え方である。

特殊化しても残る間接呼び出し（実行時に選ぶ関数値、静的に決まらない handler）を、ISR の外では C の関数ポインターとして許すか、すべて拒否するかは未決とする。ISR 到達範囲では拒否する。

### ③ 静的評価

`const` の右辺は純粋（仕様 5.6）なので、コンパイル時に評価できる。仕様の意味は「最初の参照で1回だけ評価し、値と defect を覚える」である。これを実行時に実装すると初期化フラグの書き込みが要り、ISR と通常処理から同時に参照されたときに競合する。そこで組み込みではコンパイル時に評価し、結果を読み取り専用のデータとしてフラッシュに置く。

- 結果が値型なら C の定数の初期化子にする。`List` や `String` なら読み取り専用の静的オブジェクトにし、参照カウントの対象から外す。
- 評価が defect になった `const` は、参照したところでその defect を起こすコードに置き換える。仕様どおり、使っていない `const` の defect でビルドを失敗させない。
- 評価器は Flow IR の上の解釈器として作る（`const_eval`）。止まらない評価に備えて手数の上限を設け、超えたら診断にする。
- フィールドを持つ構成子、リスト、文字列のリテラルのうち、中身がすべて定数のものも同じ仕組みで静的データにする。現行の Heap IR は文字列のリテラルと引数のない構成子だけを静的データにしている。

### ④ 静的確保

①〜③の後も Heap IR に残る確保の扱いを決める。段階的に進める。

1. 初期: 残った確保をすべて拒否する。診断には確保の元になった式の位置を示す。
2. 寿命が関数の中で終わり、サイズが静的に決まる確保をスタックに置く。
3. 「決定: メモリ管理」の大きさごとの固定容量の領域に置く。容量は config で決め、足りなくなったら defect にする。

スタックや静的領域に置いた値の `dup` / `drop` は消す。fiber のフレームも、再帰がなければ単相化の後にサイズが決まるので、この仕組みで静的に置ける。ISR 到達範囲では、どの段階でも確保を拒否する。

### ⑤ 境界フック

ホスト C ではランタイムやOSが受け持つ処理を、ボード側が実装する C 関数として境界ヘッダーに宣言する。

| フック | 用途 |
| --- | --- |
| `emela_board_defect(kind, message, length)` | defect（`panic`、ゼロ除算、照合漏れ、負のシフト、Float から Int への変換の失敗、固定容量の不足）。戻らない（「決定: defect はその場で止める」） |
| `emela_board_idle()` | `main` の後の待機（WFI など） |
| `emela_board_debug(...)` | `dbg` の出力先。未実装のボードでは `dbg` を消す |
| `emela_board_console_write(bytes, length)` / `emela_board_console_error_write(bytes, length)` | Console の出力先（断片を書く）。「決定: platform の handler」 |
| `emela_board_now_milliseconds()` / `emela_board_sleep_milliseconds(ms)` | Clock の `now` と `sleep`。「決定: platform の handler」 |
| `emela_board_random_int()` / `emela_board_random_float()` | Random。「決定: platform の handler」 |
| `emela_board_critical_enter()` / `emela_board_critical_exit()` | 静的な State の排他（割り込み禁止の区間）。「決定: State.run と割り込みをまたぐ状態」 |

- defect は起きた場所でフックを呼んで止める。巻き戻しはしない（「決定: defect はその場で止める」）。
- 外部関数（`@external(embedded, ...)`）のプロトタイプも境界ヘッダーに出す。ISR から呼んでよいかは C 側の責任とする（「決定: 外部関数」）。
- C で未定義動作になる演算（`INT_MIN / -1`、範囲外のシフトなど）は、仕様の意味（巻き戻り、defect）を実装するヘルパーで出す。

## 組み込み専用の処理

### 組み込み制約の検査

Heap IR の上で行う。

- ターゲット全体: ヒープ確保・RC 操作が残っていないこと（動的確保を許す段階になれば、固定容量 allocator を前提に緩める）。
- ISR 到達範囲: 確保、`dup` / `drop`、間接呼び出し、中断、失敗、許可されていない effect 操作がないこと。呼び出しグラフの強連結成分で再帰を検出して拒否する。
- 論理ピン ID が静的に決まること（ボード定義を読む段階で、存在しない ID も拒否する）。

### ボード境界ヘッダーの生成

境界ヘッダーを Rust の文字列ではなく `Std.Embedded.*` の宣言から生成する。

- effect の操作 → `emela_<周辺機器>_<操作>` の C 宣言。引数と戻り値の型は値型化後の表現から決める。
- enum のバリアント → C の定数（`EMELA_GPIO_EDGE_BOTH` など）。生成コードも数値ではなくこの定数を使う。
- 組み込み型の判別は名前の末尾ではなく、定義のモジュールパス（`Std.Embedded.Gpio`）か定義 ID で行う。
- `Option` などアプリ固有の型は境界ヘッダー（`emela_gpio.h`）から外し、アプリ側ヘッダーに置く。名前は `TypeId` の番号ではなく型から安定して決まるものにする。

これにより、周辺機器の追加は主に `.emel` の宣言とボード側 C の実装で済むようにする。

## 合わせて見直す設計

### 周辺機器 API が `List` を使っている

UART / I2C / SPI の宣言は送受信データを `List[Int]` で受け渡ししており、ヒープを前提にしている。組み込みでそのまま出力できないため、コード生成より先に API の形を決める必要がある。候補は固定長のバイト列型、呼び出し側が所有するバッファーへの参照、長さを型やボード定義で静的に決める方法など。言語側の型の追加を伴う可能性がある。

`fails PeripheralError` を持つ操作の C ABI（戻り値でエラーコード、結果は出力引数など）も合わせて決める。

### ターゲットが提供する effect

型検査の `check_program_with_auto_main_effects` は、ドライバーが `Std.Embedded.Gpio` の `GPIO` を名前で探して `main` に与えている。effect が増えることを考え、ターゲットまたはボード定義が提供する effect の一覧を宣言し、それを型検査に渡す形にする。提供先は `main` だけでなく ISR の入口にも広げ、静的な State（「決定: State.run と割り込みをまたぐ状態」）もこの仕組みで与える。LSP にもプロジェクトのターゲットを渡し、エディター上の診断をビルドとそろえる。

### コンパイラのテスト

組み込み出力器そのもののテスト。利用者の `@test` の扱いは「決定: テスト（@test）」に書く。

1. 生成 C のスナップショットテスト（`insta`。codegen-c と同じ）。
2. 生成 C をホストの cc でスタブのボード（GPIO 呼び出しを記録するだけの実装）とリンクして実行し、期待する呼び出し列と比べる。純粋な部分は既存の JS / C 比較テストに乗せる。
3. Renode は結合テストとして残す。ツールのパスを環境変数で必須指定にし、未設定ならスキップする。

## 既存の言語機能のうち組み込み独自の実装が要るもの

組み込みと関係なく言語仕様にある機能のうち、共通の仕組み（①〜⑤）では足りず、組み込み向けに独自の実装が要るものを挙げる。ホスト向け C でランタイム（`emela-codegen-c/src/emela_runtime.c`）が受け持つ部分は、`malloc`、`pthread`、`poll`、`stdio` を前提にしているため、ほぼすべてが対象になる。

次のものは対象外とする。

- コンパイル時に処理が終わるもの: モジュール、`opaque`、型推論、Trait（0.21 で単相化の中で解決する予定）、`derive`、`layer` の並べ替え、網羅性の検査。
- lir と共通の仕組みで共有できるもの: `if`、`match`、自己末尾呼び出し、`fails` と `escape`（①）、`with` の `release`（lir の `protect` / `cleanup`）、整数演算の意味、導出される等価関数、`const`（③）。

「対応」の欄には、方針や担当、状況を書く。

| 機能 | ホスト C での実現 | 組み込みで必要なこと | 規模 | 対応 |
| --- | --- | --- | --- | --- |
| fiber / `suspend` / Async / Deferred / 中断（11章、14.5） | pthread と poll のイベントループ、手で数えるタスクと継続の記録 | 状態機械は lir のものを共有する。スケジューラ、待ち合わせ、中断の巻き戻し、外部の `suspend fn` の完了通知（ホストはイベントループ、組み込みは割り込み）は作り直す。`Fiber.scope` で起動する子の数が実行時に決まる場合は静的に確保できない | 大 | 後の段階。fiber は当面コンパイルエラー。`suspend` な外部関数とその場で `resume` する操作は、完了まで待つ形で許す（「決定: fiber は当面拒否し、suspend は待つ形で扱う」） |
| メモリ管理 | `malloc` / `free`、参照カウントの解放、終了時のリーク検査 | 参照カウントの仕組み（`em_dup` / `em_drop`、再帰しない `em_free`、カウント 0 の静的オブジェクトに書き込まない扱い）は流用できる。作り直すのは確保の部分で、`malloc` を大きさごとの固定容量の領域に置き換え（断片化と実行時間のばらつきを避ける）、容量不足を defect にする。オブジェクトの配置（ヘッダーと8バイトのスロット）は 32 ビットの MCU 向けに詰める余地がある。ISR がヒープの値に触れない限り、参照カウントは非アトミックのままでよい。`List` や `String` を ISR の外で許すなら必須 | 中 | 方針決定（「決定: メモリ管理」） |
| String の組み込み関数（16.2） | ランタイムで UTF-8 を処理し、結果を `malloc` で確保 | 確保しない関数（`contains`、`starts_with`、`ends_with`、`byte_size`、比較）は流用できる。確保する関数（`append`、`split`、`join`、`trim`、`chars`、`code_points`）はアロケーターに載せるか拒否する。書記素の判定はホスト C でも未対応（`CError`）なので同じ方針でよい | 中 | 方針決定（「String の表現」） |
| 数値と文字列の変換 | `Float.to_string` は `snprintf("%.*e")` で桁数を1から増やし、`strtod` で読み戻して一致した最短の桁を JS と同じ形に整える。`parse` は `strtod` を使う | デバッグ用途を想定し、ホスト向け C の実装（`em_format_float`、`em_float_parse`）を printf / strtod ごと流用する。出力はほかのターゲットと同じになる。newlib-nano では浮動小数点の printf を有効にするリンクの指定（`-u _printf_float`）が要り、フラッシュを数 KB 使う。最大17回の `snprintf` と `strtod` を繰り返すので遅く、newlib の printf は ISR から安全に呼べないため、ISR 到達範囲では拒否する | 小 | 方針決定: printf を使う（ホスト向け C の実装を流用） |
| Show / 文字列補間 / `dbg` / `panic` のメッセージ | 文字列を組み立てて stderr に出す | 文字列を組み立てず、出力先へ断片を順に書き出す方式にする。`dbg` は境界フック（⑤）に渡す | 中 | 方針決定（「String の表現」） |
| defect の値 | 伝播は Heap IR の `explicit_defects` で明示される（共有できる見込み）。値は実行時に組み立てる String（`assert` の比較の表示、`while evaluating const X:` の前置き、release 中の handler 名） | defect を種類のコードと静的なメッセージで表す。最上位の境界（ホストの終了コード 101）は境界フックにする | 小〜中 | 方針決定: その場で止める（「決定: defect はその場で止める」） |
| `State.run`（8.8） | 普通の handler ではなく、ランタイムが受け持つ handler（`RUNTIME_HANDLERS`）。状態はヒープ上の `Cell` に置き、証拠の表を伸ばす | ② の handler 特殊化に乗らないので専用の展開が要る。状態を `State.run` の枠に置き、`get` / `set` を直接読み書きする | 小 | 方針決定（「決定: State.run と割り込みをまたぐ状態」） |
| `Cause` / `Exit` / 後から起きた失敗の注記（11.5） | 表示用の文字列を組み立てる | `Cause` と `Exit` を作るのは fiber の操作（`join`、`await`、`attempt`）だけなので、fiber と一緒に拒否する。注記は起きない（`release` の注記は defect をその場で止めるので、fiber の注記は fiber がないので） | なし | 方針決定: fiber と一緒に当面はコンパイルエラー。注記は持たない（「決定: fiber は当面拒否し、suspend は待つ形で扱う」） |
| 外部関数と境界を越える型（12章） | 数値はそのまま、String は末尾 NUL の `const char *` で渡し、戻り値は C が `malloc` したものをランタイムが写す。`@external type` はヒープの箱に入れる。コールバックは未対応 | String を長さ付きで渡し、戻り値は呼ぶ側のバッファーに書かせる。`@external type` は値として持つ。注釈のターゲット名とプロトタイプの出し方を決める | 中 | 方針決定（「決定: 外部関数」） |
| platform の handler（Console、Clock、Random など） | std の `.c` / `.mjs` で実装する | std の外部関数に `@external(embedded, ...)` を足し、ボード側の C 関数に写す。Console は UART、Clock は SysTick やタイマー、Random は RNG 周辺機器など。Env、FileSystem、Http は提供しない | ボードごと | 方針決定（「決定: platform の handler」） |
| `@test` / `emela test` | ホストのプロセスを起動し、標準出力でやり取りする | ターゲット上では動かさず、GPIO などをテスト用の handler に差し替えてホストのターゲットで動かす | 不要にできる | 方針決定: ホスト向け C で動かす。エミュレーターでの実行は今後対応（「決定: テスト（@test）」） |

優先度の目安:

1. 最初のファームウェア: defect の値、`State.run`。
2. UART とログ: Show の逐次書き出し、数値と文字列の変換、Console の handler。
3. `List` / `String` を ISR の外で許す段階: メモリ管理、String の組み込み関数。
4. 最後: fiber / Async のスケジューラ（組み込み専用の設計とも絡むため）。それまではコンパイルエラーにする。

## 決定: String の表現

String は、Emela の書き方を変えずに、コンパイラの内部表現と config だけで扱う。容量を超えたときに defect になることを除き、意味はほかのターゲットと同じにする。

### 方針

組み込みで 512 バイトを超える String をスタックに置くこと自体が想定外である。扱う文字列の大半は、ログのメッセージのようなリテラルか、組み立ててすぐ出力するものである。HTTP の本文のような大きな文字列は例外だが、スコープを抜けたらすぐに破棄してよいものであり、String の容量を大きくして扱うものではない。

この前提で、String を固定容量のバッファーとしてスタックに置く。容量は config で決め、既定は 256 バイト、上限は 512 バイトとする。上限を超える値は設定の誤りとして診断する。

### 表現と受け渡し

| 項目 | 方針 |
| --- | --- |
| 表現 | 「静的な文字列」（フラッシュへのポインターと長さ）と「バッファーを持つ文字列」（容量 N の領域と長さ）の2種類を、タグで見分ける。リテラルはバッファーにコピーしない |
| 容量 | UTF-8 のバイト数で数える。超えたら defect（境界フック ⑤）にし、黙って切り詰めない |
| 引数 | `const` ポインターで渡す。値は不変で、呼び出し元の枠のほうが長生きするので安全である |
| 戻り値 | 呼び出し元が用意した領域に書き込ませる |
| コピー | 構造体を丸ごと代入せず、実際の長さの分だけコピーする |
| 出力操作に直接渡す補間 | `Console.print_line("x = #{x}")` のような場合は文字列を作らず、断片を順に書き出す。容量の制限を受けない |
| 長さの上限の推論 | リテラルの長さ、`Int.to_string` は11バイト、`append` は両方の和のように、上限が静的に分かる String はその長さで確保する。分からないものだけ config の容量を使う（後の段階） |

ISR から到達する範囲では、String の領域も最悪のスタック使用量に数える。

### config だけで使える範囲

| 区分 | 書き方 | 実現方法 |
| --- | --- | --- |
| 使える | リテラル、受け渡し、戻り値、`==`、文字列リテラルとの `match` | 静的な文字列、ポインター渡し |
| 使える | `contains` / `starts_with` / `ends_with` / `byte_size` | 確保しないのでそのまま実装する |
| 使える | `append`、文字列補間、`Int.to_string`、`Float.to_string`、`trim` | 容量 N のバッファーに書く。超えたら defect |
| 使える | 出力操作に直接渡す補間 | 断片を順に書き出す |
| 使える | `panic` のメッセージ、`dbg` | バッファーに書くか、境界フックへ逐次渡す |
| 使える（重い） | String を含むレコード、`Option[String]` | 1つあたり N バイト以上になる |
| List の方針次第 | `split`、`chars`、`join`、`concat`、`code_points` | List を「決定: メモリ管理」のヒープに置けば使える（ISR の外のみ）。`heap_bytes = 0` なら拒否する |
| フラッシュと引き換え | `String.length`（書記素の数）、空の区切りでの `split` | Unicode の表を入れるかを config で選ぶ。入れない場合はコンパイルエラーにする（ホスト C と同じ） |
| 扱わない | 512 バイトを超える文字列 | 後述の大きな文字列の扱いを参照 |

### config の案

```toml
[embedded.memory]
string_capacity = 256   # String 1つあたりのバイト数（UTF-8）。既定 256、上限 512。超えたら defect
heap_bytes = 0          # Emela のヒープの大きさ（「決定: メモリ管理」）。0 ならヒープを使えない
unicode_tables = false  # 書記素の判定に使う表を入れるか
```

### ホストでのテスト

容量を超えたときの defect は、ホストでテストしても起きないため、ターゲット上で初めて見つかるおそれがある。ホストのターゲットでも同じ `string_capacity` を検査するモードを用意し、`emela test` で容量の超過を見つけられるようにする。

### 大きな文字列（後の段階）

HTTP の本文など 512 バイトを超える文字列は、スコープを抜けると破棄される前提で、呼び出しのスタックとは別の作業領域に置く方法を検討する。スコープの出入りに合わせて確保と解放を行う LIFO の領域とし、書き方は変えない。スコープの外へ出ないことは、Heap IR の `drop` の位置から確かめる。

## 決定: defect はその場で止める

組み込みターゲットでは、defect が起きたらその場で境界フック `emela_board_defect` を呼び、戻らない。仕様（7.5、14.4）の巻き戻しは行わず、途中の `with` の `release` も実行しない。これは組み込みターゲットの制限として扱う。

### 理由

- 異常時に出力を安全な状態にする後始末（モーターを止めるなど）は、`release` よりも、どこで止まっても必ず通る `emela_board_defect` で行うほうが確実である。
- 当面の組み込みの handler はボードが提供する GPIO が中心で、`release` を持つ handler はほとんどない。
- 巻き戻すには、defect になりうる呼び出しのたびに確認の分岐が要り、コードが大きく遅くなる。趣味の規模では単純さとコードサイズを優先する。
- ホストでテストするときは仕様どおり巻き戻るので、テストの結果は変わらない。

### 方式

- 生成コードは、defect の発生源（`panic`、`todo`、ゼロ除算、照合漏れ、Float から Int への変換の失敗、負のシフト、容量の不足など）で直接フックを呼ぶ。
- defect の伝播の分岐は作らない。Heap IR の `explicit_defects` が作る伝播の辺を、組み込みでは使わない（または作らない設定にする。切り替え方法は実装時に確かめる）。
- defect を保存しておく変数（ホスト向け C の `em_defect`）、`release` の注記、const の名前の前置きは持たない。
- メッセージの文面は仕様の一部ではない（7.5）ので、種類のコードと静的なメッセージで表す。`panic(message)` だけは利用者の String をそのまま渡す。Float から Int への変換の失敗のように、ホストでは値を埋め込むメッセージも、組み込みでは静的な文面にする。
- ISR の中で起きた defect も、ISR の中でそのままフックを呼ぶ。ボード側のフックは ISR から呼ばれても動くように実装する（割り込みを止める、出力を安全な状態にする、止まるかリセットする）。

### フックの形（案）

```c
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length);
```

| 種類 | 発生源 | メッセージ |
| --- | --- | --- |
| `EMELA_DEFECT_PANIC` | `panic(message)` | 利用者の文字列 |
| `EMELA_DEFECT_TODO` | `todo()` | `not yet implemented` |
| `EMELA_DEFECT_DIVISION_BY_ZERO` | 整数のゼロ除算 | `division by zero` |
| `EMELA_DEFECT_NO_MATCH` | 照合漏れ | `unreachable: no match arm matched` |
| `EMELA_DEFECT_FLOAT_TO_INT` | Float から Int への変換の失敗 | `cannot convert Float to Int` |
| `EMELA_DEFECT_NEGATIVE_SHIFT` | 負のシフト量 | `negative shift amount` |
| `EMELA_DEFECT_HEAP_EXHAUSTED` | ヒープの不足（「決定: メモリ管理」） | `heap exhausted` |
| `EMELA_DEFECT_STRING_CAPACITY` | String の容量の超過（「決定: String の表現」） | `string capacity exceeded` |
| `EMELA_DEFECT_HANDLER_RELEASED` | release 後の handler の使用（8.6 の実行時の検査） | `handler was used after release` |
| `EMELA_DEFECT_ASSERT` | `assert` の失敗 | `assertion failed: <式の字面>`。字面はコンパイル時に分かるので静的な文字列にする。ホストで出す両辺の値は出さない |

`assert` はテスト関数の外でも使え、通常のビルドでも取り除かない（13.2）ので、組み込みでも出力する。

## 決定: State.run と割り込みをまたぐ状態

### State.run

`State.run` の状態は、ヒープ上の `Cell` ではなく、`State.run` を展開した関数のスタック上のローカル変数に置く。本体から呼ばれる関数にはその変数へのポインターを渡し、`get` / `set` / `update` は直接読み書きする。`State.run` は普通の handler ではない（`RUNTIME_HANDLERS`）ので、② の handler 特殊化とは別の専用の展開にする。状態の型が値型なら、ヒープも参照カウントも使わない。

### 割り込みをまたぐ状態: 静的な State

`State.run` の状態は本体の実行中にしか存在しない。`main` は初期化の後に戻り、ISR は呼ばれるたびに新しく始まるため、`State.run` では割り込みをまたいで状態を残すことも、`main` と ISR で状態を共有することもできない。

そこで、ターゲットが「プログラムが動いている間ずっと生きている `State[S]`」を提供する。状態の読み書きは既存の `State` の書き方のままで、新しい構文や型は足さない。足すのは初期値を指定する注釈だけである（注釈は `@test` や `@external` と同じ既存の仕組みなので、文法は変わらない）。

```emel
import Std.State.{State}

type Presses(n: Int)

@static                          # この const の値を初期値として、静的な State[Presses] を用意する（注釈の名前は仮）
const PRESSES = Presses(n: 0)

pub fn button_changed(event: GpioEvent) use { GPIO, State[Presses] } {
  gpio = use GPIO
  st = use State[Presses]
  st.update(fn(p) { Presses(n: p.n + 1) })     # 割り込みのたびに増え、次の割り込みまで残る
  if st.get().n >= 3 { gpio.write(GpioPin(13), High) } else { () }
}

fn main() use { State[Presses] } {
  ...                                           # main からも同じ状態を読み書きできる
}
```

#### 意味

- `@static` を付けた const の型 S について、静的領域に状態を1つ置き、その const の値で初期化する。const は ③ の静的評価でコンパイル時に計算する。
- ターゲットは、この `State[S]` を `main` と ISR の入口に提供する。「ターゲットが提供する effect」（現在は `GPIO` を `main` に与えている仕組み）を、ISR の入口と静的な State にも広げる。
- 状態は型で区別する（仕様 8.8 と同じく、`State[Presses]`、`State[Mode]` のように型で包む）。同じ型に `@static` の const が2つあればエラーにする。
- `@static` の const を持たない型の `State[S]` は、これまでどおり `State.run` で提供しなければならない。`main` や ISR の中の `State.run` が同じ型の状態を提供すれば、その内側では `State.run` のほうが使われる（最も内側の handler が使われる通常の規則）。

#### 制約

- 状態の型は値型（①）に限る。ISR はヒープに触れないので、`List` などヒープ上の値を持つ型はエラーにする。
- ホストのターゲット（JS、C）では、`@static` の const は普通の const として扱い、静的な State は提供しない。ホストでテストするときは、テスト関数の中で `State.run` で状態を与える。

#### 排他

- 共有状態を使う関数には `use { State[S] }` が付くので、コンパイラは ISR から到達する範囲がどの状態に触るかを effect から把握できる。
- ISR と `main` の両方から触られる状態の `get` / `set` / `update` は、割り込み禁止の区間で囲んで出力する。区間の出入りは境界フック `emela_board_critical_enter` / `emela_board_critical_exit` で行う（Cortex-M では PRIMASK を保存して戻す）。割り込みの優先度が入れ子になる場合に備え、ISR の中の操作も同じく囲む。
- `update(f)` は一息に実行される（仕様 11.1）ので、f の適用まで含めて1つの区間に入れる。f は純粋なので、区間の中で effect を呼ぶことはない。
- `x = st.get()` と `st.set(x + 1)` を別々に書くと、その間に割り込みが入って更新が失われうる。仕様が fiber について述べる注意（11.1）と同じ性質であり、`update` を使うよう案内する。後の段階で linter が警告する。

## 決定: fiber は当面拒否し、suspend は待つ形で扱う

fiber、Async による後からの再開、Deferred、fiber の取り消しは、後の段階で対応する。それまでは組み込みターゲットでコンパイルエラーにする。

`suspend` な外部関数と、その場で `resume` する handler の操作は、完了まで待つ形で許す（下の「完了まで待つ形で許す範囲」）。`Clock.sleep` を使えるようにするためである（「決定: platform の handler」）。

### 理由

- 現在の方式（`main` で初期化し、以降は ISR が処理する）で、最初のファームウェアは書ける。
- 対応にはスケジューラ、タイマー割り込みによる再開、fiber のフレームの確保、取り消し時の巻き戻しを組み込み向けに作る必要があり、独自実装の中で最も規模が大きい。
- 「点けて、待って、消す」のような1本の流れは、完了まで待つ `Clock.sleep` で書ける。fiber に対応する価値は、そうした流れを複数、状態機械を手で書かずに同時に進められることにある。タイマーや UART が揃った後に入れるのが自然な順番である。

### 完了まで待つ形で許す範囲

- `suspend` な外部関数は、完了まで待つ普通の C 関数として呼ぶ（`Clock.sleep` は `HAL_Delay` などになる）。fiber が1つしかないので、完了まで止まることと中断することに違いはない。止まっている間も割り込み（ISR）は動く。
- その場で `resume` する handler の操作（本体で `resume(v)` を直接呼ぶもの）も許す。`resume` はそのまま続きの処理になる。
- これらを含む関数は、lir の状態機械への変換（`state_machines`）を通さず、普通の関数として出力する。
- 将来 fiber に対応したときは、これらが本当の中断に変わる。Emela のコードは書き換えずに済む。

### 拒否する範囲

- `resume` を後で呼ぶ handler の操作（コールバックで再開するもの、`resume` を値として持ち回るもの）。
- ISR から到達する範囲の `suspend` な呼び出し（ISR の中で待つとシステムが止まるため）。
- fiber の操作（Core IR の `Expr::Fiber`。`Fiber.scope`、`fork`、`join`、`await`、`interrupt`、`attempt`、`Deferred` の操作、`Cause` の操作）。

`Cause` と `Exit`（11.5）は fiber の操作でしか作られないので、上の拒否でまとめて扱える。`attempt` は Async を要求せず fiber なしでも型が付くが、Die を値に変える働きは「決定: defect はその場で止める」と両立しないので、同じく拒否する。後から起きた失敗の注記（11.2、14.4）は、defect で巻き戻らず fiber もないので起きない。

診断は、どの関数のどの呼び出しが該当するかを示し、組み込みターゲットでは未対応であることを明示する。現在の `emit.rs` も失敗・中断する関数や fiber の式を拒否しているが、一般的な文面（「出力できません」）であり、ISR の外では理由が分かりにくい。

### 対応するときの方針（メモ）

- 状態機械への変換は lir（`state_machines`）を共有する。
- スケジューラは待ち行列と、タイマー割り込みで fiber を起こす仕組みで作り、実行できる fiber がなければ境界フックの `emela_board_idle()` で眠る。ホスト向け C の pthread と poll のイベントループは使わない。
- fiber のフレームは「決定: メモリ管理」のヒープか静的領域に置く。
- `Fiber.scope` で起動する子の数が実行時に決まる場合の扱いを決める。
- defect はその場で止まるので、Die の `Cause` は作られない。`attempt` が受けるのは Fail だけになり、`Cause` の種類は Fail と Interrupt に限られる。注記も持たない。

## 決定: メモリ管理

①〜④の後も残るヒープ上のオブジェクト（`List` の要素など）は、大きさごとの固定容量の領域に置く。`malloc` は使わない。

### 想定する用途

趣味の電子工作の規模を想定し、数か月にわたって止めずに動かし続けるファームウェアは対象にしない。長期間の運用で問題になる断片化への対策は、この決定の主な理由ではない。主な理由は、確保と解放が定数時間になること、実装が単純なこと、容量の不足がどこで起きたかを把握しやすいことである。

### 方式

- 参照カウントの仕組みはホスト向け C のランタイムのものを流用する（`em_dup` / `em_drop`、再帰しない `em_free`、カウント 0 の静的オブジェクトに書き込まない扱い）。変えるのは確保と解放の先だけである。
- Emela のオブジェクトは「ヘッダー＋スロット n 個」の形なので、大きさの種類はスロットの数で決まる。単相化の後に使われる大きさはコンパイル時にすべて分かるので、大きさごとに空きリストを生成する。
- 領域は config の `heap_bytes` で1つだけ確保する。各大きさの空きリストが空のときは、この領域の未使用部分から切り出す（先頭から順に進めるだけ）。解放したオブジェクトは、その大きさの空きリストに戻す。
- 一度ある大きさに切り出した部分は、ほかの大きさには使い回さない。大きさごとの容量を利用者が見積もらなくて済む代わりに、使い方が途中で大きく変わるプログラムでは無駄が出る。趣味の規模ではこれを許容する。
- 未使用部分も空きリストも尽きたら、容量の不足として defect（境界フック ⑤）にする。
- ISR はヒープに触れないので、参照カウントの増減はアトミックでない操作のままにする。静的な State（「決定: State.run と割り込みをまたぐ状態」）は値型に限るので、ISR と通常処理でヒープの値を共有することはない。

### config

```toml
[embedded.memory]
heap_bytes = 0   # Emela のヒープの大きさ。0 ならヒープを使えず、確保が残るプログラムはコンパイルエラーにする
```

`heap_bytes = 0` を既定にし、ヒープを使わないプログラムではアロケーター自体を出力しない。

### 後の段階

- オブジェクトの配置を 32 ビットの MCU 向けに詰める。現在はヘッダーが 12 バイト（32 ビット環境）、スロットは `Int64` と `Float` を入れるためにすべて 8 バイトで、`List[Int]` の要素1つが 28 バイトになる。
- 大きさごとの最大使用数を記録するデバッグ用の機能。`heap_bytes` の見積もりに使う。

## 決定: 外部関数

外部関数（12章）を、組み込み専用のターゲット名 `embedded` で宣言できるようにする。ベンダーの HAL、センサーのドライバー、利用者の書いた C を呼ぶ手段であり、`Std.Embedded` にない周辺機器を使う唯一の方法になる。呼べる場所の規則（`use` を省略した外部関数はハンドラの中だけ。12.2）は変えない。

```emel
effect Thermometer {
  fn read_celsius() -> Float
}

@external(embedded, "bme280_read_celsius")
fn bme280_read_celsius() -> Float

handler Bme280 implements Thermometer {
  fn read_celsius() { bme280_read_celsius() }
}
```

### ターゲット名

- `@external(c, ...)` とは共有せず、`embedded` を足す。String の渡し方がホスト向け C と違う（ホストは末尾 NUL と `malloc`、組み込みは長さ付きと呼ぶ側のバッファー）ので、ホスト向けに書いた C を組み込みで読むと、型の合わない関数が黙ってリンクされうるためである。
- 数値だけを受け渡す関数でも注釈を2つ書くことになるが、これは許容する。
- 変更は `@external` のターゲット名の許可リスト（`emela-syntax` の `EXTERNAL_TARGETS`）に1語足すことで、Emela の書き方は変わらない。

### 型の渡し方

| 型 | C での形 |
| --- | --- |
| Int / Int64 / Float / Bool / `()` | `int32_t` / `int64_t` / `double` / `bool` / `void` |
| String（引数） | `const char *text, uint16_t length`。末尾 NUL も保証する（フラッシュのリテラルにもバッファーにも1バイト余分に取る）ので、`strlen` を前提にする C のライブラリにもそのまま渡せる |
| String（戻り値） | 呼ぶ側がバッファーを渡す: `uint16_t f(..., char *out, uint16_t capacity)`。C は書いた長さを返す。容量を超えた長さが返ったら defect（`STRING_CAPACITY`） |
| `@external type` | `void *` をそのまま値として持つ（① 値型化の対象。ヒープの箱に入れない） |
| コールバック | 当面は拒否する（ホスト向け C でも未対応） |
| `suspend fn` | 完了まで待つ普通の C 関数として呼ぶ（「決定: fiber は当面拒否し、suspend は待つ形で扱う」）。ISR からは呼べない |

- 例外（12.5）は C にないので扱わない。C 側で止めたいときは `emela_board_defect` を直接呼ぶ。

### 宣言と C のファイル

- 外部関数のプロトタイプを、生成する境界ヘッダーに出す。C 側がそのヘッダーを include すれば、宣言と実装の型の食い違いを C コンパイラが検出する（ホスト向け C にはこの検査がない）。
- ファイルを指す形（`@external(embedded, "./sensor.c", "name")`）は、そのファイルを出力先に写す。名前だけの形は、ボード側のソースが実装するものとする。

### ISR からの呼び出し

- ISR から外部関数を呼ぶことを許可し、ISR から安全に呼べるかは C 側の責任とする。GPIO の境界ヘッダーが「割り込み中に呼ぶ操作は ISR 安全に実装してください」としているのと同じ扱いである。
- String は ISR で使えないので、String を受け渡す外部関数は既存の制約で ISR から拒否される。
- `@isr_safe` のような印は付けない。書き方が増えるわりに、宣言の正しさをコンパイラが検証できない（12.2 の `use {}` と同じ）ためである。

## 決定: platform の handler

Console、Clock、Random は、ホストと同じ `with Platform { ... }`（または `with SystemConsole` など）で供給する。組み込み専用の layer や書き方は足さない。std の外部関数に `@external(embedded, ...)` を足し、ボード側の C 関数（境界ヘッダーに出す）に写す（「決定: 外部関数」）。

```emel
fn blink(n: Int) -> () use { GPIO, Clock, Console } {
  (use GPIO).toggle(GpioPin(13))
  (use Console).print_line("toggle #{n}")
  (use Clock).sleep(500)
  blink(n + 1)               # 自分への末尾呼び出し（スタックを使わない。仕様 6.8）
}

fn main() use { GPIO } {
  with Platform { blink(0) }
}
```

この例は現在の実装で型検査を通り、エラーは `console_log` と `sleep_milliseconds` に組み込み用の `@external` がないこと（E0412）だけである。E0412 の検査は単相化の後に呼ばれる操作だけを見るので、使わない Http や FileSystem はエラーにならない。

### effect ごとの実装

| effect | 組み込みでの実装 | ボード側の C 関数 |
| --- | --- | --- |
| Console | 断片を順に書き出す関数を呼び、最後の改行はコンパイラが書く | `void emela_board_console_write(const char *bytes, uint16_t length)`、`void emela_board_console_error_write(const char *bytes, uint16_t length)` |
| Clock の `now` | ボードが返すミリ秒 | `int64_t emela_board_now_milliseconds(void)` |
| Clock の `sleep` | 完了まで待つ（「決定: fiber は当面拒否し、suspend は待つ形で扱う」） | `void emela_board_sleep_milliseconds(int32_t ms)` |
| Random | ボードが返す値 | `int32_t emela_board_random_int(void)`、`double emela_board_random_float(void)` |
| Env、FileSystem、Http | 提供しない。呼べばコンパイル時に E0412 になる | — |

- Console: 「決定: String の表現」の、出力操作に直接渡す補間は文字列を作らずに断片を順に書き出す方針を実現するため、Console だけはコンパイラが断片ごとに C 関数を呼び、最後に改行を書く。C 側は1行を受け取る関数ではなく、断片を書く関数として実装する。この特別扱いは名前ではなく定義（`Std.Console`）で判別する。多くのボードでは、2つの関数を同じ UART に出す。
- Clock の `now`: 仕様では Instant は Unix エポックからのミリ秒だが、マイコンの多くは時計が合っていない。起点はボードが決める（RTC が合っていればエポックから、なければ起動から）。差（経過時間）はどちらでも正しく求められる。
- Random: 仕様は暗号論的に安全な乱数を求めるが、組み込みでは質をボードに任せる（RNG 周辺機器があればそれを使い、なければ擬似乱数）。
- ISR: platform の handler は `main` の `with` で供給するので、ISR では使えない。ISR に渡るのはターゲットが提供する effect（GPIO、静的な State）だけである。
- E0412 の検査は、現在ビルドのターゲット名（`c-embedded`）で注釈を探している。注釈の名前 `embedded`（「決定: 外部関数」）で探すように合わせる。

## 決定: テスト（@test）

`c-embedded` のプロジェクトでも、`emela test` はホスト向け C のターゲットでテストを動かす。ターゲット上やエミュレーターでは動かさない（エミュレーターでの実行は今後対応する）。

```emel
handler TestGpio implements GPIO {
  fn configure(pin, mode, pull) { () }
  fn read(pin) { Low }
  fn write(pin, state) {
    (use State[List[PinState]]).update(fn(xs) { List.append(xs, [state]) })
  }
  fn toggle(pin) { () }
  fn enable_interrupt(pin, edge) { () }
  fn disable_interrupt(pin) { () }
}

@test
fn lights_when_pressed() {
  (_, writes) = State.run([], fn() {
    with TestGpio { light_if_pressed(GpioPin(1), GpioPin(13)) }
  })
  assert writes == [High]
}
```

### 方式

- GPIO などのターゲットが提供する effect は、テストでは利用者が書くテスト用の handler に差し替える（13章のとおり、モックのための仕組みは持たない）。`Std.Embedded.*` の型と effect は、ホストのターゲットからも import できる（確認済み）。
- ISR の入口は普通の関数なので、テストから `GpioEvent` などを作って直接呼べる。
- 静的な State は、ホストでは普通の const になる（「決定: State.run と割り込みをまたぐ状態」）ので、テストでは `State.run` で状態を与える。
- `string_capacity` の検査モードを有効にして動かし、容量の超過をテストで見つけられるようにする（「決定: String の表現」の「ホストでのテスト」）。
- 確かめられるのはアプリの論理と String の容量までで、組み込み向けの C の出力、`heap_bytes` の不足、スタックの深さはホストのテストでは分からない。

### 前提となる修正

- `handler TestGpio implements GPIO { ... }` が構文エラー（E0110 `expected type name`）になり、大文字だけの名前の effect の handler を書けない（付録A）。テストで GPIO を差し替える前提なので、先に直す。

### エミュレーターでの実行（今後対応）

- テストを入れたファームウェアを Renode で動かし、Console（UART）に出した結果をランナーが読む形を想定する。
- defect はその場で止まる（「決定: defect はその場で止める」）ので、失敗したテストの後はランナーがエミュレーターを起動し直し、次のテストから再開する仕組みが要る。
- arm-none-eabi-gcc、Renode、ボード定義が必要になり、起動に数秒かかる。テストの論理はホストと同じなので、増えるのは組み込み向けの出力の確認である。まずはコンパイラのテスト（例のプログラムを Renode で動かす回帰テスト）から始め、利用者向けの機能はその後に検討する。

## 移行手順案

1. 現行の GPIO 例の出力をスナップショットテストで固定し、付録Aの不具合を直す。以降の作業の回帰基準にする。
2. ① 値型化の方式（案A / 案B）を決め、lir に実装する。既存の codegen-c / codegen-wasm のテストが通ることを確認する。
3. ② 静的特殊化のうち、静的な関数引数を実装する。
4. Heap IR から組み込み C を出す出力器を作り、⑤ 境界フック（defect、待機）とともに、GPIO 例を同等の C で再現できたら `emit.rs` を置き換える。④ 静的確保は初期段階（残った確保をすべて拒否）で入れる。ISR 検査も Heap IR へ移す。fiber は専用の診断で拒否し、`suspend` な外部関数とその場で `resume` する操作は完了まで待つ形で出す（「決定: fiber は当面拒否し、suspend は待つ形で扱う」）。
5. 境界ヘッダーを `.emel` の宣言から生成する。外部関数（`@external(embedded, ...)`）のプロトタイプも同じヘッダーに出す（「決定: 外部関数」）。
6. ② 静的特殊化を handler に広げ、③ 静的評価で `const` に対応する。`State.run` の組み込み展開と、静的な State（排他を含む）を実装する。
7. String を「決定: String の表現」の方針で実装する。まず config だけで使える範囲（List を使わない関数と、出力操作への逐次書き出し）から行い、ホストでの容量検査モードも合わせて入れる。`c-embedded` のプロジェクトの `emela test` を、検査モードを有効にしたホスト向け C で動かす（「決定: テスト（@test）」）。
8. ボード定義の読み込みと、ピン・エッジ・ハンドラーの `bind` 記法を実装する（HAL コールバックの生成を含む）。
9. 周辺機器 API の形を決め、UART から順に対応する。Console、Clock、Random の外部関数に `@external(embedded, ...)` を足し、Console の断片の書き出しを実装する（「決定: platform の handler」）。

`emit.rs` は約1300行で、作り直しの損失は小さい。周辺機器を足した後では置き換えの範囲が大きくなるため、手順2〜4を先に行う。

## 未決事項

- 値型化の方式と、対象にする型の条件の細部。
- 特殊化後も残る間接呼び出しを、ISR 外でも拒否するか、C の関数ポインターとして許すか。
- 境界フックの名前と引数の確定（`emela_board_defect` の案は「決定: defect はその場で止める」）。
- String の長さを持つ整数の型（容量の上限が 512 バイトなら `uint16_t` で足りる）と、静的な文字列とバッファーを持つ文字列を見分けるタグの配置。
- `string_capacity` などの設定を置く場所（プロジェクトの設定かボード定義か）。
- 静的な State の初期値を指定する注釈の名前と形（仮に `@static`）。
- 組み込み出力器を独立 crate のままにするか、codegen-c にプロファイル（ホスト / フリースタンディング）として統合するか。

## 付録A: 第0フェーズの調査で見つかった不具合

- 値が `()` の `let` で値が関数呼び出し以外のとき、何も出力しない（`emit.rs` の `write_binding`）。関数の途中に文として書いた `if` / `match` が、中の GPIO 操作ごと消える。
- `emela-syntax` のスナップショットテスト `parser::grammar::tests::型` が失敗している。大文字だけの effect 名を許した変更で、effect 集合の中の型変数（`use R` の `R`）が `TYPE_VAR` ではなく `PATH` として読まれるようになった。効果多相のプログラムは JS で正しく動くことを確認したが、構文木の形が変わっているため fmt と LSP への影響を確かめる。
- ISR から到達する関数の再帰を拒否しない。
- 実行時に決まるピン ID（`GpioPin(event.pin.id + 1)`）を受け付ける。
- `c_type` が `Emitter` のメソッドと自由関数で重複している。
- `handler TestGpio implements GPIO { ... }` が構文エラー（E0110 `expected type name`）になる。大文字だけの effect 名を許した変更が `implements` の後に効いておらず、GPIO の handler を利用者が書けない。テストで GPIO を差し替えられない（「決定: テスト（@test）」）。

## 付録B: 文書の食い違い

- [`embedded-c.md`](embedded-c.md#作業用のファイル分割) のファイル分割（`check.rs`、`const_eval.rs`、`names.rs`）は削除済みで、現在は `emit.rs` だけである。
- `emela-lir/NOTES.md` は「driver と backend にはまだつないでいない」としているが、codegen-c はすでに Flow IR と Heap IR を使っている。
- 仕様 12.1 は `@external` のターゲットを `js` と `wasm` としている（E0140）が、実装はすでに `c` も受け付けている。`embedded` を足すときに仕様も合わせて直す。
- ボード定義の例は NUCLEO-F401RE（`GpioPin(2)` = PA10）、動作確認は NUCLEO-F446RE（`GpioPin(2)` = PC13、`board.c` に手書き）で、対応表が異なる。
