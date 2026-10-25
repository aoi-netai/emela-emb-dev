# Flow IRの機能一覧

組み込みC backendへ渡すFlow IRの機能と、入力を生成するときの条件をまとめる。ここでのFlow IRは `emela_lir::Module` の `Stage::Flow` を指す。定義は [`ir.rs`](../../emela-lir/src/ir.rs)、値の表現は [`repr.rs`](../../emela-lir/src/repr.rs) が正となる。

これは手書きの解説である。[定義から自動抽出した一覧](generated/flow-ir-inventory.md)と[生成コマンド・照合フォーマット案](inventory-format.md)を別に用意している。自動抽出一覧は宣言の記録であり、Flowへの入力可否やbackendの実装対応は判定しない。

以下のFlow用操作には、このbackendの生成処理がある。個々の制約・実装箇所・回帰試験は [実装範囲と対応状況](implementation.md) を参照する。正常なFlowへの対応と、任意のEmelaソースをそのFlowへ変換できることは別の条件である。

## コンパイル経路

```text
Core IR ── 単相化・from_core ──→ Flow IR ── compile_flow ──→ 組み込みC
                                  ↑
                         Flow producerが直接構築
```

`compile_flow(&flow, &options)` はFlowの型を正規化・検証し、Cソースとヘッダーを返す。入力はRustの構造体で、dump文字列を読み込むAPIではない。`emela_lir::to_heap` は呼ばず、closure変換、メモリ配置、所有権、中断・再開の継続はbackend内で処理する。

`flow_from_core` はCoreからFlowを作り、`compile` はその結果を `compile_flow` へ渡す便宜APIである。Core便宜APIにある成長する再帰型などの制約は、[入力境界の説明](implementation.md#core便宜apiとsourceの制約)を参照する。

## ソースの機能との対応

| 機能 | Flow IRでの表現 |
| --- | --- |
| ローカル束縛・逐次評価 | `Stmt::Let` と `Seq` |
| 算術・比較・論理反転 | `Value::Prim`。短絡評価は `Stmt::If` |
| 条件分岐・パターンマッチ | `Stmt::If` / `Stmt::Switch` とフィールド・Listの読み取り |
| ループ・末尾再帰 | `Stmt::Loop` / `Term::Recur`。一般の再帰・相互再帰は関数呼び出し |
| レコード・enum・tuple・List | `Value::Make` / `Value::Tuple` / `Value::List` と射影 |
| 関数値・クロージャ | `Value::FnRef` / `Value::Closure` と間接呼び出し |
| 型付きエラー・catch・escape | `Term::Fail`、呼び出しの `OnFail`、`Stmt::Block`、errorの型判定 |
| effect・handler・with | evidence表、handler値、operation呼び出し、`Stmt::Protect` |
| Stateの共有状態 | Cellの生成・読み取り・更新 |
| suspend・Fiber・Deferred・Cause | 呼び出しの `suspends` と `Value::Fiber` |
| const | `Function::constant` の付いた関数の参照・呼び出し。静的評価または初回評価cache |
| trait・derive・汎用関数 | 単相化・静的dispatch後の関数呼び出し。Flowにtrait選択や汎用関数の専用命令はない |
| 外部関数・不透明handle | `Function::external` とCore側のexternal型定義 |

`List.map` などEmela自身で書ける標準関数は、通常の関数・ループ・クロージャとして現れる。標準ライブラリの関数名ごとにFlow命令があるわけではない。

## データ構造と値の表現

`Module` はCoreの型・enum・effect定義とLIRの関数群を持つ。型は `TypeId`、関数は `FnId` で参照する。各 `Function` は変数・文・文列・ラベルのarenaを持ち、それぞれ `VarId` / `StmtId` / `SeqId` / `LabelId` で接続する。

- `Seq` は文の列と終端 `Term` 一つからなる。引数は変数か定数の `Operand` で、評価が必要な式を直接入れ子にしない（ANF）。
- 変数は引数・文の結果・loop変数・closure引数として一度だけ定義する。`VarData` にCore型と `Repr` の両方を記録する。
- compound文の `dst` には各経路の `Term::Yield` が値を渡す。結果が不要なら `dst: None` とするが、評価の副作用やdefectは残る。

| Flowの表現 | Core型の例 | 組み込みbackendでの扱い |
| --- | --- | --- |
| `Repr::I32` | `Int`、`Bool`、payloadもexternal属性もない通常の非空enum | 32 bit整数・tag |
| `Repr::I64` | `Int64` | 64 bit整数 |
| `Repr::F64` | `Float` | binary64の `double` |
| `Repr::Ref` | String、List、tuple、payload付きenum、error、関数、handler、evidence、Cell、Any、external型など | Core型から具体的な配置を決める。inline構造体・String descriptor・固定heap上の参照など |
| `Repr::Void` | `()`、`Never` | 通常の変数・引数・戻り値を作らない |

`Repr::Ref` は、組み込みCで必ずheap pointerになるという指定ではない。tupleやenumはinlineに配置でき、再帰する辺など必要な箇所を箱にする。

`Operand::Var` は変数、`Operand::Const` は定数を表す。定数は `Const::Int` / `Const::Int64` / `Const::Float` / `Const::Bool` / `Const::String` / `Const::Unit`。`Const::Unit` はコンテナー内の表現のない枠を保つplaceholderで、通常の値変数には束縛しない。tuple・構成子の添字と `List[()]` の長さを保つために使う。

## 値・データ操作（Value）

| 操作 | 意味・入力条件 |
| --- | --- |
| `Value::Const` | 定数を値にする |
| `Value::Prim` | 型付きの基本演算。演算と型の組み合わせは後述 |
| `Value::Call` | 関数またはeffect operationを呼ぶ。呼び出し種別は後述 |
| `Value::Builtin` | 文字列・数値変換・Cellなどの組み込み操作 |
| `Value::Make` | enum・レコードの構成子をフィールド定義順の引数で作る |
| `Value::Field` | 構成子のフィールドを読む。入力側で構成子が一致することを保証する |
| `Value::Tag` | enumの構成子indexを得る |
| `Value::Tuple` | tupleを作る |
| `Value::TupleGet` | tupleの指定indexの要素を読む |
| `Value::List` | 要素列と任意のtailからListを作る |
| `Value::ListIsEmpty` | Listが空か調べる |
| `Value::ListHead` | 非空Listの先頭を読む。非空条件は入力側で保証する |
| `Value::ListTail` | 非空Listの残りを読む |
| `Value::FnRef` | top-level関数を関数値にする |
| `Value::Closure` | 引数・明示的capture列・本体・`suspends` を持つclosureを作る。本体は親関数のarenaを共有する |
| `Value::EvLookup` | evidence表からeffectのhandlerを引く |
| `Value::EvExtend` | 空の表または既存の表にhandlerを追加する。同じeffectは後のbindingが優先する |
| `Value::Handler` | effectの宣言順にoperation closureを並べてhandler値を作る |
| `Value::Fiber` | Fiber・Deferred・Causeの操作。全variantは後述 |
| `Value::IsError` | catchしたerrorが指定error enumの値か判定する |
| `Value::AsError` | 対応する型判定後、errorをそのenum型として読む |
| `Value::Runtime` | 表示・handler失効・取消延期などのruntime操作 |
| `Value::Coerce` | 関数型のfailure/suspend/evidence ABIを適合させる、またはAnyへのbox化・Anyからの復元を行う |

構造的なEq/Showは型ごとの関数呼び出しになる。直接Flowを作る場合は `derived_binding` で有限の構造比較・表示のbindingを作れる。ユーザー定義のtrait実装を使う場合、その呼び出しはproducer側で残す。

## 文と終端（Stmt / Term）

| 文 | 意味・入力条件 |
| --- | --- |
| `Stmt::Let` | `Value` を評価して任意の `dst` に束縛する。Flowでは `on_defect` / `on_suspend` はともに `None` |
| `Stmt::If` | `i32` の0/非0でelse/thenへ分岐する |
| `Stmt::Switch` | enumのtagまたは整数literalで分岐する |
| `Stmt::Loop` | loop変数を初期値で束縛し、本体を実行する |
| `Stmt::Block` | label付きの脱出先を作る。`Break` とcatchの受け口になる |
| `Stmt::Protect` | 本体とcleanupを組にする。組み込みtargetのdefect時の契約は後述 |

`SwitchOn::Tag` は `CaseKey::Variant`、`SwitchOn::Lit` は `CaseKey::Lit` を使う。literal switchは `i32` / `i64` でdefaultが必要。tag switchはdefaultがなければ全構成子を覆う。Float/Stringのliteralパターンは比較と `If` で表す。

| 終端 | 意味・入力条件 |
| --- | --- |
| `Term::Yield` | 囲むcompound文に結果を渡す。loop本体ではloopを終了する。schedulerへのyieldではない |
| `Term::Return` | 囲む関数・closureから戻る |
| `Term::Fail` | 囲む関数・closureを回復可能なerrorで失敗させる。`fails: true` が必要 |
| `Term::Break` | 囲むlabel付きblockへ値を渡して脱出する |
| `Term::Recur` | 最も内側のloopの引数を並列に更新し、次の反復へ進む。Protectやclosureの境界を越えない |
| `Term::Defect` | Stringのmessage付きdefectを発生させる |
| `Term::Unreachable` | 制御が到達しない終端を示す |

関数・closure本体は `Yield` で終わらない。cleanupの正常終了は値なしの `Yield` とし、cleanupから外へreturn/break/recurしない。cleanup内のローカルな制御フローは使える。

## 呼び出し・失敗辺・effect

| 呼び出し先 | 意味 |
| --- | --- |
| `Callee::Direct` | `FnId` で指定した関数 |
| `Callee::Indirect` | 関数型の変数に入っている関数値 |
| `Callee::Op` | handler値・effect ID・operation indexで指定したeffect操作 |

通常の関数・closureの引数は、表現のある引数だけを並べる。evidence表を取る場合は最後の引数に置く。`fails` / `suspends` / evidenceの有無は、関数定義・関数型・呼び出しで一致させる。operation自身が型引数を持つ場合は、[LIR verifier](../../emela-lir/src/verify.rs)が定めるboxed表現の規約に従う。

失敗しうる呼び出しとFiber操作には `on_fail` を付け、失敗しないものには付けない。

| 失敗辺 | 意味 |
| --- | --- |
| `OnFail::Break` | error値を囲むblockの結果へ渡す。errorの型判定とcatch分岐につなぐ |
| `OnFail::Propagate` | 囲む関数・closureを失敗させる。`fails: true` が必要 |

`FiberOp::CauseRaise` は保守的に常にfailure辺を要求する。失敗しない関数内でも辺を省略せず、局所blockへの `OnFail::Break` などで受ける。`flow_from_core` はCore出力のこのケースを正規化するが、`compile_flow` は入力の失敗辺を自動修正しない。処理は [`normalize_infallible_cause_raise`](../src/fiber.rs) を参照する。

## 基本演算（PrimOp / PrimTy）

| 演算 | `PrimTy::I32` | `PrimTy::I64` | `PrimTy::F64` | `PrimTy::String` |
| --- | --- | --- | --- | --- |
| `PrimOp::Add` / `PrimOp::Sub` / `PrimOp::Mul` / `PrimOp::Neg` | 32 bit wrap | 64 bit wrap | double演算 | 使用不可 |
| `PrimOp::Div` / `PrimOp::Rem` | 0除算はdefect。MIN/-1はMIN、MIN%-1は0 | 同左 | IEEE除算 / fmod | 使用不可 |
| `PrimOp::Not` | 0/非0を1/0へ反転 | 使用不可 | 使用不可 | 使用不可 |
| `PrimOp::Eq` / `PrimOp::Ne` / `PrimOp::Lt` / `PrimOp::Le` / `PrimOp::Gt` / `PrimOp::Ge` | 整数比較 | 整数比較 | IEEE比較 | byte列の内容比較 |

比較と論理反転の結果は `i32`。`Neg` / `Not` は単項、他は二項。String連結は `PrimOp::Add` ではなく `Builtin::StringAppend` を使う。enum・List等の構造比較は `Prim` ではなく生成した比較関数を呼ぶ。

## Flowで使う組み込み操作（Builtin）

以下は `Value::Builtin` の `op` に指定する。型引数は単相化済みで、失敗辺を持たない。容量不足などによるdefectは起こりうる。定義と署名は [`intrinsics.rs`](../../emela-core/src/intrinsics.rs) を参照する。

| 分類 | variant | 内容 |
| --- | --- | --- |
| Stringの長さ | `Builtin::StringLength` / `Builtin::StringByteSize` | 書記素数 / UTF-8のbyte長 |
| Stringの連結 | `Builtin::StringAppend` / `Builtin::StringJoin` | 2文字列 / List内の文字列の結合 |
| Stringの検索 | `Builtin::StringContains` / `Builtin::StringStartsWith` / `Builtin::StringEndsWith` | 部分文字列 / prefix / suffixの判定 |
| Stringの分割 | `Builtin::StringSplit` / `Builtin::StringChars` | separatorによる分割 / 書記素ごとのList。空separatorも書記素境界 |
| Stringの変換 | `Builtin::StringTrim` / `Builtin::StringCodePoints` / `Builtin::StringFromCodePoint` | Unicode空白除去 / code point列 / scalarからの文字列 |
| Intの変換・検査 | `Builtin::IntToString` / `Builtin::IntParse` / `Builtin::IntCheckedAdd` | 十進表示 / parse / overflow付き加算。後二つはOptionを返す |
| Intのbit演算 | `Builtin::IntBitAnd` / `Builtin::IntBitOr` / `Builtin::IntBitXor` / `Builtin::IntBitNot` | 32 bitのAND / OR / XOR / 反転 |
| Intのshift | `Builtin::IntShiftLeft` / `Builtin::IntShiftRight` / `Builtin::IntShiftRightUnsigned` | 左 / 算術右 / 論理右shift |
| Int64の変換 | `Builtin::Int64FromInt` / `Builtin::Int64ToString` / `Builtin::Int64ToInt` | 符号拡張 / 十進表示 / 下位32 bitの取得 |
| Int64のbit演算 | `Builtin::Int64BitAnd` / `Builtin::Int64BitOr` / `Builtin::Int64BitXor` / `Builtin::Int64BitNot` | 64 bitのAND / OR / XOR / 反転 |
| Int64のshift | `Builtin::Int64ShiftLeft` / `Builtin::Int64ShiftRight` / `Builtin::Int64ShiftRightUnsigned` | 左 / 算術右 / 論理右shift |
| Floatの変換 | `Builtin::FloatToString` / `Builtin::FloatFromInt` / `Builtin::FloatParse` | 表示 / Intからの変換 / Optionを返すparse |
| Floatの丸め | `Builtin::FloatFloor` / `Builtin::FloatCeil` / `Builtin::FloatRound` / `Builtin::FloatTruncate` | 負方向 / 正方向 / 最近接（tieは0から遠く） / 小数部切捨てでIntにする |
| 共有状態 | `Builtin::CellNew` / `Builtin::CellGet` / `Builtin::CellSet` | Cellの生成 / 読み取り / 更新 |

FloatからIntへの丸めは非有限値・範囲外でdefectになる。Stringを生成する操作には容量と生存期間の計画が必要で、ListやCellなどには固定heapを使う。Stringの所有権と容量の詳細は [値表現・メモリ](implementation.md#2-値表現メモリ所有権)を参照する。

## Flowで使うruntime操作（RuntimeOp）

| 操作 | 内容 |
| --- | --- |
| `RuntimeOp::ShowString` | Stringを引用符とescape付きの表示用Stringにする |
| `RuntimeOp::DebugPrint` | 表示済みStringをboardのdebug hookへ渡す |
| `RuntimeOp::Retire` | release後のhandlerを失効させる。以後のoperation使用を検査する |
| `RuntimeOp::Hold` | release中のFiberのinterrupt/cancelを延期する |
| `RuntimeOp::Unhold` | 延期したinterrupt/cancelを再開する |

Hold/Unholdはhardware割込みのmaskではない。

## Fiber・Deferred・Cause（FiberOp）

いずれも `Value::Fiber` で表す。引数順・結果・`suspends()` / `fails()` は [`FiberOp` の定義](../../emela-core/src/ir.rs) を参照する。表の「中断しうる」はIR上の印であり、必ず中断するという意味ではない。

| 操作 | 内容 | 中断しうる |
| --- | --- | --- |
| `FiberOp::Scope` | 子Fiberの生存範囲を作る。終了時に残る子を取り消し、cleanup完了を待つ | はい |
| `FiberOp::Fork` | scope内に子Fiberを作る。子は親の次の実際の中断で実行を開始する | いいえ |
| `FiberOp::Deferred` | scope内に一度だけ完成するDeferredを作る | いいえ |
| `FiberOp::Join` | 子の成功値を待つ。中断された子のjoinはdefect | はい |
| `FiberOp::Await` | 子のExitを待つ | はい |
| `FiberOp::Interrupt` | 子の取消を要求し、cleanup完了を待つ | はい |
| `FiberOp::Attempt` | callbackを呼び、回復可能な失敗をCause付きExitにする | `suspends` に従う |
| `FiberOp::Complete` | Deferredへ最初の値を保存し、保存できたかBoolで返す | いいえ |
| `FiberOp::DeferredAwait` | Deferredの完成を待つ | はい |
| `FiberOp::Poll` | Deferredの現在値をOptionで読む | いいえ |
| `FiberOp::CauseKind` | Causeの分類を読む | いいえ |
| `FiberOp::CauseToString` | Causeを表示用Stringにする | いいえ |
| `FiberOp::CauseRaise` | CauseのFail / Interrupt等を制御経路へ再送する。戻り値はNever | いいえ |

## Flowの入力に含めない要素

FlowとHeapはRustのデータ型を共有するが、次の要素はこのbackendのFlow入口で拒否する。

| 対象 | Flowでの表現・役割分担 |
| --- | --- |
| `Stage::ClosuresConverted` / `Stage::DefectsExplicit` / `Stage::StateMachines` / `Stage::RefCounted` | `Stage::Flow` を渡す。後段のHeapパスは実行しない |
| `Value::MakeClosure` / `Value::EnvGet` | `Value::FnRef` / `Value::Closure` を渡す。環境の配置はbackendが決める |
| `Stmt::Dup` / `Stmt::Drop` | 明示RCを渡さない。backendが所有権処理を挿入する |
| `Term::Unwind` / `OnFail::Handle` | `Term::Defect`、`OnFail::Break` / `OnFail::Propagate` と `Protect` で表す |
| `Stmt::Let` の `on_defect` / `on_suspend` | ともに `None`。Heap用のlanding・`Suspend` のstate/saved情報を付けない |
| `RuntimeOp::Defect` / `RuntimeOp::DefectTake` / `RuntimeOp::DefectResume` | 保留defectの明示unwindはHeap専用 |
| `RuntimeOp::Yield` / `RuntimeOp::YieldPerform` | Heapの公平性yield。組み込みschedulerが必要な実行制御を生成する |

また、次のsource builtinはFlowへ変換するまでに置き換える。生の `Value::Builtin` として渡すと拒否される。

| builtin | Flowでの表現 |
| --- | --- |
| `Builtin::Panic` / `Builtin::Todo` | message付き `Term::Defect` |
| `Builtin::Dbg` / `Builtin::Inspect` / `Builtin::ShowNested` | 単相化で選んだShowや構造表示の関数呼び出し、必要なruntime操作。dbgは元の値を返す |
| `Builtin::ShowString` / `Builtin::DebugPrint` | `RuntimeOp::ShowString` / `RuntimeOp::DebugPrint` |

## 組み込みtargetでの入力・実行条件

- 型・error/effect集合を確定させ、関数型・呼び出し・evidenceのABIを一致させる。`compile_flow` は閉じた関数型の集合情報を正規化するが、未解決の型引数や集合引数は受け付けない。
- Flowの構造検証には `verify_flow`、C生成と資源・外部ABIを含む確認には `compile_flow` を使う。組み立てには [`FnBuilder`](../../emela-lir/src/build.rs)、確認用の表示には `emela_lir::dump` を使える。
- List・動的capture・Cell等で固定heapが必要なら `Options::with_heap_bytes` を設定する。既定値は0。String、frame、Fiber、task、event、Deferredにも容量があり、必要なruntimeと領域だけを生成する。[設定値一覧](implementation.md#設定値)を参照する。
- **このtargetのdefectは即時停止する。** `emela_board_defect` へkind・messageを渡し、Protect cleanupや参照解放を行わない。`Attempt` もdefectをDieとして捕捉しない。回復可能なfailsやFiber取消では必要なcleanupを実行する。
- 非scalar・fails付き同期externalには明示的なexternal adapter、中断するexternalにはtask adapterを指定する。ISR entryにも到達先の制約がある。[外部C・ISRの契約](implementation.md#4-外部cisrボードとの境界)を参照する。

Flowの構造とstage境界の回帰試験は [`tests/flow.rs`](../tests/flow.rs)、loweringとdumpの例は [`emela-lir/tests`](../../emela-lir/tests) にある。周辺機器のeffectとboard接続は [組み込みインターフェース](embedded-interfaces.md)・[標準周辺機器bridge](embedded-peripherals.md)を参照する。
